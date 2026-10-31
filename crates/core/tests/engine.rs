//! Engine and session behaviour, driven through the demo backend.

use std::time::Duration;

use liveside_core::demo::{self, DemoConfig};
use liveside_core::{EngineConfig, EngineHandle, session};
use liveside_proto::model::LineContent;
use liveside_proto::{
    ClientMessage, Command, ErrorKind, Event, PROTOCOL_VERSION, RequestId, Seq, ServerMessage,
    UserId,
};
use tokio::sync::mpsc;
use tokio::time::timeout;

const QUIET: DemoConfig = DemoConfig {
    membership: false,
    chatter_interval: None,
};

/// Starts an engine for a test. The local chat log is off: tests must not
/// write into the user's own history, nor read it back.
fn start(config: EngineConfig, demo: DemoConfig) -> EngineHandle {
    let config = EngineConfig {
        history: liveside_core::history::HistoryConfig::disabled(),
        ..config
    };
    liveside_core::start(config, demo::spawn(demo)).0
}

async fn join(engine: &EngineHandle, login: &str) -> UserId {
    engine
        .run(Command::Join {
            channel: login.into(),
        })
        .await
        .unwrap()
        .unwrap();
    // Read back from a snapshot rather than from the event stream: a busy
    // demo can push the join out of a small buffer before this gets to it,
    // and a test that waited for an event it will never see would hang
    // rather than fail.
    let wanted = login.trim_start_matches('#').to_ascii_lowercase();
    let snapshot = wait_for(engine, |s| {
        s.channels
            .iter()
            .any(|c| c.channel.login.as_str() == wanted)
    })
    .await;
    snapshot
        .channels
        .into_iter()
        .find(|c| c.channel.login.as_str() == wanted)
        .expect("the channel that was just joined")
        .channel
        .id
}

