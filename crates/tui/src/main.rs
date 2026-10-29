//! Terminal frontend.
//!
//! Runs either against a daemon (`--socket`) or an engine started in this
//! process (`--demo`). Both look the same from here: a `Connection` that
//! takes commands and yields server messages.

mod app;
mod hyperlink;
mod render;

use std::path::PathBuf;
use std::process::ExitCode;

use liveside_client::Connection;
use liveside_proto::model::NoticeLevel;
use liveside_proto::{Command, Login};
use ratatui::crossterm::event::{Event as TermEvent, KeyEvent, KeyEventKind};
use tokio::sync::mpsc;

use crate::app::App;
use liveside_client::settings::Settings;

const USAGE: &str = "\
Usage: liveside-tui [--demo | --daemon | --socket PATH] [--channel NAME[,NAME...]]

With no options it talks to Twitch from this process.

Options:
  --demo             Use the demo backend (fake chat, no Twitch account)
  --daemon           Connect to a daemon on the default socket
  --socket PATH      Connect to a daemon on this socket
                     (default: $XDG_RUNTIME_DIR/liveside/daemon.sock)
  --channel NAMES    Join these channels on start (comma separated;
                     replaces the channels from the settings file)
  --config PATH      Settings file (default: $XDG_CONFIG_HOME/liveside/config.toml)
  --check-config     Check that file and print the settings it gives
  --no-history       Keep no local chat log, and restore none on join
  --no-third-party-emotes
                     Do not contact 7TV, BetterTTV and FrankerFaceZ
                     (/privacy lists what is contacted and stored)
  --login            Log in on start
  -h, --help         Show this help
  -V, --version      Show the version

Keys:
  Tab                Complete a name, emote, /command or :emoji: (again: next one)
                     On an empty line it switches buffers, as before
  Up/Down            Lines sent earlier
  Ctrl-Up/Down       Pick a message; Ctrl-R answers it (Esc cancels)
  Ctrl-T/B/D, Alt-P  Prepare /timeout, /ban, /delete or /pin for that message
                     (the line is only sent when you press Enter)
  PgUp/PgDn          Scroll back and forth a screenful (Esc: newest)
  Ctrl-F             Search this buffer: type, then Enter/Up older,
                     Down newer, Esc done (the match is selected)
  F3 or /users       Show or hide the list of people active in the channel
  F2 or /settings    Settings (edits the file above)
  Enter              Send the typed line (/join, /part, /timeout, ...)
                     /privacy lists what is sent where and what is stored
                     /stream [quality] plays the channel with Streamlink
  Tab / Shift-Tab    Next / previous buffer      Ctrl-N / Ctrl-P  the same
  PageUp / PageDown  Scroll                      Esc              back to live
  Ctrl-U / Ctrl-W    Clear input / delete word
  Ctrl-C / Ctrl-Q    Quit                        /quit            the same
";

struct Args {
    config: Option<std::path::PathBuf>,
    check_config: bool,
    demo: bool,
    daemon: bool,
    history: bool,
    third_party_emotes: bool,
    socket: PathBuf,
    channels: Vec<Login>,
    login: bool,
}

/// Engine settings: the file first, then what the command line overrides.
/// The demo backend invents its chat, so it is never written to the log.
fn engine_config(config: &liveside_client::Config, args: &Args) -> liveside_client::EngineConfig {
    let mut engine = config.engine();
    if !args.history || args.demo {
        engine.history = liveside_client::HistoryConfig::disabled();
    }
    // Channels named on the command line are the ones wanted this time;
    // the demo opens again only what was open in the demo.
    if !args.channels.is_empty() {
        engine.open_channels = None;
    } else if args.demo && engine.open_channels.is_some() {
        engine.open_channels = Some(liveside_client::open_channels::default_path(true));
    }
    engine
}

fn parse_args() -> Result<Args, String> {
    let mut args = Args {
        config: None,
        check_config: false,
        demo: false,
        daemon: false,
        history: true,
        third_party_emotes: true,
        socket: liveside_client::default_socket_path(),
        channels: Vec::new(),
        login: false,
    };
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--check-config" => args.check_config = true,
            "--config" => {
                let path = it.next().ok_or("--config needs a path")?;
                args.config = Some(path.into());
            }
            "--demo" => args.demo = true,
            "--no-history" => args.history = false,
            "--no-third-party-emotes" => args.third_party_emotes = false,
            "--daemon" => args.daemon = true,
            "--login" => args.login = true,
            "--socket" => {
                args.socket = it.next().ok_or("--socket needs a path")?.into();
                args.daemon = true;
            }
            "--channel" => {
                let names = it.next().ok_or("--channel needs a name")?;
                args.channels.extend(
                    names
                        .split(',')
                        .filter(|s| !s.is_empty())
                        .map(|s| Login(s.trim_start_matches('#').to_ascii_lowercase())),
                );
            }
            "-h" | "--help" => {
                print!("{USAGE}");
                std::process::exit(0);
            }
            "-V" | "--version" => {
                println!("liveside-tui {}", env!("CARGO_PKG_VERSION"));
                std::process::exit(0);
            }
            other => return Err(format!("unknown argument {other:?}")),
        }
    }
    Ok(args)
}

