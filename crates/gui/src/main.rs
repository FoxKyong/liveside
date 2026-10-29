//! The graphical frontend: a window over the same engine as the terminal
//! one, so both show the same chat and obey the same settings file.

mod about;
mod blocked;
mod bots;
mod card;
mod channel;
mod chat;
mod collected;
mod dialog;
mod emotes;
mod engine;
mod followed;
mod graph;
mod join;
mod link;
mod moderated;
mod moderation;
mod notify;
mod people;
mod poll;
mod prediction;
mod privacy;
mod review;
mod rewards;
mod schedule;
mod search;
mod settings;
mod stream_chat;
mod theme;
mod users;
mod window;

use std::path::PathBuf;
use std::process::ExitCode;

use gtk::prelude::*;
use liveside_client::lang::Language;

const USAGE: &str = "\
liveside-gui — graphical Twitch chat client

Usage: liveside-gui [--demo | --daemon | --socket PATH] [--config PATH]

Without options it talks to Twitch from this process, with the settings
from $XDG_CONFIG_HOME/liveside/config.toml.

Options:
  --demo          Use the demo backend (fake chat, no Twitch account)
  --daemon        Connect to a daemon on the default socket
  --socket PATH   Connect to a daemon on this socket
  --config PATH   Settings file
  -h, --help      Show this help
  -V, --version   Show the version
";

struct Args {
    demo: bool,
    socket: Option<PathBuf>,
    config: Option<PathBuf>,
}

fn main() -> ExitCode {
    let args = match parse_args() {
        Ok(Some(args)) => args,
        Ok(None) => {
            println!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        Err(e) => {
            eprintln!("liveside-gui: {e}\n\n{USAGE}");
            return ExitCode::FAILURE;
        }
    };

    let path = args
        .config
        .clone()
        .unwrap_or_else(liveside_client::config::default_path);
    let config = match liveside_client::Config::load(&path) {
        Ok(config) => config,
        Err(e) => {
            // As in the terminal client: settings that quietly do not apply
            // are worse than not starting.
            eprintln!("liveside-gui: {e}");
            return ExitCode::FAILURE;
        }
    };
    let language = Language::parse(&config.ui.language).unwrap_or_else(Language::detect);

    let application = gtk::Application::builder()
        .application_id("net.jifox.liveside.Liveside")
        // Several windows on one machine are fine: each is its own client.
        .flags(gtk::gio::ApplicationFlags::NON_UNIQUE)
        .build();
    application.connect_activate(move |application| {
        window::build(
            application,
            config.clone(),
            path.clone(),
            args.socket.clone(),
            args.demo,
            language,
        );
    });
    // GTK parses its own arguments; ours are already handled.
    let status = application.run_with_args::<&str>(&[]);
    if status == gtk::glib::ExitCode::SUCCESS {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

fn parse_args() -> Result<Option<Args>, String> {
    let mut args = Args {
        demo: false,
        socket: None,
        config: None,
    };
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--demo" => args.demo = true,
            "--daemon" => args.socket = Some(liveside_client::default_socket_path()),
            "--socket" => {
                let path = it.next().ok_or("--socket needs a path")?;
                args.socket = Some(path.into());
            }
            "--config" => {
                let path = it.next().ok_or("--config needs a path")?;
                args.config = Some(path.into());
            }
            "-h" | "--help" => return Ok(None),
            "-V" | "--version" => {
                println!("liveside-gui {}", env!("CARGO_PKG_VERSION"));
                std::process::exit(0);
            }
            other => return Err(format!("unknown option {other}")),
        }
    }
    Ok(Some(args))
}
