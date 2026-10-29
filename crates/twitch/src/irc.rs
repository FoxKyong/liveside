//! Twitch IRC, used for two things Twitch's newer interfaces do not do:
//! seeing who is in a channel, and sending `/me`.
//!
//! EventSub says nothing about people joining or leaving, and Helix tells
//! only moderators who is present. The old IRC interface still carries
//! JOIN and PART, so a client that wants a real user list has to speak it.
//! And Helix's Send Chat Message takes no commands, `/me` included, which
//! IRC does with the `twitch.tv/commands` capability (dev.twitch.tv/docs/
//! chat/irc, checked 2026-10-17).
//!
//! Chat itself still comes from EventSub and goes out through Helix: these
//! connections are for those two things only, which keeps them apart and
//! easy to turn off.
//!
//! Measured against the live server on 2026-09-24: an anonymous connection
//! (`justinfan…`) is enough. Joining a channel answers with its member
//! list, in as many 353 pieces as the server likes, ended by 366 — 666
//! names for a channel with some 1500 viewers, 7 for a quiet one. JOIN and
//! PART then arrive for small channels; in busy ones Twitch sends few or
//! none, which is why the list is asked for again from time to time.
//!
//! An authenticated login (`Auth::Token`) needs the `chat:read` scope and
//! buys nothing here, so this client does not ask for that scope.

use futures_util::{SinkExt, StreamExt};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite;

/// Twitch's IRC-over-WebSocket endpoint, so this needs no second TLS stack.
pub const DEFAULT_URL: &str = "wss://irc-ws.chat.twitch.tv:443";

/// What the connection reports to its owner. Channels are logins without
/// the leading `#`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// Logged in; the owner may ask to join channels.
    Ready,
    /// The channel's member list, complete: the server sends it in as many
    /// pieces as it likes, and this is the assembled result.
    Names {
        channel: String,
        users: Vec<String>,
    },
    Joined {
        channel: String,
        user: String,
    },
    Parted {
        channel: String,
        user: String,
    },
    /// Twitch's answer to something done here, in its own words: a
    /// message that was not sent, a channel that could not be joined.
    Notice {
        channel: Option<String>,
        text: String,
    },
    /// The connection ended; the owner reconnects and rejoins.
    Closed {
        reason: String,
    },
}

#[derive(Debug, Clone)]
pub enum Command {
    Join(String),
    Part(String),
    /// A line to send to a channel, as typed: `/me waves` with the
    /// commands capability.
    Say {
        channel: String,
        text: String,
    },
}

/// Who is in a channel.
pub const MEMBERSHIP: &str = "twitch.tv/membership";
/// Sending `/me`.
pub const COMMANDS: &str = "twitch.tv/commands";

/// How to log in. Anonymous works without any scope, but sees less.
#[derive(Debug, Clone)]
pub enum Auth {
    Anonymous,
    /// A token with the `chat:read` scope, and the account's login.
    Token {
        login: String,
        token: String,
    },
}

