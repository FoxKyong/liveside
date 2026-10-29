//! Wire format tests: every message survives a JSON round trip, and the JSON
//! shape stays readable (it is what `socat` shows when debugging the daemon).

use liveside_proto::model::*;
use liveside_proto::*;
use serde_json::{Value, json};

fn user(id: &str, login: &str) -> User {
    User {
        id: id.into(),
        login: login.into(),
        display_name: login.to_uppercase(),
    }
}

fn channel() -> Channel {
    Channel {
        id: "1001".into(),
        login: "somechannel".into(),
        display_name: "SomeChannel".into(),
    }
}

fn message() -> ChatMessage {
    ChatMessage {
        id: "b0a3c1e2-0000-4000-8000-000000000001".into(),
        sender: user("42", "viewer"),
        color: Rgb::parse_hex("#1e90ff"),
        badges: vec![Badge {
            image: "https://static-cdn.jtvnw.net/badges/v1/abc/2".into(),
            title: "Subscriber".into(),
            set_id: "subscriber".into(),
            id: "12".into(),
            info: "14".into(),
        }],
        fragments: vec![
            Fragment::Text { text: "hi ".into() },
            Fragment::Mention {
                text: "@streamer".into(),
                user: user("1001", "somechannel"),
            },
            Fragment::Text { text: " ".into() },
            Fragment::Emote {
                emote: EmoteRef {
                    provider: EmoteProvider::SevenTv,
                    id: "01F6ME9FRG0005TFYTWP1H8R42".into(),
                    code: "catJAM".into(),
                    animated: true,
                    zero_width: false,
                },
            },
        ],
        kind: MessageKind::Text,
        bits: None,
        reply: Some(Box::new(ReplyTo {
            parent_id: "p1".into(),
            parent_user: user("7", "other"),
            parent_text: "question?".into(),
            thread_id: "p1".into(),
        })),
        shared_source: None,
    }
}

fn line() -> Line {
    Line {
        time: Timestamp(1_790_000_000_000),
        content: LineContent::Message(message()),
        flags: LineFlags {
            highlighted: true,
            ..Default::default()
        },
    }
}

/// Serializes, checks the round trip, returns the JSON for shape assertions.
fn roundtrip<T>(value: &T) -> Value
where
    T: serde::Serialize + serde::de::DeserializeOwned + PartialEq + std::fmt::Debug,
{
    let text = serde_json::to_string(value).unwrap();
    let back: T = serde_json::from_str(&text).unwrap_or_else(|e| panic!("{e}: {text}"));
    assert_eq!(&back, value);
    serde_json::from_str(&text).unwrap()
}

#[test]
fn plain_text_joins_fragments() {
    assert_eq!(message().plain_text(), "hi @streamer catJAM");
}

#[test]
fn client_messages_roundtrip() {
    let hello = ClientMessage::Hello {
        protocol: PROTOCOL_VERSION,
        client: "liveside-tui 0.1.0".into(),
    };
    assert_eq!(
        roundtrip(&hello),
        json!({"type": "hello", "protocol": 1, "client": "liveside-tui 0.1.0"})
    );

    let commands = [
        Command::Login,
        Command::Logout,
        Command::Join {
            channel: "somechannel".into(),
        },
        Command::Part {
            channel: "1001".into(),
        },
        Command::Input {
            channel: "1001".into(),
            text: "/timeout viewer 600".into(),
            reply_to: None,
        },
        Command::SendMessage {
            channel: "1001".into(),
            text: "hello".into(),
            reply_to: Some("p1".into()),
        },
        Command::DeleteMessage {
            channel: "1001".into(),
            message: "m1".into(),
        },
        Command::Timeout {
            channel: "1001".into(),
            user: "42".into(),
            seconds: 600,
            reason: Some("spam".into()),
        },
        Command::Ban {
            channel: "1001".into(),
            user: "42".into(),
            reason: None,
        },
        Command::Unban {
            channel: "1001".into(),
            user: "42".into(),
        },
        Command::Shutdown,
    ];
    for (i, command) in commands.into_iter().enumerate() {
        roundtrip(&ClientMessage::Request {
            id: RequestId(i as u64),
            command,
        });
    }

    let join = ClientMessage::Request {
        id: RequestId(7),
        command: Command::Join {
            channel: "somechannel".into(),
        },
    };
    assert_eq!(
        roundtrip(&join),
        json!({"type": "request", "id": 7, "command": "join", "channel": "somechannel"})
    );
}

