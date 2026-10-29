//! Programs of the user's own system: Streamlink and its player, a bot,
//! a sound player given a file of the user's.
//!
//! Inside a Flatpak the sandbox holds only the runtime's programs and none
//! of the user's files, so these run on the host through
//! `flatpak-spawn --host`, which the manifest allows by letting the app
//! talk to `org.freedesktop.Flatpak`. Running the user's own programs is
//! what these features are; see `docs/architecture.md`, *Packages*.

use std::ffi::OsString;
use std::path::Path;
use std::process::Command;
use std::sync::OnceLock;

/// Whether this process runs inside a Flatpak, which puts this file at
/// the root of every sandbox.
pub fn in_flatpak() -> bool {
    static INSIDE: OnceLock<bool> = OnceLock::new();
    *INSIDE.get_or_init(|| Path::new("/.flatpak-info").exists())
}

/// A command for `program` that runs it on the user's own system, in
/// `dir` when given; the caller adds the arguments and the pipes, which
/// `flatpak-spawn` passes through.
pub fn command(program: impl Into<OsString>, dir: Option<&Path>) -> Command {
    command_for(in_flatpak(), program.into(), dir)
}

fn command_for(flatpak: bool, program: OsString, dir: Option<&Path>) -> Command {
    if !flatpak {
        let mut command = Command::new(program);
        if let Some(dir) = dir {
            command.current_dir(dir);
        }
        return command;
    }
    let mut command = Command::new("flatpak-spawn");
    // `--watch-bus` ends the program when flatpak-spawn ends, so a bot
    // that is killed does not live on outside the sandbox.
    command.args(["--host", "--watch-bus"]);
    // The directory is the host's, which the sandbox may not have, so it
    // is not where flatpak-spawn itself starts.
    if let Some(dir) = dir {
        let mut option = OsString::from("--directory=");
        option.push(dir);
        command.arg(option);
    }
    command.arg(program);
    command
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(command: &Command) -> Vec<String> {
        std::iter::once(command.get_program())
            .chain(command.get_args())
            .map(|part| part.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn outside_a_flatpak_a_program_runs_as_it_is() {
        let mut command = command_for(false, "python3".into(), Some(Path::new("/srv/bots")));
        command.arg("weather.py");
        assert_eq!(argv(&command), ["python3", "weather.py"]);
        assert_eq!(command.get_current_dir(), Some(Path::new("/srv/bots")));
    }

    #[test]
    fn inside_a_flatpak_it_runs_on_the_host() {
        let mut command = command_for(true, "python3".into(), Some(Path::new("/srv/bots")));
        command.arg("weather.py");
        assert_eq!(
            argv(&command),
            [
                "flatpak-spawn",
                "--host",
                "--watch-bus",
                "--directory=/srv/bots",
                "python3",
                "weather.py"
            ]
        );
        assert_eq!(command.get_current_dir(), None);
        let command = command_for(true, "streamlink".into(), None);
        assert_eq!(
            argv(&command),
            ["flatpak-spawn", "--host", "--watch-bus", "streamlink"]
        );
    }
}