/// Runs one connection until it ends, always finishing with
/// [`Event::Closed`].
pub async fn run(
    url: &str,
    auth: Auth,
    capability: &str,
    mut commands: mpsc::Receiver<Command>,
    out: mpsc::Sender<Event>,
) {
    let mut socket = match tokio_tungstenite::connect_async(url).await {
        Ok((socket, _)) => socket,
        Err(e) => {
            let _ = out
                .send(Event::Closed {
                    reason: format!("cannot connect to {url}: {e}"),
                })
                .await;
            return;
        }
    };

    // One capability, for what this connection is for; chat text and its
    // tags come from EventSub.
    let (pass, nick) = match &auth {
        Auth::Anonymous => (
            "PASS SCHMOOPIIE".to_owned(),
            // Twitch's convention for a read-only anonymous connection.
            format!("NICK justinfan{}", std::process::id() % 100_000),
        ),
        Auth::Token { login, token } => (format!("PASS oauth:{token}"), format!("NICK {login}")),
    };
    for line in [format!("CAP REQ :{capability}"), pass, nick] {
        if socket
            .send(tungstenite::Message::Text(line.into()))
            .await
            .is_err()
        {
            let _ = out
                .send(Event::Closed {
                    reason: "connection closed while logging in".to_owned(),
                })
                .await;
            return;
        }
    }

    // 353 arrives in pieces and 366 ends the list, so the pieces are kept
    // here until then. Treating each piece as the whole list leaves the
    // channel holding whatever the last piece happened to contain.
    let mut pending: std::collections::HashMap<String, Vec<String>> =
        std::collections::HashMap::new();
    let reason = loop {
        tokio::select! {
            command = commands.recv() => {
                let line = match command {
                    Some(command) => line_for(&command),
                    // The owner is gone.
                    None => break "no longer needed".to_owned(),
                };
                if socket.send(tungstenite::Message::Text(line.into())).await.is_err() {
                    break "connection closed".to_owned();
                }
            }
            frame = socket.next() => {
                let text = match frame {
                    Some(Ok(tungstenite::Message::Text(text))) => text,
                    Some(Ok(tungstenite::Message::Close(_))) | None => {
                        break "connection closed by Twitch".to_owned();
                    }
                    Some(Err(e)) => break format!("connection error: {e}"),
                    // Ping/pong at the WebSocket level is tungstenite's job.
                    Some(Ok(_)) => continue,
                };
                for line in text.split("\r\n").filter(|line| !line.is_empty()) {
                    match parse(line) {
                        Some(Parsed::Pong(payload)) => {
                            let pong = format!("PONG :{payload}");
                            if socket.send(tungstenite::Message::Text(pong.into())).await.is_err() {
                                break;
                            }
                        }
                        // Twitch is about to drop us; the owner reconnects.
                        Some(Parsed::Reconnect) => {
                            break;
                        }
                        Some(Parsed::Failed(reason)) => {
                            let _ = out.send(Event::Closed { reason }).await;
                            return;
                        }
                        Some(Parsed::NamesPart { channel, users }) => {
                            pending.entry(channel).or_default().extend(users);
                        }
                        Some(Parsed::NamesEnd { channel }) => {
                            let users = pending.remove(&channel).unwrap_or_default();
                            if out.send(Event::Names { channel, users }).await.is_err() {
                                return;
                            }
                        }
                        Some(Parsed::Event(event)) => {
                            if out.send(event).await.is_err() {
                                return;
                            }
                        }
                        None => {}
                    }
                }
            }
        }
    };
    let _ = out.send(Event::Closed { reason }).await;
}

/// The IRC line for a command.
fn line_for(command: &Command) -> String {
    match command {
        Command::Join(channel) => format!("JOIN #{channel}"),
        Command::Part(channel) => format!("PART #{channel}"),
        // One line: a line break would end the command, and what follows
        // it would be read as another.
        Command::Say { channel, text } => {
            format!("PRIVMSG #{channel} :{}", text.replace(['\r', '\n'], " "))
        }
    }
}

/// What one line means to us. Everything else is ignored: chat, room state
/// and the welcome banter all arrive here too.
#[derive(Debug, PartialEq, Eq)]
enum Parsed {
    Event(Event),
    /// One piece of a member list (353).
    NamesPart {
        channel: String,
        users: Vec<String>,
    },
    /// The list is complete (366).
    NamesEnd {
        channel: String,
    },
    Pong(String),
    Reconnect,
    /// Login refused, e.g. a token without `chat:read`.
    Failed(String),
}