#[test]
fn server_messages_roundtrip() {
    let account = Account {
        user: user("5", "me"),
        scopes: vec!["user:read:chat".into(), "user:write:chat".into()],
    };
    let messages = [
        ServerMessage::Welcome {
            protocol: PROTOCOL_VERSION,
            server: "liveside-daemon 0.1.0".into(),
        },
        ServerMessage::Rejected {
            reason: "protocol mismatch".into(),
        },
        ServerMessage::Snapshot {
            seq: Seq(10),
            snapshot: Snapshot {
                account: Some(account.clone()),
                backend: "twitch".into(),
                // A whisper thread travels with the snapshot, so a
                // frontend attaching sees what was said.
                whispers: vec![liveside_proto::WhisperThread {
                    user: user("7", "alice"),
                    lines: vec![line()],
                }],
                // And what is known about links, with every field a
                // report can have.
                link_reports: vec![
                    liveside_proto::model::LinkReport {
                        url: "http://github.com/".into(),
                        source: liveside_proto::model::LinkSource::Redirects,
                        state: liveside_proto::model::LinkState::Leads {
                            final_url: Some("https://github.com/".into()),
                            redirects: vec![
                                "http://github.com/".into(),
                                "https://github.com/".into(),
                            ],
                            status: 200,
                        },
                    },
                    liveside_proto::model::LinkReport {
                        url: "http://github.com/".into(),
                        source: liveside_proto::model::LinkSource::VirusTotal,
                        state: liveside_proto::model::LinkState::Found {
                            malicious: 1,
                            suspicious: 0,
                            engines: 93,
                            analysed: Some(liveside_proto::Timestamp(1_791_915_746_000)),
                            final_url: Some("https://github.com/".into()),
                            redirects: vec!["http://github.com/".into()],
                        },
                    },
                ],
                // What lit up and what was hidden, with where.
                highlighted: vec![liveside_proto::model::CollectedLine {
                    channel: channel(),
                    line: line(),
                }],
                ignored: vec![liveside_proto::model::CollectedLine {
                    channel: channel(),
                    line: line(),
                }],
                // A bot, how it ended, and what it said.
                bots: vec![liveside_proto::model::BotStatus {
                    config: liveside_proto::model::BotConfig {
                        name: "weather".into(),
                        command: vec!["python3".into(), "weather.py".into()],
                        dir: String::new(),
                        channels: vec!["abc".into()],
                        autostart: true,
                        ..Default::default()
                    },
                    state: liveside_proto::model::BotState::Ended { code: Some(1) },
                    log: vec![liveside_proto::model::BotLogLine {
                        time: liveside_proto::Timestamp(1_791_921_746_000),
                        text: "Ended with exit code 1".into(),
                        code: "bot.ended".into(),
                        args: vec![("code".into(), "1".into())],
                    }],
                }],
                connection: ConnectionState::Reconnecting {
                    attempt: 2,
                    reason: "keepalive timeout".into(),
                },
                channels: vec![ChannelState {
                    channel: channel(),
                    modes: RoomModes {
                        slow: Some(30),
                        ..Default::default()
                    },
                    lines: vec![line()],
                    chatters: Vec::new(),
                    live: true,
                    shield: true,
                    stats: Some(liveside_proto::model::StreamStats {
                        viewers: 1234,
                        started: liveside_proto::Timestamp(1_791_921_746_000),
                    }),
                    // Minute by minute, with a change of title between.
                    viewers: liveside_proto::model::ViewerHistory {
                        started: Some(liveside_proto::Timestamp(1_791_921_746_000)),
                        samples: vec![liveside_proto::model::ViewerSample {
                            time: liveside_proto::Timestamp(1_791_921_806_000),
                            viewers: 1234,
                        }],
                        marks: vec![liveside_proto::model::StreamMark {
                            time: liveside_proto::Timestamp(1_791_921_866_000),
                            title: "Fixing the graph".into(),
                            game: "Software and Game Development".into(),
                        }],
                    },
                    held: Vec::new(),
                    unban_requests: Vec::new(),
                    // A poll and a prediction mid-flight: a frontend
                    // attaching now must see what everyone else sees.
                    poll: Some(liveside_proto::model::Poll {
                        id: "poll1".into(),
                        title: "What next?".into(),
                        choices: vec![liveside_proto::model::PollChoice {
                            id: "c0".into(),
                            title: "This".into(),
                            votes: 7,
                        }],
                        status: liveside_proto::model::PollStatus::Active,
                        ends_at: "2026-10-04T21:00:00Z".into(),
                    }),
                    redemptions: vec![liveside_proto::model::Redemption {
                        id: "red1".into(),
                        reward_id: "rew1".into(),
                        reward_title: "Pick the next game".into(),
                        cost: 5000,
                        user: user("7", "alice"),
                        text: "Factorio".into(),
                        status: liveside_proto::model::RedemptionStatus::Unfulfilled,
                        time: Timestamp(1_790_000_000_000),
                    }],
                    prediction: Some(liveside_proto::model::Prediction {
                        id: "pred1".into(),
                        title: "Will I?".into(),
                        outcomes: vec![liveside_proto::model::PredictionOutcome {
                            id: "o0".into(),
                            title: "Yes".into(),
                            users: 3,
                            points: 1200,
                        }],
                        status: liveside_proto::model::PredictionStatus::Locked,
                        locks_at: "2026-10-04T21:05:00Z".into(),
                        winner: None,
                    }),
                }],
                data_flows: vec![DataFlow {
                    party: "Twitch".into(),
                    party_code: String::new(),
                    kind: DataFlowKind::Network,
                    detail: "Chat is read and sent through Twitch".into(),
                    code: "flow.twitch.api".into(),
                    args: Vec::new(),
                    enabled: true,
                    switch: String::new(),
                }],
            },
        },
        ServerMessage::Event {
            seq: Seq(11),
            event: Event::DataFlows {
                flows: vec![DataFlow {
                    party: "7TV".into(),
                    party_code: String::new(),
                    kind: DataFlowKind::Network,
                    detail: "Emotes".into(),
                    code: "flow.third_party".into(),
                    args: vec![("what".into(), "emotes".into())],
                    enabled: false,
                    switch: "--no-third-party-emotes".into(),
                }],
            },
        },
        ServerMessage::Event {
            seq: Seq(12),
            event: Event::Backend {
                name: "demo".into(),
            },
        },
        ServerMessage::response(RequestId(3), Ok(())),
        ServerMessage::response(
            RequestId(4),
            Err(CommandError::new(
                ErrorKind::MissingScope,
                "twitch.missing_scope",
                &[("detail", "moderator:manage:banned_users required")],
                "moderator:manage:banned_users required",
            )),
        ),
    ];
    for m in &messages {
        roundtrip(m);
    }

    let events = [
        Event::LoginPending {
            uri: "https://www.twitch.tv/activate".into(),
            code: "ABCDEFGH".into(),
            expires_in_secs: 1800,
        },
        Event::LoggedIn { account },
        Event::LoginFailed {
            reason: "expired".into(),
        },
        Event::LoggedOut,
        Event::Connection {
            state: ConnectionState::Connected,
        },
        Event::ChannelJoined {
            channel: channel(),
            modes: RoomModes::default(),
        },
        Event::ChannelParted {
            channel: "1001".into(),
        },
        Event::RoomModesChanged {
            channel: "1001".into(),
            modes: RoomModes {
                followers_only: Some(0),
                ..Default::default()
            },
        },
        Event::Line {
            channel: "1001".into(),
            line: line(),
        },
        Event::Line {
            channel: "1001".into(),
            line: Line {
                time: Timestamp(0),
                content: LineContent::UserNotice {
                    system_text: "viewer subscribed for 14 months".into(),
                    message: Some(message()),
                },
                flags: LineFlags::default(),
            },
        },
        Event::History {
            channel: "1001".into(),
            lines: vec![line(), line()],
        },
        Event::MessageDeleted {
            channel: "1001".into(),
            message: "m1".into(),
        },
        Event::UserCleared {
            channel: "1001".into(),
            user: "42".into(),
            seconds: Some(600),
        },
        Event::ChatCleared {
            channel: "1001".into(),
        },
    ];
    for (i, event) in events.into_iter().enumerate() {
        roundtrip(&ServerMessage::Event {
            seq: Seq(i as u64),
            event,
        });
    }
}