/// Waits until the snapshot satisfies `f`. Events are applied asynchronously
/// after a command's result, so tests poll instead of assuming timing.
async fn wait_for(
    engine: &EngineHandle,
    f: impl Fn(&liveside_proto::Snapshot) -> bool,
) -> liveside_proto::Snapshot {
    for _ in 0..500 {
        let snapshot = engine.attach().await.unwrap().snapshot;
        if f(&snapshot) {
            return snapshot;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("condition not reached");
}

#[tokio::test]
async fn join_builds_channel_state() {
    let engine = start(EngineConfig::default(), QUIET);
    let id = join(&engine, "#SomeChannel").await;

    let snapshot = wait_for(&engine, |s| {
        s.channels.first().is_some_and(|c| c.lines.len() == 4)
    })
    .await;
    let channel = &snapshot.channels[0];
    assert_eq!(channel.channel.id, id);
    assert_eq!(channel.channel.login.as_str(), "somechannel");
    // Three history lines before the join notice.
    assert!(channel.lines[..3].iter().all(|l| l.flags.historical));
    assert!(
        matches!(&channel.lines[3].content, LineContent::Notice { text, .. } if text.contains("#somechannel"))
    );
}

#[tokio::test]
async fn attach_is_consistent_with_event_stream() {
    let engine = start(EngineConfig::default(), QUIET);
    let a = engine.attach().await.unwrap();
    let mut events = a.events;
    join(&engine, "abc").await;
    // Events after an attachment continue its sequence without gaps.
    for expected in (a.seq.0 + 1..).take(3) {
        let s = events.recv().await.unwrap();
        assert_eq!(s.seq, Seq(expected));
    }
}

#[tokio::test]
async fn input_and_errors() {
    let engine = start(EngineConfig::default(), QUIET);
    let id = join(&engine, "abc").await;

    let input = |text: &str| Command::Input {
        channel: id.clone(),
        text: text.into(),
        reply_to: None,
    };

    let err = engine.run(input("hello")).await.unwrap().unwrap_err();
    assert_eq!(err.kind, ErrorKind::NotLoggedIn);

    engine.run(Command::Login).await.unwrap().unwrap();
    engine.run(input("hello Kappa")).await.unwrap().unwrap();
    let snapshot = wait_for(&engine, |s| s.channels[0].lines.iter().any(|l| l.flags.own)).await;
    assert_eq!(snapshot.account.unwrap().user.login.as_str(), "demo_user");

    let err = engine.run(input("/nope")).await.unwrap().unwrap_err();
    assert_eq!(err.kind, ErrorKind::InvalidArgument);

    let err = engine
        .run(Command::Part {
            channel: "999".into(),
        })
        .await
        .unwrap()
        .unwrap_err();
    assert_eq!(err.kind, ErrorKind::NotJoined);

    engine.run(input("/part")).await.unwrap().unwrap();
    wait_for(&engine, |s| s.channels.is_empty()).await;
}

#[tokio::test]
async fn timeout_marks_messages_deleted() {
    let engine = start(EngineConfig::default(), QUIET);
    let id = join(&engine, "abc").await;
    engine.run(Command::Login).await.unwrap().unwrap();
    let send = Command::Input {
        channel: id.clone(),
        text: "my message".into(),
        reply_to: None,
    };
    engine.run(send).await.unwrap().unwrap();
    wait_for(&engine, |s| s.channels[0].lines.iter().any(|l| l.flags.own)).await;

    let timeout_me = Command::Input {
        channel: id,
        text: "/timeout demo_user 1m".into(),
        reply_to: None,
    };
    engine.run(timeout_me).await.unwrap().unwrap();
    wait_for(&engine, |s| {
        s.channels[0]
            .lines
            .iter()
            .filter(|l| l.flags.own)
            .all(|l| l.flags.deleted)
    })
    .await;
}

#[tokio::test]
async fn scrollback_is_bounded() {
    let config = EngineConfig {
        scrollback: 10,
        ..Default::default()
    };
    let demo = DemoConfig {
        chatter_interval: Some(Duration::from_millis(1)),
        ..QUIET
    };
    let engine = start(config, demo);
    join(&engine, "busy").await;
    // Wait until the generator produced more than the limit.
    tokio::time::sleep(Duration::from_millis(100)).await;
    let snapshot = engine.attach().await.unwrap().snapshot;
    assert_eq!(snapshot.channels[0].lines.len(), 10);
}

#[tokio::test]
async fn shutdown_stops_engine() {
    let (engine, task) = liveside_core::start(EngineConfig::default(), demo::spawn(QUIET));
    engine.run(Command::Shutdown).await.unwrap().unwrap();
    timeout(Duration::from_secs(5), task)
        .await
        .unwrap()
        .unwrap();
    assert!(engine.attach().await.is_err());
}

#[tokio::test]
async fn a_bot_hears_chat_and_speaks_as_the_account() {
    let locks = std::env::temp_dir().join(format!("liveside-engine-bots-{}", std::process::id()));
    // It answers every message it hears with "heard", and would say so if
    // it ever heard one of its own answers.
    let script = r#"read hello
        while read line; do
          case "$line" in
            *'"text":"heard"'*) echo LOOP >&2 ;;
            *'"text":"typed by hand"'*) echo TYPED >&2 ;;
            *) echo '{"type":"say","channel":"botland","text":"heard"}' ;;
          esac
        done"#;
    let bot = liveside_proto::model::BotConfig {
        name: "echo".into(),
        command: vec!["sh".into(), "-c".into(), script.into()],
        dir: std::env::temp_dir().display().to_string(),
        channels: vec!["botland".into()],
        autostart: true,
        ..Default::default()
    };
    let config = EngineConfig {
        bots: vec![bot],
        bot_locks: locks.clone(),
        ..Default::default()
    };
    let demo = DemoConfig {
        chatter_interval: Some(Duration::from_millis(20)),
        ..QUIET
    };
    let engine = start(config, demo);
    engine.run(Command::Login).await.unwrap().unwrap();
    join(&engine, "botland").await;
    let snapshot = wait_for(&engine, |s| {
        let me = s.account.as_ref().map(|a| a.user.id.clone());
        s.channels[0].lines.iter().any(|line| {
            matches!(&line.content, LineContent::Message(m)
                if Some(&m.sender.id) == me.as_ref() && m.plain_text() == "heard")
        })
    })
    .await;
    assert_eq!(
        snapshot.bots[0].state,
        liveside_proto::model::BotState::Running
    );
    tokio::time::sleep(Duration::from_millis(300)).await;
    let snapshot = engine.attach().await.unwrap().snapshot;
    assert!(
        !snapshot.bots[0].log.iter().any(|line| line.text == "LOOP"),
        "a bot heard its own line come back"
    );
    // What the user types as the account, it hears.
    let channel = snapshot.channels[0].channel.id.clone();
    engine
        .run(Command::SendMessage {
            channel,
            text: "typed by hand".into(),
            reply_to: None,
        })
        .await
        .unwrap()
        .unwrap();
    wait_for(&engine, |s| {
        s.bots[0].log.iter().any(|line| line.text == "TYPED")
    })
    .await;
    engine
        .run(Command::StopBot {
            name: "echo".into(),
        })
        .await
        .unwrap()
        .unwrap();
    wait_for(&engine, |s| {
        s.bots[0].state == liveside_proto::model::BotState::Stopped
    })
    .await;
    let _ = std::fs::remove_dir_all(&locks);
}

