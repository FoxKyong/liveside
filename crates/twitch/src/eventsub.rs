//! EventSub over WebSocket: the channel through which Twitch delivers chat.
//!
//! The socket itself carries no subscriptions. Twitch sends a welcome with a
//! session id, the client creates subscriptions for that session through
//! Helix, and notifications then arrive here. Twitch also sends keepalives;
//! silence longer than the announced timeout means the connection is dead.

use std::time::Duration;

use futures_util::StreamExt;
use serde::Deserialize;
use tokio::sync::mpsc;

/// Twitch's default EventSub WebSocket endpoint.
pub const DEFAULT_URL: &str = "wss://eventsub.wss.twitch.tv/ws";

/// Grace period on top of the keepalive interval Twitch announces, so a
/// slightly late keepalive does not drop a healthy connection.
const KEEPALIVE_GRACE: Duration = Duration::from_secs(10);

/// What the socket reports to its owner.
#[derive(Debug)]
pub enum Message {
    /// The session is ready; subscriptions can be created for `session_id`.
    /// Twitch closes the socket unless a subscription follows within 10 s.
    Welcome {
        session_id: String,
        /// Whether this session inherited the last one's subscriptions,
        /// which it does when the connection was made to a reconnect URL.
        resumed: bool,
    },
    /// An event notification: the subscription type and its `event` object.
    Notification {
        kind: String,
        event: serde_json::Value,
    },
    /// Twitch revoked a subscription (token gone, channel gone, ...).
    Revoked { kind: String, status: String },
    /// The connection ended. The owner reconnects.
    Closed {
        reason: String,
        /// Where Twitch asked us to carry on. A connection made to this
        /// URL keeps the subscriptions of the one it replaces, so the
        /// owner must not make them again.
        reconnect_to: Option<String>,
    },
}

#[derive(Deserialize)]
struct Envelope {
    metadata: Metadata,
    #[serde(default)]
    payload: serde_json::Value,
}

#[derive(Deserialize)]
struct Metadata {
    message_type: String,
    #[serde(default)]
    subscription_type: Option<String>,
}

#[derive(Deserialize)]
struct SessionPayload {
    session: Session,
}

#[derive(Deserialize)]
struct Session {
    id: String,
    #[serde(default)]
    keepalive_timeout_seconds: Option<u64>,
    #[serde(default)]
    reconnect_url: Option<String>,
}

/// Connects and pumps messages into `out` until the connection ends. Always
/// finishes by sending [`Message::Closed`].
///
/// Reconnects are the caller's job: this returns, and the caller opens a new
/// connection (to `reconnect_url` when Twitch offered one) and re-subscribes.
pub async fn run(url: &str, resumed: bool, out: mpsc::Sender<Message>) {
    let mut socket = match tokio_tungstenite::connect_async(url).await {
        Ok((socket, _)) => socket,
        Err(e) => {
            let _ = out
                .send(Message::Closed {
                    reason: format!("cannot connect to {url}: {e}"),
                    reconnect_to: None,
                })
                .await;
            return;
        }
    };

    // Until the welcome says otherwise, Twitch's default is 10 s.
    let mut keepalive = Duration::from_secs(10) + KEEPALIVE_GRACE;
    let stop = loop {
        let next = tokio::time::timeout(keepalive, socket.next()).await;
        let frame = match next {
            Err(_) => break ended(format!("no keepalive for {} s", keepalive.as_secs())),
            Ok(None) => break ended("connection closed by Twitch".to_owned()),
            Ok(Some(Err(e))) => break ended(format!("connection error: {e}")),
            Ok(Some(Ok(frame))) => frame,
        };
        let text = match frame {
            tokio_tungstenite::tungstenite::Message::Text(text) => text,
            tokio_tungstenite::tungstenite::Message::Close(_) => {
                break ended("connection closed by Twitch".to_owned());
            }
            // Ping/pong is handled by tungstenite; other frames are not used.
            _ => continue,
        };
        match interpret(&text, resumed, &mut keepalive) {
            Step::Ignore => continue,
            Step::Deliver(message) => {
                if out.send(message).await.is_err() {
                    return; // The owner is gone.
                }
            }
            stop @ Step::Stop { .. } => break stop,
        }
    };
    let Step::Stop {
        reason,
        reconnect_to,
    } = stop
    else {
        return;
    };
    let _ = out
        .send(Message::Closed {
            reason,
            reconnect_to,
        })
        .await;
}