#[test]
fn response_json_shape() {
    assert_eq!(
        roundtrip(&ServerMessage::response(RequestId(3), Ok(()))),
        json!({"type": "response", "id": 3})
    );
    // Twitch's own words travel without a code, so the shape stays small.
    let error = CommandError::passthrough(ErrorKind::NotJoined, "Not in channel 1");
    assert_eq!(
        roundtrip(&ServerMessage::response(RequestId(4), Err(error))),
        json!({"type": "response", "id": 4, "error": {"kind": "not_joined", "message": "Not in channel 1"}})
    );

    // One the frontend can translate carries its code and arguments.
    let error = CommandError::new(
        ErrorKind::NotJoined,
        "channel.not_joined",
        &[("channel", "1")],
        "Not in channel 1",
    );
    assert_eq!(
        roundtrip(&ServerMessage::response(RequestId(5), Err(error))),
        json!({
            "type": "response",
            "id": 5,
            "error": {
                "kind": "not_joined",
                "message": "Not in channel 1",
                "code": "channel.not_joined",
                "args": [["channel", "1"]]
            }
        })
    );
}

#[test]
fn event_json_shape() {
    let msg = ServerMessage::Event {
        seq: Seq(11),
        event: Event::Line {
            channel: "1001".into(),
            line: Line {
                time: Timestamp(5),
                content: LineContent::Notice {
                    level: NoticeLevel::Info,
                    text: "Joined".into(),
                    code: String::new(),
                    args: Vec::new(),
                },
                flags: LineFlags::default(),
            },
        },
    };
    let v = roundtrip(&msg);
    assert_eq!(v["type"], "event");
    assert_eq!(v["seq"], 11);
    assert_eq!(v["event"], "line");
    assert_eq!(v["channel"], "1001");
    assert_eq!(v["line"]["content"]["type"], "notice");
    assert_eq!(v["line"]["content"]["level"], "info");

    let emote = roundtrip(&message())["fragments"][3].clone();
    assert_eq!(
        emote,
        json!({
            "type": "emote",
            "provider": "seven_tv",
            "id": "01F6ME9FRG0005TFYTWP1H8R42",
            "code": "catJAM",
            "animated": true,
            "zero_width": false
        })
    );
}

#[test]
fn unknown_enum_values_are_tolerated() {
    // Twitch adds message types over time; old frontends must not choke.
    let kind: MessageKind = serde_json::from_str(r#""some_future_type""#).unwrap();
    assert_eq!(kind, MessageKind::Unknown);
    let kind: ErrorKind = serde_json::from_str(r#""quota""#).unwrap();
    assert_eq!(kind, ErrorKind::Unknown);
}
