//! Both transports deliver the same protocol.

use std::time::Duration;

use liveside_client::{Connection, socket};
use liveside_core::demo::{self, DemoConfig};
use liveside_core::{EngineConfig, EngineHandle};
use liveside_proto::{Command, Event, ServerMessage};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::time::timeout;

/// The local chat log is off: a test must not write into the user's own
/// history.
fn engine() -> EngineHandle {
    let demo = DemoConfig {
        chatter_interval: None,
        ..DemoConfig::default()
    };
    let config = EngineConfig {
        history: liveside_core::history::HistoryConfig::disabled(),
        ..EngineConfig::default()
    };
    liveside_core::start(config, demo::spawn(demo)).0
}

async fn next(conn: &mut Connection) -> ServerMessage {
    timeout(Duration::from_secs(5), conn.messages.recv())
        .await
        .expect("timed out")
        .expect("connection closed")
}

/// Handshake, then join and check the response follows the join event.
async fn exercise(mut conn: Connection) {
    assert!(matches!(
        next(&mut conn).await,
        ServerMessage::Welcome { .. }
    ));
    assert!(matches!(
        next(&mut conn).await,
        ServerMessage::Snapshot { .. }
    ));

    let id = conn
        .client
        .send(Command::Join {
            channel: "abc".into(),
        })
        .await
        .unwrap();
    // The response comes after the events the command caused.
    let mut joined = false;
    loop {
        match next(&mut conn).await {
            ServerMessage::Response { id: got, error } => {
                assert_eq!(got, id);
                assert_eq!(error, None);
                assert!(joined, "response arrived before its events");
                break;
            }
            ServerMessage::Event {
                event: Event::ChannelJoined { channel, .. },
                ..
            } => {
                assert_eq!(channel.login.as_str(), "abc");
                joined = true;
            }
            _ => {}
        }
    }
}

#[tokio::test]
async fn in_process() {
    exercise(liveside_client::in_process(engine(), "test")).await;
}

#[tokio::test]
async fn unix_socket() {
    let dir = std::env::temp_dir().join(format!("liveside-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("daemon.sock");
    let _ = std::fs::remove_file(&path);
    let listener = tokio::net::UnixListener::bind(&path).unwrap();
    let engine = engine();
    tokio::spawn(async move {
        loop {
            let (stream, _) = listener.accept().await.unwrap();
            tokio::spawn(socket::serve(engine.clone(), "test".into(), stream));
        }
    });

    // Two frontends at once see the same engine.
    let a = liveside_client::connect(&path, "a").await.unwrap();
    exercise(a).await;
    let mut b = liveside_client::connect(&path, "b").await.unwrap();
    assert!(matches!(next(&mut b).await, ServerMessage::Welcome { .. }));
    match next(&mut b).await {
        ServerMessage::Snapshot { snapshot, .. } => assert_eq!(snapshot.channels.len(), 1),
        other => panic!("unexpected {other:?}"),
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn raw_json_lines() {
    // What a human with socat sees.
    let (ours, theirs) = tokio::io::duplex(64 * 1024);
    tokio::spawn(socket::serve(engine(), "test".into(), theirs));
    let (read, mut write) = tokio::io::split(ours);
    let mut lines = BufReader::new(read).lines();

    write
        .write_all(b"{\"type\":\"hello\",\"protocol\":1,\"client\":\"socat\"}\n")
        .await
        .unwrap();
    let welcome = lines.next_line().await.unwrap().unwrap();
    assert!(
        welcome.starts_with(r#"{"type":"welcome","protocol":1,"#),
        "{welcome}"
    );
    let snapshot = lines.next_line().await.unwrap().unwrap();
    assert!(
        snapshot.starts_with(r#"{"type":"snapshot","seq":"#),
        "{snapshot}"
    );

    // Garbage closes the connection.
    write.write_all(b"not json\n").await.unwrap();
    let end = timeout(Duration::from_secs(5), lines.next_line())
        .await
        .unwrap();
    assert!(matches!(end, Ok(None)), "{end:?}");
}