// --- session ---

async fn open_session(
    engine: &EngineHandle,
    protocol: u32,
) -> (mpsc::Sender<ClientMessage>, mpsc::Receiver<ServerMessage>) {
    let (to_engine, incoming) = mpsc::channel(16);
    let (outgoing, from_engine) = mpsc::channel(16);
    tokio::spawn(session::run(
        engine.clone(),
        "test".into(),
        incoming,
        outgoing,
    ));
    to_engine
        .send(ClientMessage::Hello {
            protocol,
            client: "test".into(),
        })
        .await
        .unwrap();
    (to_engine, from_engine)
}

async fn next(rx: &mut mpsc::Receiver<ServerMessage>) -> Option<ServerMessage> {
    timeout(Duration::from_secs(5), rx.recv()).await.unwrap()
}

#[tokio::test]
async fn session_handshake_request_and_events() {
    let engine = start(EngineConfig::default(), QUIET);
    let (tx, mut rx) = open_session(&engine, PROTOCOL_VERSION).await;

    assert!(
        matches!(next(&mut rx).await, Some(ServerMessage::Welcome { protocol, .. }) if protocol == PROTOCOL_VERSION)
    );
    assert!(matches!(
        next(&mut rx).await,
        Some(ServerMessage::Snapshot { .. })
    ));

    tx.send(ClientMessage::Request {
        id: RequestId(1),
        command: Command::Join {
            channel: "abc".into(),
        },
    })
    .await
    .unwrap();

    // The response must come after the events the join caused.
    let mut got_joined = false;
    loop {
        match next(&mut rx).await.unwrap() {
            ServerMessage::Response { id, error } => {
                assert_eq!(id, RequestId(1));
                assert_eq!(error, None);
                assert!(got_joined, "response arrived before its events");
                break;
            }
            ServerMessage::Event {
                event: Event::ChannelJoined { .. },
                ..
            } => got_joined = true,
            _ => {}
        }
    }
}

#[tokio::test]
async fn session_rejects_wrong_protocol() {
    let engine = start(EngineConfig::default(), QUIET);
    let (_tx, mut rx) = open_session(&engine, PROTOCOL_VERSION + 1).await;
    assert!(matches!(
        next(&mut rx).await,
        Some(ServerMessage::Rejected { .. })
    ));
    assert!(next(&mut rx).await.is_none());
}

