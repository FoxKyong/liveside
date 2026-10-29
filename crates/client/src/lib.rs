//! Connections between frontends and the engine.
//!
//! A frontend gets a [`Connection`]: a [`Client`] to send commands and a
//! stream of [`ServerMessage`]s. It looks the same whether the engine runs in
//! the same process ([`in_process`]) or in `liveside-daemon` ([`connect`]), so
//! frontends are written once for both modes.
//!
//! The daemon side of the socket protocol lives here too
//! ([`socket::serve`]), so both ends always agree on framing.

pub mod socket;

use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

use liveside_core::EngineHandle;
use liveside_proto::{ClientMessage, Command, PROTOCOL_VERSION, RequestId, ServerMessage};
use tokio::sync::mpsc;

pub use socket::default_socket_path;

/// Messages buffered in each direction before the sender waits.
const QUEUE: usize = 1024;

/// The connection to the engine is gone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Disconnected;

impl std::fmt::Display for Disconnected {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("disconnected from engine")
    }
}

impl std::error::Error for Disconnected {}

/// Sends commands to the engine. Answers arrive on the connection's message
/// stream as [`ServerMessage::Response`] with the returned request id.
#[derive(Debug)]
pub struct Client {
    outgoing: mpsc::Sender<ClientMessage>,
    next_id: AtomicU64,
}

impl Client {
    fn new(outgoing: mpsc::Sender<ClientMessage>) -> Self {
        Self {
            outgoing,
            next_id: AtomicU64::new(1),
        }
    }

    pub async fn send(&self, command: Command) -> Result<RequestId, Disconnected> {
        let id = self.next_id();
        self.send_with_id(id, command).await?;
        Ok(id)
    }

    /// The next request id, for a frontend that has to know what it is
    /// about to ask before the asking is done: the window notes the id
    /// to know when the answer it is waiting for has come.
    pub fn next_id(&self) -> RequestId {
        RequestId(self.next_id.fetch_add(1, Ordering::Relaxed))
    }

    pub async fn send_with_id(&self, id: RequestId, command: Command) -> Result<(), Disconnected> {
        self.outgoing
            .send(ClientMessage::Request { id, command })
            .await
            .map_err(|_| Disconnected)
    }
}

/// A frontend's connection. The first messages are `Welcome` (or
/// `Rejected`) and `Snapshot`. A `Snapshot` may arrive again at any time; it
/// then replaces all state the frontend holds.
#[derive(Debug)]
pub struct Connection {
    pub client: Client,
    pub messages: mpsc::Receiver<ServerMessage>,
}

/// Connects to an engine running in this process. `name` identifies the
/// frontend in logs, e.g. `"liveside-tui 0.1.0"`.
pub fn in_process(engine: EngineHandle, name: &str) -> Connection {
    let (to_engine, incoming) = mpsc::channel(QUEUE);
    let (outgoing, messages) = mpsc::channel(QUEUE);
    let server = format!("liveside-core {} (in-process)", env!("CARGO_PKG_VERSION"));
    tokio::spawn(liveside_core::session::run(
        engine, server, incoming, outgoing,
    ));
    hello(&to_engine, name);
    Connection {
        client: Client::new(to_engine),
        messages,
    }
}

/// Starts an engine with the demo backend in this process and connects to
/// it. Lets frontends run without a daemon, a network or a Twitch account.
///
/// `membership` invents who is in the channel, as the `irc` user list
/// would report it, so that setting can be tried out here too.
pub fn in_process_demo(name: &str, config: EngineConfig, membership: bool) -> Connection {
    let engine = liveside_core::start(
        config,
        liveside_core::demo::spawn(liveside_core::demo::DemoConfig {
            membership,
            ..liveside_core::demo::DemoConfig::default()
        }),
    )
    .0;
    in_process(engine, name)
}

pub mod addressbook;
pub mod bots;
pub mod chatters;
pub mod collected;
pub mod custom;
pub mod days;
pub mod followed;
pub mod input;

/// Re-exported so frontends can configure the Twitch backend without
/// depending on the engine crate directly.
pub mod lang;
pub mod layout;
pub mod notify;
pub mod settings;
pub mod streamlink;

pub use liveside_core::EngineConfig;
/// The rule language, for checking a rule as it is typed.
pub use liveside_core::commands;
pub use liveside_core::config::{self, Config};
pub use liveside_core::history::HistoryConfig;
pub use liveside_core::hotkeys;
/// Channel commands the engine understands, for completion in a frontend.
pub use liveside_core::input::COMMANDS;
pub use liveside_core::open_channels;
pub use liveside_core::rules;
pub use liveside_core::twitch::TwitchConfig;

/// Starts an engine that talks to Twitch in this process and connects to it.
pub fn in_process_twitch(name: &str, engine: EngineConfig, twitch: TwitchConfig) -> Connection {
    let engine = liveside_core::start(engine, liveside_core::twitch::spawn(twitch)).0;
    in_process(engine, name)
}

/// Connects to a running daemon.
pub async fn connect(path: &Path, name: &str) -> std::io::Result<Connection> {
    let stream = tokio::net::UnixStream::connect(path).await?;
    let (to_engine, messages) = socket::client(stream);
    hello(&to_engine, name);
    Ok(Connection {
        client: Client::new(to_engine),
        messages,
    })
}

fn hello(to_engine: &mpsc::Sender<ClientMessage>, name: &str) {
    // The queue is fresh and empty, so this cannot fail for lack of space.
    let _ = to_engine.try_send(ClientMessage::Hello {
        protocol: PROTOCOL_VERSION,
        client: name.to_owned(),
    });
}

/// Every command a line can start with in a frontend, for completion: the
/// engine's, the frontend's own (`own`), and the custom ones, sorted and
/// each once.
pub fn command_names(own: &[&str], custom: &commands::CustomCommands) -> Vec<String> {
    let mut names: Vec<String> = COMMANDS
        .iter()
        .chain(own)
        .map(|name| (*name).to_owned())
        .chain(custom.names().into_iter().map(str::to_owned))
        .collect();
    names.sort();
    names.dedup();
    names
}

#[cfg(test)]
mod command_name_tests {
    #[test]
    fn the_engines_the_frontends_and_the_custom_ones_once_each() {
        let custom =
            super::commands::CustomCommands::read(&["/slap x".to_owned(), "/join y".to_owned()])
                .unwrap();
        let names = super::command_names(&["viewers", "join"], &custom);
        for name in ["join", "ban", "viewers", "slap"] {
            assert_eq!(names.iter().filter(|n| *n == name).count(), 1, "{name}");
        }
        assert!(names.windows(2).all(|w| w[0] < w[1]), "sorted");
    }
}
