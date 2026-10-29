//! The engine, seen from the GTK main loop.
//!
//! GTK owns the thread that draws; the engine and its network live on a
//! Tokio runtime beside it. Everything crosses between the two through two
//! channels, so no widget is ever touched from another thread.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use liveside_client::{Config, Connection};
use liveside_proto::{Command, RequestId, ServerMessage};
use tokio::sync::mpsc;

/// How long a request may go unanswered before the window says it is
/// working. Most are answered in a moment, and a wheel that appears for
/// a tenth of a second at every typed line is worse than none.
const PATIENCE: Duration = Duration::from_millis(300);

/// The longest wait between two tries at a daemon that went away.
const MOST_PATIENCE: Duration = Duration::from_secs(8);

/// What reaches the window from the engine thread. Nearly every one is a
/// message, so boxing them to shrink the rare `Lost` would cost more.
#[allow(clippy::large_enum_variant)]
pub enum FromEngine {
    Message(ServerMessage),
    /// The daemon went away. The thread keeps knocking, and the
    /// `Welcome` of a new session says it is back.
    Lost,
}

/// What the window holds to talk to the engine.
pub struct Engine {
    commands: mpsc::UnboundedSender<(RequestId, Command)>,
    /// Server messages, read from the GTK main loop.
    pub messages: async_channel::Receiver<FromEngine>,
    /// Request ids are handed out here rather than by the client, so
    /// that what was asked is known before the answer comes.
    next_id: Cell<u64>,
    /// What has been asked and not yet answered, with the moment of
    /// asking. Both ends of this run on the GTK thread.
    waiting: RefCell<HashMap<RequestId, Instant>>,
    /// Whether the daemon is gone, so nothing asked can be answered.
    lost: Cell<bool>,
}

impl Engine {
    /// Starts the engine (in this process, or a connection to a daemon) on
    /// its own runtime thread.
    pub fn start(config: Config, socket: Option<PathBuf>, demo: bool) -> Self {
        let (commands, mut commands_in) = mpsc::unbounded_channel();
        let (messages_out, messages) = async_channel::unbounded();
        let name = format!("liveside-gui {}", env!("CARGO_PKG_VERSION"));

        std::thread::Builder::new()
            .name("engine".to_owned())
            .spawn(move || {
                let runtime = match tokio::runtime::Builder::new_multi_thread()
                    .enable_all()
                    .build()
                {
                    Ok(runtime) => runtime,
                    Err(e) => {
                        eprintln!("liveside-gui: cannot start the runtime: {e}");
                        return;
                    }
                };
                runtime.block_on(async move {
                    let mut connection = match connect(config, socket.clone(), demo, &name).await {
                        Ok(connection) => connection,
                        Err(e) => {
                            eprintln!("liveside-gui: {e}");
                            // Said in the window too, which otherwise stood
                            // empty with the reason only in a terminal.
                            let _ = messages_out
                                .send(FromEngine::Message(ServerMessage::Rejected {
                                    reason: e.to_string(),
                                }))
                                .await;
                            return;
                        }
                    };
                    loop {
                        match relay(&mut connection, &mut commands_in, &messages_out).await {
                            End::Window | End::Refused => return,
                            End::Engine => {}
                        }
                        // An engine in this process does not come back; a
                        // daemon may well, restarted or upgraded.
                        let Some(path) = &socket else {
                            return;
                        };
                        if messages_out.send(FromEngine::Lost).await.is_err() {
                            return;
                        }
                        match reconnect(path, &name, &mut commands_in).await {
                            Some(again) => connection = again,
                            None => return,
                        }
                    }
                });
            })
            .expect("a thread for the engine");

        Self {
            commands,
            messages,
            next_id: Cell::new(1),
            waiting: RefCell::new(HashMap::new()),
            lost: Cell::new(false),
        }
    }