#[tokio::test]
async fn slow_frontend_gets_resynced() {
    // Tiny event buffer and a frontend that does not read for a while.
    let config = EngineConfig {
        event_buffer: 4,
        ..Default::default()
    };
    let demo = DemoConfig {
        chatter_interval: Some(Duration::from_millis(1)),
        ..QUIET
    };
    let engine = start(config, demo);
    let (_tx, mut rx) = open_session(&engine, PROTOCOL_VERSION).await;
    assert!(matches!(
        next(&mut rx).await,
        Some(ServerMessage::Welcome { .. })
    ));
    assert!(matches!(
        next(&mut rx).await,
        Some(ServerMessage::Snapshot { .. })
    ));

    join(&engine, "busy").await;
    tokio::time::sleep(Duration::from_millis(200)).await;

    // Somewhere in what follows there must be a second snapshot, and after
    // it the sequence numbers continue from the snapshot's.
    let mut resync_seq = None;
    for _ in 0..200 {
        match next(&mut rx).await.unwrap() {
            ServerMessage::Snapshot { seq, snapshot } => {
                assert_eq!(snapshot.channels.len(), 1);
                resync_seq = Some(seq);
                break;
            }
            ServerMessage::Event { .. } => {}
            other => panic!("unexpected {other:?}"),
        }
    }
    let resync_seq = resync_seq.expect("no resync snapshot");
    match next(&mut rx).await.unwrap() {
        ServerMessage::Event { seq, .. } => assert_eq!(seq.0, resync_seq.0 + 1),
        other => panic!("unexpected {other:?}"),
    }
}