/// What one message from Twitch means to the loop above.
#[derive(Debug)]
enum Step {
    /// Pass it on to the owner.
    Deliver(Message),
    /// A keepalive, or something this client does not know about.
    Ignore,
    /// Stop reading. `reconnect_to` is where Twitch asked us to carry on,
    /// which is the one case where the subscriptions are kept.
    Stop {
        reason: String,
        reconnect_to: Option<String>,
    },
}

/// The end of a connection that keeps nothing.
fn ended(reason: String) -> Step {
    Step::Stop {
        reason,
        reconnect_to: None,
    }
}

/// Reads one message. `resumed` says whether this connection was made to
/// a reconnect URL, which the welcome passes on to the owner because it
/// decides whether the subscriptions have to be made again.
fn interpret(text: &str, resumed: bool, keepalive: &mut Duration) -> Step {
    let envelope: Envelope = match serde_json::from_str(text) {
        Ok(envelope) => envelope,
        Err(e) => return ended(format!("unreadable message: {e}")),
    };
    match envelope.metadata.message_type.as_str() {
        "session_welcome" => match serde_json::from_value::<SessionPayload>(envelope.payload) {
            Ok(payload) => {
                if let Some(seconds) = payload.session.keepalive_timeout_seconds {
                    *keepalive = Duration::from_secs(seconds) + KEEPALIVE_GRACE;
                }
                Step::Deliver(Message::Welcome {
                    session_id: payload.session.id,
                    resumed,
                })
            }
            Err(e) => ended(format!("unreadable welcome: {e}")),
        },
        "session_keepalive" => Step::Ignore,
        "notification" => {
            let kind = envelope
                .metadata
                .subscription_type
                .unwrap_or_else(|| "unknown".to_owned());
            let event = envelope
                .payload
                .get("event")
                .cloned()
                .unwrap_or(serde_json::Value::Null);
            Step::Deliver(Message::Notification { kind, event })
        }
        // Twitch is about to close this connection and has given us
        // somewhere to carry on. A connection made to that URL keeps this
        // one's subscriptions, so the owner must not make them again:
        // connecting afresh instead left a second set behind at every
        // reconnect, and Twitch counts them against the ten points a
        // token may spend, which took the chat down after a few hours.
        "session_reconnect" => {
            let url = serde_json::from_value::<SessionPayload>(envelope.payload)
                .ok()
                .and_then(|payload| payload.session.reconnect_url);
            let reason = match &url {
                Some(url) => format!("Twitch asked us to reconnect to {url}"),
                None => "Twitch asked us to reconnect".to_owned(),
            };
            Step::Stop {
                reason,
                reconnect_to: url,
            }
        }
        "revocation" => {
            let kind = envelope
                .metadata
                .subscription_type
                .unwrap_or_else(|| "unknown".to_owned());
            let status = envelope
                .payload
                .get("subscription")
                .and_then(|s| s.get("status"))
                .and_then(|s| s.as_str())
                .unwrap_or("revoked")
                .to_owned();
            Step::Deliver(Message::Revoked { kind, status })
        }
        // Unknown message types are Twitch's business, not ours.
        _ => Step::Ignore,
    }
}

/// One of the subscriptions that are worth having but that not every
/// account may have in every channel.
pub struct Extra {
    pub kind: &'static str,
    pub version: &'static str,
    pub about: About,
}

/// Who a subscription's condition names besides the channel, which
/// decides both its shape and who Twitch will grant it to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum About {
    /// The channel alone.
    Channel,
    /// The channel and us as one of its moderators.
    Moderator,
    /// The channel and us as an ordinary viewer in it.
    Viewer,
}