    pub fn send(&self, command: Command) {
        // The thread drops it anyway; counted, it would turn the wheel
        // for a minute.
        if self.lost.get() {
            return;
        }
        let id = RequestId(self.next_id.get());
        self.next_id.set(self.next_id.get().wrapping_add(1));
        let now = Instant::now();
        // Anything owed for a minute is forgotten: an answer that never
        // came must not leave the wheel turning for the rest of the
        // evening.
        let mut waiting = self.waiting.borrow_mut();
        waiting.retain(|_, asked| now.duration_since(*asked) < Duration::from_secs(60));
        waiting.insert(id, now);
        drop(waiting);
        // The engine thread only stops when the window is gone.
        let _ = self.commands.send((id, command));
    }

    /// Notes that a request has been answered.
    pub fn answered(&self, id: RequestId) {
        self.waiting.borrow_mut().remove(&id);
    }

    /// Notes that the daemon went away: nothing asked will be answered.
    pub fn lose(&self) {
        self.lost.set(true);
        self.waiting.borrow_mut().clear();
    }

    /// Notes that it is back. True when it had been away.
    pub fn regain(&self) -> bool {
        self.lost.replace(false)
    }

    pub fn is_lost(&self) -> bool {
        self.lost.get()
    }

    /// Whether something has been asked for long enough to be worth
    /// saying so.
    pub fn is_waiting(&self) -> bool {
        let now = Instant::now();
        self.waiting
            .borrow()
            .values()
            .any(|asked| now.duration_since(*asked) >= PATIENCE)
    }
}

/// Why [`relay`] stopped.
enum End {
    /// The window is gone.
    Window,
    /// So is the engine.
    Engine,
    /// The daemon would not have this window, most likely after an
    /// upgrade to another protocol. Asking again would be refused again.
    Refused,
}

/// Passes commands one way and messages the other until either side
/// ends.
async fn relay(
    connection: &mut Connection,
    commands: &mut mpsc::UnboundedReceiver<(RequestId, Command)>,
    messages: &async_channel::Sender<FromEngine>,
) -> End {
    let mut refused = false;
    loop {
        tokio::select! {
            command = commands.recv() => match command {
                Some((id, command)) => {
                    // The window shows what the engine says about a
                    // command, so nothing is done with the reply here.
                    let _ = connection.client.send_with_id(id, command).await;
                }
                None => return End::Window,
            },
            message = connection.messages.recv() => match message {
                Some(message) => {
                    refused |= matches!(message, ServerMessage::Rejected { .. });
                    if messages.send(FromEngine::Message(message)).await.is_err() {
                        return End::Window;
                    }
                }
                None if refused => return End::Refused,
                None => return End::Engine,
            },
        }
    }
}

/// Tries the daemon's socket until it answers, waiting a little longer
/// each time. What the window asks meanwhile is dropped: it knows the
/// daemon is gone. `None` when the window closed instead.
async fn reconnect(
    path: &Path,
    name: &str,
    commands: &mut mpsc::UnboundedReceiver<(RequestId, Command)>,
) -> Option<Connection> {
    let mut wait = Duration::from_secs(1);
    loop {
        let pause = tokio::time::sleep(wait);
        tokio::pin!(pause);
        loop {
            tokio::select! {
                _ = &mut pause => break,
                command = commands.recv() => {
                    command?;
                }
            }
        }
        if let Ok(connection) = liveside_client::connect(path, name).await {
            return Some(connection);
        }
        wait = (wait * 2).min(MOST_PATIENCE);
    }
}

async fn connect(
    config: Config,
    socket: Option<PathBuf>,
    demo: bool,
    name: &str,
) -> Result<Connection, String> {
    if let Some(path) = socket {
        return liveside_client::connect(&path, name)
            .await
            .map_err(|e| format!("cannot connect to {}: {e}", path.display()));
    }
    let mut engine = config.engine();
    // The demo opens again only what was open in the demo.
    if demo && engine.open_channels.is_some() {
        engine.open_channels = Some(liveside_client::open_channels::default_path(true));
    }
    Ok(if demo {
        let membership = config.chat.user_list == liveside_client::config::UserList::Irc;
        liveside_client::in_process_demo(name, engine, membership)
    } else {
        liveside_client::in_process_twitch(name, engine, config.twitch())
    })
}
