//! Runs the engine in the background and serves frontends over a Unix socket.

use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use liveside_core::EngineConfig;
use liveside_core::config::{self, Config};
use liveside_core::demo::{self, DemoConfig};
use liveside_core::history::HistoryConfig;
use liveside_core::twitch::{self, TwitchConfig};
use liveside_proto::Command;
use tokio::net::{UnixListener, UnixStream};
use tokio::signal::unix::{SignalKind, signal};

const USAGE: &str = "\
Usage: liveside-daemon [--demo] [--socket PATH]

Runs the chat engine and serves frontends over a Unix socket.
Without --demo it talks to Twitch; log in from a frontend (/login).

Options:
  --config PATH   Settings file (default: $XDG_CONFIG_HOME/liveside/config.toml)
  --check-config  Check that file and print the settings it gives
  --demo          Use the demo backend (fake chat, no Twitch account)
  --no-history    Keep no local chat log, and restore none on join
  --no-third-party-emotes
                  Do not contact 7TV, BetterTTV and FrankerFaceZ
  --socket PATH   Socket to listen on
                  (default: $XDG_RUNTIME_DIR/liveside/daemon.sock)
  -h, --help      Show this help
  -V, --version   Show the version
";

struct Args {
    config: Option<PathBuf>,
    check_config: bool,
    history: bool,
    third_party_emotes: bool,
    socket: PathBuf,
    demo: bool,
}

fn parse_args() -> Result<Args, String> {
    let mut args = Args {
        socket: liveside_client::default_socket_path(),
        config: None,
        check_config: false,
        demo: false,
        history: true,
        third_party_emotes: true,
    };
    let mut it = std::env::args_os().skip(1);
    while let Some(arg) = it.next() {
        match arg.to_str() {
            Some("--check-config") => args.check_config = true,
            Some("--config") => {
                let path = it.next().ok_or("--config needs a path")?;
                args.config = Some(path.into());
            }
            Some("--demo") => args.demo = true,
            Some("--no-history") => args.history = false,
            Some("--no-third-party-emotes") => args.third_party_emotes = false,
            Some("--socket") => {
                args.socket = it.next().ok_or("--socket needs a path")?.into();
            }
            Some("-h" | "--help") => {
                print!("{USAGE}");
                std::process::exit(0);
            }
            Some("-V" | "--version") => {
                println!("liveside-daemon {}", env!("CARGO_PKG_VERSION"));
                std::process::exit(0);
            }
            _ => return Err(format!("unknown argument {arg:?}")),
        }
    }
    Ok(args)
}

/// Twitch settings: the file, then what the command line overrides.
fn config_path(args: &Args) -> PathBuf {
    args.config.clone().unwrap_or_else(config::default_path)
}

fn twitch_config(config: &Config, args: &Args) -> TwitchConfig {
    let mut twitch = config.twitch();
    if !args.third_party_emotes {
        twitch.third_party_emotes = false;
    }
    twitch
}

/// Engine settings, likewise. The demo backend invents its chat, so it is
/// never written to the log.
fn engine_config(config: &Config, args: &Args) -> EngineConfig {
    let mut engine = config.engine();
    if !args.history || args.demo {
        engine.history = HistoryConfig::disabled();
    }
    // The demo opens again only what was open in the demo.
    if args.demo && engine.open_channels.is_some() {
        engine.open_channels = Some(liveside_core::open_channels::default_path(true));
    }
    engine
}

#[tokio::main]
async fn main() -> ExitCode {
    let args = match parse_args() {
        Ok(args) => args,
        Err(e) => {
            eprintln!("liveside-daemon: {e}\n\n{USAGE}");
            return ExitCode::FAILURE;
        }
    };
    match run(&args).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("liveside-daemon: {e}");
            ExitCode::FAILURE
        }
    }
}

async fn run(args: &Args) -> Result<(), String> {
    // The settings are read before the socket is created: a broken file
    // should not leave a socket behind, and --check-config must not fight
    // a running daemon for it.
    let path = config_path(args);
    let settings = Config::load(&path).map_err(|e| e.to_string())?;
    if args.check_config {
        println!("{} is valid.\n", path.display());
        println!("{}", settings.summary());
        return Ok(());
    }

    let listener = bind(&args.socket).await?;
    eprintln!("liveside-daemon: listening on {}", args.socket.display());
    let engine_settings = engine_config(&settings, args);
    let (engine, mut engine_task) = if args.demo {
        liveside_core::start(engine_settings, demo::spawn(DemoConfig::default()))
    } else {
        liveside_core::start(
            engine_settings,
            twitch::spawn(twitch_config(&settings, args)),
        )
    };
    // The demo backend logs in instantly, and a frontend running the demo
    // itself asks it to. One attached to a demo daemon cannot tell it is
    // the demo, so the daemon asks, or every client came up logged out.
    if args.demo {
        let _ = engine.command(Command::Login).await;
    }
    let server = format!("liveside-daemon {}", env!("CARGO_PKG_VERSION"));

    let mut sigterm = signal(SignalKind::terminate()).map_err(|e| e.to_string())?;
    let mut sigint = signal(SignalKind::interrupt()).map_err(|e| e.to_string())?;

    loop {
        tokio::select! {
            accepted = listener.accept() => match accepted {
                Ok((stream, _)) => {
                    tokio::spawn(liveside_client::socket::serve(engine.clone(), server.clone(), stream));
                }
                Err(e) => eprintln!("liveside-daemon: accept failed: {e}"),
            },
            _ = &mut engine_task => {
                eprintln!("liveside-daemon: engine stopped");
                break;
            }
            _ = sigterm.recv() => break,
            _ = sigint.recv() => break,
        }
    }

    let _ = std::fs::remove_file(&args.socket);
    Ok(())
}

/// Binds the socket, refusing to start if another daemon is listening and
/// cleaning up a stale socket file left by one that crashed.
async fn bind(path: &Path) -> Result<UnixListener, String> {
    if let Some(dir) = path.parent() {
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir)
            .map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    }
    if path.exists() {
        if UnixStream::connect(path).await.is_ok() {
            return Err(format!(
                "already running (socket {} is live)",
                path.display()
            ));
        }
        std::fs::remove_file(path)
            .map_err(|e| format!("cannot remove stale socket {}: {e}", path.display()))?;
    }
    let listener =
        UnixListener::bind(path).map_err(|e| format!("cannot bind {}: {e}", path.display()))?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
        .map_err(|e| format!("cannot restrict {}: {e}", path.display()))?;
    Ok(listener)
}