#[tokio::main]
async fn main() -> ExitCode {
    let args = match parse_args() {
        Ok(args) => args,
        Err(e) => {
            eprintln!("liveside-tui: {e}\n\n{USAGE}");
            return ExitCode::FAILURE;
        }
    };

    let config_path = args
        .config
        .clone()
        .unwrap_or_else(liveside_client::config::default_path);
    let config = match liveside_client::Config::load(&config_path) {
        Ok(config) => config,
        Err(e) => {
            // Starting with settings that silently did not apply would be
            // worse than not starting.
            eprintln!("liveside-tui: {e}");
            return ExitCode::FAILURE;
        }
    };
    if args.check_config {
        println!("{} is valid.\n", config_path.display());
        println!("{}", config.summary());
        if !args.channels.is_empty() {
            println!(
                "\nThis run would override the channels with: {}",
                args.channels
                    .iter()
                    .map(|c| c.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        }
        return ExitCode::SUCCESS;
    }

    let name = format!("liveside-tui {}", env!("CARGO_PKG_VERSION"));
    let connection = if args.daemon {
        match liveside_client::connect(&args.socket, &name).await {
            Ok(connection) => connection,
            Err(e) => {
                eprintln!(
                    "liveside-tui: cannot connect to {}: {e}\n\
                     Start the daemon first (liveside-daemon), or drop --daemon.",
                    args.socket.display()
                );
                return ExitCode::FAILURE;
            }
        }
    } else if args.demo {
        liveside_client::in_process_demo(
            &name,
            engine_config(&config, &args),
            config.chat.user_list == liveside_client::config::UserList::Irc,
        )
    } else {
        let mut twitch = config.twitch();
        if !args.third_party_emotes {
            twitch.third_party_emotes = false;
        }
        liveside_client::in_process_twitch(&name, engine_config(&config, &args), twitch)
    };

    // Asked before the screen is taken over: the answer comes in on the
    // same input as the keys, and nothing reads that yet. Only when the
    // settings leave it to the terminal; a terminal that cannot answer is
    // told apart quickly, and one over a slow SSH link gets a second.
    let terminal_light = (config.ui.theme == liveside_client::config::Theme::System)
        .then(|| {
            terminal_colorsaurus::theme_mode(terminal_colorsaurus::QueryOptions::default())
                .ok()
                .map(|mode| mode == terminal_colorsaurus::ThemeMode::Light)
        })
        .flatten();
    render::set_light_background(app::light_background(config.ui.theme, terminal_light));

    // Restore the terminal even if drawing panics, so the shell stays usable.
    let previous_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        restore();
        previous_hook(info);
    }));
    let mut terminal = ratatui::init();
    // Whether the terminal has the focus decides what is worth a
    // notification. A terminal that cannot say ignores the request.
    let _ = ratatui::crossterm::execute!(
        std::io::stdout(),
        ratatui::crossterm::event::EnableFocusChange
    );
    let result = run(
        &mut terminal,
        connection,
        &args,
        config,
        config_path,
        terminal_light,
    )
    .await;
    restore();

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("liveside-tui: {e}");
            ExitCode::FAILURE
        }
    }
}

/// Gives the terminal back as it was: focus reports left on would type
/// `^[[I` into the shell every time its window is clicked.
fn restore() {
    let _ = ratatui::crossterm::execute!(
        std::io::stdout(),
        ratatui::crossterm::event::DisableFocusChange
    );
    ratatui::restore();
}

/// Draws the screen and, when links are to be opened from the terminal,
/// makes them clickable.
fn draw(
    terminal: &mut ratatui::DefaultTerminal,
    app: &App,
    links_drawn: &mut liveside_client::config::LinkMode,
) -> Result<(), String> {
    use liveside_client::config::LinkMode;
    let mode = app.settings.config().security.links;
    if mode != *links_drawn {
        // Cells ratatui thinks unchanged would keep being links.
        terminal.clear().map_err(|e| format!("cannot draw: {e}"))?;
        *links_drawn = mode;
    }
    let mut links = Vec::new();
    let completed = terminal
        .draw(|frame| links = render::draw(frame, app))
        .map_err(|e| format!("cannot draw: {e}"))?;
    if mode != LinkMode::Open {
        return Ok(());
    }
    let pieces = hyperlink::pieces(completed.buffer, links);
    hyperlink::paint(terminal.backend_mut(), &pieces).map_err(|e| format!("cannot draw: {e}"))
}