/// The local log is the only source of history: what one session recorded,
/// the next one restores, and nothing is fetched from anywhere.
#[tokio::test]
async fn local_history_survives_a_restart() {
    let dir = std::env::temp_dir().join(format!("liveside-engine-history-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let config = || EngineConfig {
        history: liveside_core::history::HistoryConfig {
            dir: Some(dir.clone()),
            lines: 20,
            max_bytes: 1024 * 1024,
        },
        ..EngineConfig::default()
    };

    // First session: join and say something.
    let engine = liveside_core::start(config(), demo::spawn(QUIET)).0;
    let id = join(&engine, "#somechannel").await;
    engine.run(Command::Login).await.unwrap().unwrap();
    engine
        .run(Command::Input {
            channel: id.clone(),
            text: "remember this".into(),
            reply_to: None,
        })
        .await
        .unwrap()
        .unwrap();
    wait_for(&engine, |s| {
        s.channels.first().is_some_and(|c| {
            c.lines.iter().any(|line| match &line.content {
                LineContent::Message(m) => m.plain_text() == "remember this",
                _ => false,
            })
        })
    })
    .await;
    drop(engine);

    // Second session: the same channel comes back with what was said.
    let engine = liveside_core::start(config(), demo::spawn(QUIET)).0;
    join(&engine, "#somechannel").await;
    let snapshot = wait_for(&engine, |s| {
        s.channels.first().is_some_and(|c| {
            c.lines.iter().any(|line| match &line.content {
                LineContent::Message(m) => m.plain_text() == "remember this",
                _ => false,
            })
        })
    })
    .await;
    let restored = snapshot.channels[0]
        .lines
        .iter()
        .find(|line| matches!(&line.content, LineContent::Message(m) if m.plain_text() == "remember this"))
        .unwrap();
    assert!(restored.flags.historical, "restored lines are marked");
    // The engine reports where that history lives.
    let flow = snapshot
        .data_flows
        .iter()
        .find(|flow| flow.kind == liveside_proto::DataFlowKind::Disk)
        .expect("the local log is reported as a data flow");
    assert!(flow.enabled && flow.detail.contains(&dir.display().to_string()));

    std::fs::remove_dir_all(&dir).unwrap();
}

#[tokio::test]
async fn open_channels_are_opened_again_at_the_next_start() {
    let dir = std::env::temp_dir().join(format!("liveside-engine-reopen-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let path = dir.join("channels.json");
    let config = || EngineConfig {
        open_channels: Some(path.clone()),
        ..EngineConfig::default()
    };
    let logins = |s: &liveside_proto::Snapshot| -> Vec<String> {
        s.channels
            .iter()
            .map(|c| c.channel.login.as_str().to_owned())
            .collect()
    };

    // First run: two channels joined, one of them left again.
    let engine = start(config(), QUIET);
    join(&engine, "orbitcast").await;
    let left = join(&engine, "tidewave").await;
    engine
        .run(Command::Part { channel: left })
        .await
        .unwrap()
        .unwrap();
    wait_for(&engine, |s| logins(s) == ["orbitcast"]).await;
    drop(engine);

    // Second run: what was open at the end comes back by itself. A
    // frontend attaching at once is told it is on its way, since the
    // engine's ChannelJoining went out before anybody listened. The old
    // settings key is not read once the list exists: a channel left must
    // not come back from there.
    let engine = start(
        EngineConfig {
            channels: vec!["brightmaple".to_owned()],
            ..config()
        },
        QUIET,
    );
    let first = engine.attach().await.unwrap().snapshot;
    let mut seen: Vec<String> = first
        .joining
        .iter()
        .map(|l| l.as_str().to_owned())
        .collect();
    seen.extend(logins(&first));
    assert_eq!(seen, ["orbitcast"]);
    let snapshot = wait_for(&engine, |s| s.channels.len() == 1 && s.joining.is_empty()).await;
    assert_eq!(logins(&snapshot), ["orbitcast"]);
    drop(engine);

    // Without a list yet, the old key's channels start it.
    let fresh = dir.join("fresh.json");
    let engine = start(
        EngineConfig {
            open_channels: Some(fresh.clone()),
            channels: vec!["brightmaple".to_owned(), "orbitcast".to_owned()],
            ..EngineConfig::default()
        },
        QUIET,
    );
    let snapshot = wait_for(&engine, |s| s.channels.len() == 2).await;
    assert_eq!(logins(&snapshot), ["brightmaple", "orbitcast"]);
    assert_eq!(
        liveside_core::open_channels::load(&fresh),
        ["brightmaple", "orbitcast"]
    );
    drop(engine);
    let engine = start(config(), QUIET);
    let snapshot = wait_for(&engine, |s| s.channels.len() == 1).await;
    assert_eq!(logins(&snapshot), ["orbitcast"]);
    let flow = snapshot
        .data_flows
        .iter()
        .find(|flow| flow.code == "flow.channels.on")
        .expect("the list is reported as a data flow");
    assert!(flow.enabled && flow.detail.contains(&path.display().to_string()));
    drop(engine);

    // Switched off, nothing is opened, and nothing is written.
    let engine = start(EngineConfig::default(), QUIET);
    tokio::time::sleep(Duration::from_millis(200)).await;
    let snapshot = engine.attach().await.unwrap().snapshot;
    assert!(snapshot.channels.is_empty());
    assert!(
        snapshot
            .data_flows
            .iter()
            .any(|flow| flow.code == "flow.channels.off" && !flow.enabled)
    );

    std::fs::remove_dir_all(&dir).unwrap();
}

/// History restored on join can overlap messages that then arrive live,
/// for instance when a daemon and a frontend on the same machine both log
/// to the same file. The engine shows such a message once.
#[tokio::test]
async fn a_message_is_not_shown_twice() {
    let engine = start(EngineConfig::default(), QUIET);
    let id = join(&engine, "abc").await;
    engine.run(Command::Login).await.unwrap().unwrap();
    engine
        .run(Command::Input {
            channel: id.clone(),
            text: "only once".into(),
            reply_to: None,
        })
        .await
        .unwrap()
        .unwrap();

    let snapshot = wait_for(&engine, |s| {
        s.channels.first().is_some_and(|c| {
            c.lines.iter().any(|line| match &line.content {
                LineContent::Message(m) => m.plain_text() == "only once",
                _ => false,
            })
        })
    })
    .await;
    let sent = snapshot.channels[0]
        .lines
        .iter()
        .find_map(|line| match &line.content {
            LineContent::Message(m) if m.plain_text() == "only once" => Some(m.clone()),
            _ => None,
        })
        .unwrap();

    // The demo backend echoes whatever it is told to send, so asking it to
    // deliver the same message id again is the same situation.
    engine
        .run(Command::Input {
            channel: id.clone(),
            text: "only once".into(),
            reply_to: None,
        })
        .await
        .unwrap()
        .unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;
    let snapshot = engine.attach().await.unwrap().snapshot;
    let copies = snapshot.channels[0]
        .lines
        .iter()
        .filter(|line| matches!(&line.content, LineContent::Message(m) if m.id == sent.id))
        .count();
    assert_eq!(copies, 1, "the same message id must appear once");
}

#[tokio::test]
async fn membership_reaches_the_frontend_and_the_snapshot() {
    // What the `irc` user list gives a frontend: people who are in the
    // channel without having said anything.
    let demo = DemoConfig {
        membership: true,
        ..QUIET
    };
    let engine = start(EngineConfig::default(), demo);
    join(&engine, "quiet_channel").await;

    let snapshot = wait_for(&engine, |s| {
        s.channels.first().is_some_and(|c| !c.chatters.is_empty())
    })
    .await;
    let chatters = &snapshot.channels[0].chatters;
    assert!(
        chatters
            .iter()
            .all(|login| login.as_str().starts_with("lurker_"))
    );

    // Without the setting the engine holds nobody, and a frontend falls
    // back to whoever has talked.
    let engine = start(EngineConfig::default(), QUIET);
    join(&engine, "quiet_channel").await;
    let snapshot = wait_for(&engine, |s| !s.channels.is_empty()).await;
    assert!(snapshot.channels[0].chatters.is_empty());
}

#[tokio::test]
async fn the_demo_keeps_the_room_modes_it_is_given() {
    // Twitch reports the modes back over EventSub; the demo has to keep
    // them itself, or /slow would change nothing a reader can see and the
    // header would stay empty whatever is asked for.
    let engine = start(EngineConfig::default(), QUIET);
    let channel = join(&engine, "quiet_channel").await;
    let say = |text: &str| Command::Input {
        channel: channel.clone(),
        text: text.to_owned(),
        reply_to: None,
    };
    engine.run(say("/slow 30")).await.unwrap().unwrap();
    engine.run(say("/emoteonly")).await.unwrap().unwrap();

    let snapshot = wait_for(&engine, |s| {
        s.channels.first().is_some_and(|c| c.modes.slow.is_some())
    })
    .await;
    let modes = &snapshot.channels[0].modes;
    assert_eq!(modes.slow, Some(30));
    assert!(modes.emote_only, "emote-only was asked for: {modes:?}");
    assert!(!modes.subscribers_only, "nothing else changed: {modes:?}");
}

#[tokio::test]
async fn a_poll_counts_and_then_ends() {
    use liveside_proto::model::PollStatus;
    let engine = start(EngineConfig::default(), QUIET);
    let channel = join(&engine, "quiet_channel").await;
    engine
        .run(Command::CreatePoll {
            channel: channel.clone(),
            title: "What next?".to_owned(),
            choices: vec!["This".to_owned(), "That".to_owned()],
            seconds: 15,
        })
        .await
        .unwrap()
        .unwrap();

    // It shows up in the snapshot, so a frontend attaching mid-poll sees
    // what everyone else is looking at.
    let snapshot = wait_for(&engine, |s| {
        s.channels.first().is_some_and(|c| c.poll.is_some())
    })
    .await;
    let poll = snapshot.channels[0].poll.clone().expect("a poll");
    assert_eq!(poll.title, "What next?");
    assert_eq!(poll.choices.len(), 2);
    assert_eq!(poll.status, PollStatus::Active);

    // Stopped early, it answers with the votes it had reached, and the
    // channel keeps no poll afterwards.
    engine
        .run(Command::EndPoll {
            channel: channel.clone(),
            archive: false,
        })
        .await
        .unwrap()
        .unwrap();
    let snapshot = wait_for(&engine, |s| {
        s.channels.first().is_some_and(|c| c.poll.is_none())
    })
    .await;
    assert!(snapshot.channels[0].poll.is_none());

    // And there is nothing left to stop.
    let again = engine
        .run(Command::EndPoll {
            channel,
            archive: false,
        })
        .await
        .unwrap();
    assert!(again.is_err(), "ending a poll that is over: {again:?}");
}

#[tokio::test]
async fn a_prediction_locks_before_it_pays_out() {
    use liveside_proto::PredictionEnd;
    use liveside_proto::model::PredictionStatus;
    let engine = start(EngineConfig::default(), QUIET);
    let channel = join(&engine, "quiet_channel").await;
    engine
        .run(Command::CreatePrediction {
            channel: channel.clone(),
            title: "Will I finish it?".to_owned(),
            outcomes: vec!["Yes".to_owned(), "No".to_owned()],
            seconds: 30,
        })
        .await
        .unwrap()
        .unwrap();

    let snapshot = wait_for(&engine, |s| {
        s.channels.first().is_some_and(|c| c.prediction.is_some())
    })
    .await;
    let prediction = snapshot.channels[0].prediction.clone().expect("one");
    assert_eq!(prediction.status, PredictionStatus::Active);

    // Betting closes, and the prediction stays: this is the state the
    // broadcaster must not forget about, and Twitch cancels it after a
    // day if they do.
    engine
        .run(Command::EndPrediction {
            channel: channel.clone(),
            how: PredictionEnd::Lock,
        })
        .await
        .unwrap()
        .unwrap();
    let snapshot = wait_for(&engine, |s| {
        s.channels.first().is_some_and(|c| {
            c.prediction
                .as_ref()
                .is_some_and(|p| p.status == PredictionStatus::Locked)
        })
    })
    .await;
    let locked = snapshot.channels[0].prediction.clone().expect("one");
    assert_eq!(locked.id, prediction.id, "a different prediction");

    // Paid out, it is over and the channel keeps nothing.
    engine
        .run(Command::EndPrediction {
            channel,
            how: PredictionEnd::Resolve {
                outcome: locked.outcomes[0].id.clone(),
            },
        })
        .await
        .unwrap()
        .unwrap();
    let snapshot = wait_for(&engine, |s| {
        s.channels.first().is_some_and(|c| c.prediction.is_none())
    })
    .await;
    assert!(snapshot.channels[0].prediction.is_none());
}

#[tokio::test]
async fn a_redemption_waits_in_the_queue_until_it_is_answered() {
    // The demo spends points now and then, which is the only way a
    // redemption reaches the engine: it keeps whatever is unanswered.
    let demo = DemoConfig {
        chatter_interval: Some(Duration::from_millis(1)),
        ..QUIET
    };
    let engine = start(EngineConfig::default(), demo);
    let channel = join(&engine, "busy").await;

    let snapshot = wait_for(&engine, |s| {
        s.channels
            .first()
            .is_some_and(|c| !c.redemptions.is_empty())
    })
    .await;
    let waiting = snapshot.channels[0].redemptions[0].clone();
    assert_eq!(
        waiting.status,
        liveside_proto::model::RedemptionStatus::Unfulfilled
    );

    // Answered, it leaves the queue.
    engine
        .run(Command::AnswerRedemption {
            channel,
            reward: waiting.reward_id.clone(),
            redemption: waiting.id.clone(),
            fulfil: true,
        })
        .await
        .unwrap()
        .unwrap();
    let gone = wait_for(&engine, |s| {
        s.channels
            .first()
            .is_some_and(|c| c.redemptions.iter().all(|r| r.id != waiting.id))
    })
    .await;
    assert!(
        gone.channels[0]
            .redemptions
            .iter()
            .all(|r| r.id != waiting.id)
    );
}

#[tokio::test]
async fn what_went_wrong_is_not_written_to_the_chat_log() {
    // A night of "Invalid OAuth token" used to be logged and read back
    // into the channel at every start afterwards.
    let dir = std::env::temp_dir().join(format!("liveside-log-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let config = EngineConfig {
        history: liveside_core::history::HistoryConfig {
            dir: Some(dir.clone()),
            ..liveside_core::history::HistoryConfig::disabled()
        },
        ..Default::default()
    };
    let engine = liveside_core::start(config, demo::spawn(QUIET)).0;
    let channel = join(&engine, "quiet_channel").await;

    // One of each: something said, and something that went wrong.
    engine
        .run(Command::Input {
            channel: channel.clone(),
            text: "/slow abc".to_owned(),
            reply_to: None,
        })
        .await
        .unwrap()
        .unwrap_err();
    tokio::time::sleep(Duration::from_millis(50)).await;

    let written = std::fs::read_dir(&dir)
        .map(|entries| {
            entries
                .filter_map(|e| e.ok())
                .filter_map(|e| std::fs::read_to_string(e.path()).ok())
                .collect::<String>()
        })
        .unwrap_or_default();
    assert!(
        !written.contains("error"),
        "an error was written to the log: {written}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn bots_are_listed_started_and_stopped_by_typing() {
    let locks = std::env::temp_dir().join(format!("liveside-engine-typed-{}", std::process::id()));
    let bot = liveside_proto::model::BotConfig {
        name: "quiet".into(),
        command: vec![
            "sh".into(),
            "-c".into(),
            "echo said on stderr >&2; cat >/dev/null".into(),
        ],
        dir: std::env::temp_dir().display().to_string(),
        channels: Vec::new(),
        autostart: false,
        ..Default::default()
    };
    let config = EngineConfig {
        bots: vec![bot],
        bot_locks: locks.clone(),
        ..Default::default()
    };
    let engine = start(config, QUIET);
    let channel = join(&engine, "typing").await;
    let typed = |text: &str| Command::Input {
        channel: channel.clone(),
        text: text.into(),
        reply_to: None,
    };
    let notices = |s: &liveside_proto::Snapshot| -> Vec<String> {
        s.channels[0]
            .lines
            .iter()
            .filter_map(|line| match &line.content {
                LineContent::Notice { code, .. } if code.starts_with("bot.") => Some(code.clone()),
                _ => None,
            })
            .collect()
    };
    engine.run(typed("/bots")).await.unwrap().unwrap();
    engine
        .run(typed("/bot start quiet"))
        .await
        .unwrap()
        .unwrap();
    wait_for(&engine, |s| {
        s.bots[0]
            .log
            .iter()
            .any(|line| line.text == "said on stderr")
    })
    .await;
    engine.run(typed("/bot log quiet")).await.unwrap().unwrap();
    let error = engine
        .run(typed("/bot frobnicate"))
        .await
        .unwrap()
        .unwrap_err();
    assert_eq!(error.code, "usage.bot");
    let error = engine
        .run(typed("/bot stop nobody"))
        .await
        .unwrap()
        .unwrap_err();
    assert_eq!(error.code, "bot.unknown");
    engine.run(typed("/bot stop quiet")).await.unwrap().unwrap();
    let snapshot = wait_for(&engine, |s| {
        s.bots[0].state == liveside_proto::model::BotState::Stopped
    })
    .await;
    assert_eq!(
        notices(&snapshot),
        ["bot.is_stopped", "bot.log", "bot.started", "bot.output"]
    );
    let _ = std::fs::remove_dir_all(&locks);
}

#[tokio::test]
async fn only_a_bot_that_hears_ignored_people_hears_them() {
    let locks =
        std::env::temp_dir().join(format!("liveside-engine-ignored-{}", std::process::id()));
    // Each says on its errors who it heard.
    let script = r#"read hello; while read line; do
          echo "$line" | sed 's/.*"login":"\([^"]*\)".*/\1/' >&2
        done"#;
    let bot = |name: &str, hear_ignored: bool| liveside_proto::model::BotConfig {
        name: name.into(),
        command: vec!["sh".into(), "-c".into(), script.into()],
        dir: std::env::temp_dir().display().to_string(),
        hear_ignored,
        autostart: true,
        ..Default::default()
    };
    let config = EngineConfig {
        bots: vec![bot("deaf", false), bot("hears", true)],
        bot_locks: locks.clone(),
        rules: liveside_core::rules::Rules::read(&[], &["user:alice".into()], &[]).unwrap(),
        ..Default::default()
    };
    let demo = DemoConfig {
        chatter_interval: Some(Duration::from_millis(5)),
        ..QUIET
    };
    let engine = start(config, demo);
    join(&engine, "ignoring").await;
    let snapshot = wait_for(&engine, |s| {
        s.bots[1].log.iter().any(|line| line.text == "alice")
            && s.bots[0]
                .log
                .iter()
                .filter(|line| line.code.is_empty())
                .count()
                > 20
    })
    .await;
    assert!(
        !snapshot.bots[0].log.iter().any(|line| line.text == "alice"),
        "a bot heard somebody the ignore rules hide"
    );
    let _ = std::fs::remove_dir_all(&locks);
}