fn parse(line: &str) -> Option<Parsed> {
    if let Some(payload) = line.strip_prefix("PING") {
        return Some(Parsed::Pong(
            payload.trim().trim_start_matches(':').to_owned(),
        ));
    }
    // `:nick!user@host COMMAND params`, or `:tmi.twitch.tv 353 nick = #chan :a b`.
    let rest = line.strip_prefix(':')?;
    let (prefix, rest) = rest.split_once(' ')?;
    let nick = prefix
        .split_once('!')
        .map(|(nick, _)| nick)
        .unwrap_or(prefix);
    let (command, params) = rest.split_once(' ').unwrap_or((rest, ""));
    match command {
        "001" => Some(Parsed::Event(Event::Ready)),
        // The member list, in as many 353s as the server likes.
        "353" => {
            let (targets, names) = params.split_once(" :")?;
            let channel = targets.split(' ').find_map(|p| p.strip_prefix('#'))?;
            Some(Parsed::NamesPart {
                channel: channel.to_owned(),
                users: names
                    .split(' ')
                    .filter(|name| !name.is_empty())
                    .map(str::to_owned)
                    .collect(),
            })
        }
        // End of the list: what was collected so far is the answer.
        "366" => {
            let channel = params.split(' ').find_map(|p| p.strip_prefix('#'))?;
            Some(Parsed::NamesEnd {
                channel: channel.to_owned(),
            })
        }
        "JOIN" | "PART" => {
            let channel = params.split(' ').next()?.strip_prefix('#')?.to_owned();
            let user = nick.to_owned();
            Some(Parsed::Event(if command == "JOIN" {
                Event::Joined { channel, user }
            } else {
                Event::Parted { channel, user }
            }))
        }
        "RECONNECT" => Some(Parsed::Reconnect),
        "NOTICE"
            if params.contains("Login authentication failed")
                || params.contains("Improperly formatted auth") =>
        {
            Some(Parsed::Failed(
                "Twitch refused the login; a token for chat needs the chat:read scope".to_owned(),
            ))
        }
        // `NOTICE #channel :text`, or `NOTICE * :text` for the connection.
        "NOTICE" => {
            let (target, text) = params.split_once(" :")?;
            Some(Parsed::Event(Event::Notice {
                channel: target.strip_prefix('#').map(str::to_owned),
                text: text.to_owned(),
            }))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_me_is_one_privmsg_and_twitchs_refusals_come_back() {
        assert_eq!(
            line_for(&Command::Say {
                channel: "harborlight".into(),
                text: "/me waves\r\nPRIVMSG #other :sneaky".into(),
            }),
            "PRIVMSG #harborlight :/me waves  PRIVMSG #other :sneaky"
        );
        assert_eq!(
            line_for(&Command::Join("harborlight".into())),
            "JOIN #harborlight"
        );
        assert_eq!(
            parse(
                ":tmi.twitch.tv NOTICE #harborlight :You are permanently banned from talking in harborlight."
            ),
            Some(Parsed::Event(Event::Notice {
                channel: Some("harborlight".into()),
                text: "You are permanently banned from talking in harborlight.".into(),
            }))
        );
        assert_eq!(
            parse(":tmi.twitch.tv NOTICE * :Improperly formatted auth"),
            Some(Parsed::Failed(
                "Twitch refused the login; a token for chat needs the chat:read scope".into()
            ))
        );
    }

    /// Lines as the live server sent them on 2026-09-24.
    #[test]
    fn reads_the_lines_that_matter() {
        assert_eq!(
            parse("PING :tmi.twitch.tv"),
            Some(Parsed::Pong("tmi.twitch.tv".to_owned()))
        );
        assert_eq!(
            parse(":tmi.twitch.tv 001 justinfan838 :Welcome, GLHF!"),
            Some(Parsed::Event(Event::Ready))
        );
        assert_eq!(
            parse(
                ":justinfan838.tmi.twitch.tv 353 justinfan838 = #snowyvalley :justinfan838 someone_else"
            ),
            Some(Parsed::NamesPart {
                channel: "snowyvalley".to_owned(),
                users: vec!["justinfan838".to_owned(), "someone_else".to_owned()],
            })
        );
        // The list is complete only when 366 says so.
        assert_eq!(
            parse(":justinfan838.tmi.twitch.tv 366 justinfan838 #snowyvalley :End of /NAMES list"),
            Some(Parsed::NamesEnd {
                channel: "snowyvalley".to_owned(),
            })
        );
        assert_eq!(
            parse(":foo!foo@foo.tmi.twitch.tv JOIN #bar"),
            Some(Parsed::Event(Event::Joined {
                channel: "bar".to_owned(),
                user: "foo".to_owned(),
            }))
        );
        assert_eq!(
            parse(":foo!foo@foo.tmi.twitch.tv PART #bar"),
            Some(Parsed::Event(Event::Parted {
                channel: "bar".to_owned(),
                user: "foo".to_owned(),
            }))
        );
        assert_eq!(parse(":tmi.twitch.tv RECONNECT"), Some(Parsed::Reconnect));
        assert!(matches!(
            parse(":tmi.twitch.tv NOTICE * :Login authentication failed"),
            Some(Parsed::Failed(_))
        ));
    }

    #[test]
    fn ignores_everything_else() {
        // Chat, room state and the welcome banter come down the same pipe.
        for line in [
            ":tmi.twitch.tv 372 justinfan838 :You are in a maze of twisty passages, all alike.",
            ":foo!foo@foo.tmi.twitch.tv PRIVMSG #bar :hello",
            "@emote-only=0 :tmi.twitch.tv ROOMSTATE #bar",
            "",
            "not an irc line",
        ] {
            assert_eq!(parse(line), None, "{line}");
        }
    }
}