async fn run(
    terminal: &mut ratatui::DefaultTerminal,
    mut connection: Connection,
    args: &Args,
    config: liveside_client::Config,
    config_path: std::path::PathBuf,
    terminal_light: Option<bool>,
) -> Result<(), String> {
    let mut app = App::default();
    app.terminal_light = terminal_light;
    let mut keys = key_reader();
    // The channels to join come from the file unless the command line
    // names some; the settings screen edits the same file.
    let channels: Vec<Login> = if args.channels.is_empty() {
        config
            .channels
            .iter()
            .map(|name| name.as_str().into())
            .collect()
    } else {
        args.channels.clone()
    };
    // The language the file asks for, or what the environment says.
    app.language = liveside_client::lang::Language::parse(&config.ui.language)
        .unwrap_or_else(liveside_client::lang::Language::detect);
    app.settings = Settings::new(
        config,
        config_path,
        app.language,
        liveside_client::settings::Frontend::Terminal,
    );
    let (streamlink_out, mut streamlink_in) = tokio::sync::mpsc::unbounded_channel();
    app.streamlink_ended = Some(streamlink_out);
    app.notifier
        .set_config(app.settings.config().notify.clone());
    // Why desktop notifications cannot be shown (no session bus over SSH)
    // comes from their own thread, once.
    let (notify_failed, mut notify_failed_in) = tokio::sync::mpsc::unbounded_channel::<String>();
    // And where a clicked notification leads, from the thread that heard
    // the click. A terminal cannot come to the front, so the activation
    // token goes unused.
    let (notify_clicked, mut notify_clicked_in) =
        tokio::sync::mpsc::unbounded_channel::<liveside_client::notify::Place>();
    app.desktop = Some(liveside_client::notify::Desktop::start(
        move |error| {
            let _ = notify_failed.send(error);
        },
        std::sync::Arc::new(move |place, _token| {
            let _ = notify_clicked.send(place);
        }),
        app.text().notify_open,
    ));

    if args.login || args.demo {
        // The demo backend logs in instantly; Twitch needs /login, unless
        // asked for here.
        send(&connection, &mut app, Command::Login).await;
    }
    for channel in &channels {
        let command = Command::Join {
            channel: channel.clone(),
        };
        send(&connection, &mut app, command).await;
    }

    // What links were drawn as, so a change from the settings redraws the
    // screen rather than leave clickable cells behind.
    let mut links_drawn = app.settings.config().security.links;
    while !app.quit {
        // The scroll limit depends on how much fits on screen, which only
        // the terminal knows, and it changes when the window is resized.
        if let Ok(size) = terminal.size() {
            app.chat_height = usize::from(size.height.saturating_sub(2)).max(1);
        }
        draw(terminal, &app, &mut links_drawn)?;
        if std::mem::take(&mut app.bell) {
            use std::io::Write;
            let backend = terminal.backend_mut();
            let _ = backend.write_all(b"\x07").and_then(|()| backend.flush());
        }

        tokio::select! {
            // Only while something is waiting: a quiet terminal must not
            // redraw ten times a second for a wheel nobody is watching.
            _ = tokio::time::sleep(std::time::Duration::from_millis(120)), if app.busy() => {
                app.spinner = app.spinner.wrapping_add(1);
            }
            message = connection.messages.recv() => match message {
                Some(message) => app.apply(message),
                None => {
                    // Daemon gone: show why instead of vanishing, and
                    // say it again in the shell, where it stays.
                    let said = app.text().connection_closed;
                    app.local(NoticeLevel::Error, said);
                    draw(terminal, &app, &mut links_drawn)?;
                    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                    return Err(said.to_owned());
                }
            },
            key = keys.recv() => match key {
                Some(Terminal::Key(key)) => {
                    for command in app.on_key(key) {
                        send(&connection, &mut app, command).await;
                    }
                }
                Some(Terminal::Focus(focused)) => app.focused = focused,
                None => return Ok(()),
            },
            Some(place) = notify_clicked_in.recv() => {
                for command in app.go_to(place) {
                    send(&connection, &mut app, command).await;
                }
            }
            Some(error) = notify_failed_in.recv() => {
                let said = liveside_client::lang::fill(
                    app.text().notify_desktop_failed,
                    &[("error", &error)],
                );
                app.status(NoticeLevel::Warning, said);
            }
            Some((from, outcome)) = streamlink_in.recv() => app.streamlink_ended(&from, outcome),
        }
    }
    Ok(())
}

async fn send(connection: &Connection, app: &mut App, command: Command) {
    match connection.client.send(command).await {
        // Noted as owed, so the header can say the client is working.
        Ok(id) => app.asked(id),
        Err(_) => {
            app.local(NoticeLevel::Error, app.text().connection_closed);
            app.quit = true;
        }
    }
}

/// What the terminal says: a key, or that it gained or lost the focus.
enum Terminal {
    Key(KeyEvent),
    Focus(bool),
}

/// Reads the terminal on a blocking thread and forwards what matters.
fn key_reader() -> mpsc::Receiver<Terminal> {
    let (tx, rx) = mpsc::channel(64);
    std::thread::spawn(move || {
        loop {
            let said = match ratatui::crossterm::event::read() {
                // Windows sends key release events too; only act on presses.
                Ok(TermEvent::Key(key)) if key.kind == KeyEventKind::Press => Terminal::Key(key),
                Ok(TermEvent::FocusGained) => Terminal::Focus(true),
                Ok(TermEvent::FocusLost) => Terminal::Focus(false),
                Ok(_) => continue,
                Err(_) => return,
            };
            if tx.blocking_send(said).is_err() {
                return;
            }
        }
    });
    rx
}