/// Things that happen in a channel besides chat. None of these is needed
/// to read a channel, and Twitch refuses most of them unless the account
/// moderates it or owns it, so a refusal is ordinary and must never stop
/// a join.
///
/// `channel.raid` is deliberately absent: Twitch says it only fires for
/// raids that appear in chat, which already reach us as chat
/// notifications, and subscribing would show every raid twice.
pub const EXTRA_SUBSCRIPTIONS: &[Extra] = &[
    // No scope at all: anyone may watch a channel go live or change.
    Extra {
        kind: "stream.online",
        version: "1",
        about: About::Channel,
    },
    Extra {
        kind: "stream.offline",
        version: "1",
        about: About::Channel,
    },
    Extra {
        kind: "channel.update",
        version: "2",
        about: About::Channel,
    },
    // Moderators of that channel.
    Extra {
        kind: "channel.follow",
        version: "2",
        about: About::Moderator,
    },
    Extra {
        kind: "channel.shield_mode.begin",
        version: "1",
        about: About::Moderator,
    },
    Extra {
        kind: "channel.shield_mode.end",
        version: "1",
        about: About::Moderator,
    },
    Extra {
        kind: "channel.shoutout.create",
        version: "1",
        about: About::Moderator,
    },
    Extra {
        kind: "channel.shoutout.receive",
        version: "1",
        about: About::Moderator,
    },
    // The queue of messages caught before they reach the chat. AutoMod's
    // own, and the ones the channel's blocked terms stopped.
    Extra {
        kind: "automod.message.hold",
        version: "2",
        about: About::Moderator,
    },
    Extra {
        kind: "automod.message.update",
        version: "2",
        about: About::Moderator,
    },
    // Whoever Twitch does not trust: a ban evader, somebody banned in a
    // channel this one shares bans with, or a person a moderator flagged.
    Extra {
        kind: "channel.suspicious_user.message",
        version: "1",
        about: About::Moderator,
    },
    // And a moderator deciding about one of them. channel.moderate does
    // not carry this (checked against its action list on 2026-10-11),
    // so without this subscription one moderator's decision is invisible
    // to the next.
    Extra {
        kind: "channel.suspicious_user.update",
        version: "1",
        about: About::Moderator,
    },
    // Every moderator action in the channel, in one subscription: the
    // moderation log.
    Extra {
        kind: "channel.moderate",
        version: "2",
        about: About::Moderator,
    },
    // People asking to be let back in.
    Extra {
        kind: "channel.unban_request.create",
        version: "1",
        about: About::Moderator,
    },
    Extra {
        kind: "channel.unban_request.resolve",
        version: "1",
        about: About::Moderator,
    },
    // Our own messages: AutoMod holding one explains why it never
    // appeared, which is otherwise a mystery to whoever wrote it.
    Extra {
        kind: "channel.chat.user_message_hold",
        version: "1",
        about: About::Viewer,
    },
    Extra {
        kind: "channel.chat.user_message_update",
        version: "1",
        about: About::Viewer,
    },
    // The broadcaster's own: a poll, from the first vote to the result.
    Extra {
        kind: "channel.poll.begin",
        version: "1",
        about: About::Channel,
    },
    Extra {
        kind: "channel.poll.progress",
        version: "1",
        about: About::Channel,
    },
    Extra {
        kind: "channel.poll.end",
        version: "1",
        about: About::Channel,
    },
    // ...and a prediction, which has one more state than a poll: betting
    // closes before anybody knows the answer.
    Extra {
        kind: "channel.prediction.begin",
        version: "1",
        about: About::Channel,
    },
    Extra {
        kind: "channel.prediction.progress",
        version: "1",
        about: About::Channel,
    },
    Extra {
        kind: "channel.prediction.lock",
        version: "1",
        about: About::Channel,
    },
    Extra {
        kind: "channel.prediction.end",
        version: "1",
        about: About::Channel,
    },
    // What people spend their channel points on. Without a reward id in
    // the condition this is every custom reward, whoever made it.
    Extra {
        kind: "channel.channel_points_custom_reward_redemption.add",
        version: "1",
        about: About::Channel,
    },
    // ...and what became of one, which is how the queue empties when it
    // is answered somewhere else.
    Extra {
        kind: "channel.channel_points_custom_reward_redemption.update",
        version: "1",
        about: About::Channel,
    },
    // The money. Subscriptions, what people pay with bits, and the three
    // things Twitch counts towards something: a Hype Train, a goal and a
    // charity campaign.
    Extra {
        kind: "channel.subscribe",
        version: "1",
        about: About::Channel,
    },
    Extra {
        kind: "channel.subscription.message",
        version: "1",
        about: About::Channel,
    },
    Extra {
        kind: "channel.subscription.gift",
        version: "1",
        about: About::Channel,
    },
    Extra {
        kind: "channel.cheer",
        version: "1",
        about: About::Channel,
    },
    // Version 2 for the Hype Train; version 1 for the rest.
    Extra {
        kind: "channel.hype_train.begin",
        version: "2",
        about: About::Channel,
    },
    Extra {
        kind: "channel.hype_train.end",
        version: "2",
        about: About::Channel,
    },
    Extra {
        kind: "channel.goal.begin",
        version: "1",
        about: About::Channel,
    },
    Extra {
        kind: "channel.goal.end",
        version: "1",
        about: About::Channel,
    },
    Extra {
        kind: "channel.charity_campaign.donate",
        version: "1",
        about: About::Channel,
    },
    // A campaign starting and stopping, which is the frame the donations
    // are counted inside.
    Extra {
        kind: "channel.charity_campaign.start",
        version: "1",
        about: About::Channel,
    },
    Extra {
        kind: "channel.charity_campaign.stop",
        version: "1",
        about: About::Channel,
    },
    // A Shared Chat session starting and ending, so the chat can say
    // whose messages are about to start arriving.
    Extra {
        kind: "channel.shared_chat.begin",
        version: "1",
        about: About::Channel,
    },
    Extra {
        kind: "channel.shared_chat.update",
        version: "1",
        about: About::Channel,
    },
    Extra {
        kind: "channel.shared_chat.end",
        version: "1",
        about: About::Channel,
    },
    // The broadcaster's own: when the advertisements run.
    Extra {
        kind: "channel.ad_break.begin",
        version: "1",
        about: About::Channel,
    },
];

/// Whether a subscription of this type is paid for out of the socket's
/// budget. Twitch allows ten points per token: a subscription about a
/// user who has authorized this client is free, one about anybody else
/// costs a point, and once the ten are spent Twitch refuses *every*
/// further subscription, free ones included — which took chat itself
/// down from the fourth channel on. These are the ones that need no
/// permission from the channel, so these are the ones that cost; they
/// are asked for about our own channel, where they are free, and
/// everywhere else the client makes do: the live mark comes from Get
/// Streams once a minute, and a shared chat is noticed on joining
/// rather than the moment it starts.
pub fn costs_budget(kind: &str) -> bool {
    matches!(
        kind,
        "stream.online"
            | "stream.offline"
            | "channel.update"
            | "channel.shared_chat.begin"
            | "channel.shared_chat.update"
            | "channel.shared_chat.end"
    )
}

/// What this client subscribes to about the account itself rather than
/// about a channel: a whisper arrives wherever we are.
pub const ACCOUNT_SUBSCRIPTIONS: &[(&str, &str)] = &[("user.whisper.message", "1")];

/// Subscription types this client creates for every joined channel, with
/// their versions.
pub const CHANNEL_SUBSCRIPTIONS: &[(&str, &str)] = &[
    ("channel.chat.message", "1"),
    ("channel.chat.message_delete", "1"),
    ("channel.chat.clear", "1"),
    ("channel.chat.clear_user_messages", "1"),
    ("channel.chat.notification", "1"),
    ("channel.chat_settings.update", "1"),
];

#[cfg(test)]
mod tests {
    use super::*;

    fn read(text: &str, resumed: bool) -> Step {
        let mut keepalive = Duration::from_secs(10);
        interpret(text, resumed, &mut keepalive)
    }

    #[test]
    fn a_reconnect_keeps_the_url_twitch_offered() {
        let text = r#"{
            "metadata": {"message_id": "1", "message_type": "session_reconnect"},
            "payload": {"session": {
                "id": "second", "status": "reconnecting",
                "reconnect_url": "wss://eventsub.wss.twitch.tv/ws?challenge=abc"
            }}
        }"#;
        // Without the URL the client would open a plain connection, make
        // every subscription a second time, and spend a budget Twitch
        // allows ten points of.
        match read(text, false) {
            Step::Stop { reconnect_to, .. } => assert_eq!(
                reconnect_to.as_deref(),
                Some("wss://eventsub.wss.twitch.tv/ws?challenge=abc")
            ),
            other => panic!("expected a stop, got {other:?}"),
        }
    }

    #[test]
    fn a_welcome_says_whether_the_subscriptions_came_with_it() {
        let text = r#"{
            "metadata": {"message_id": "1", "message_type": "session_welcome"},
            "payload": {"session": {"id": "abc", "status": "connected",
                                    "keepalive_timeout_seconds": 30}}
        }"#;
        let mut keepalive = Duration::from_secs(10);
        match interpret(text, true, &mut keepalive) {
            Step::Deliver(Message::Welcome {
                session_id,
                resumed,
            }) => {
                assert_eq!(session_id, "abc");
                assert!(resumed);
            }
            other => panic!("expected a welcome, got {other:?}"),
        }
        assert_eq!(keepalive, Duration::from_secs(30) + KEEPALIVE_GRACE);
        // And a connection made to the plain URL carries nothing over.
        match read(text, false) {
            Step::Deliver(Message::Welcome { resumed, .. }) => assert!(!resumed),
            other => panic!("expected a welcome, got {other:?}"),
        }
    }

    #[test]
    fn a_keepalive_is_nothing_to_pass_on() {
        let text = r#"{"metadata": {"message_id": "1", "message_type": "session_keepalive"},
                       "payload": {}}"#;
        assert!(matches!(read(text, false), Step::Ignore));
    }
}
