//! Starting Streamlink for a channel, on the machine the reader sits at.
//!
//! The frontend starts it, not the engine: a daemon may run somewhere with
//! no screen, and the player belongs where the reader is. Streamlink runs
//! on its own; what it prints is kept only to say why, should it end with
//! an error ("No playable streams found" for a channel that is off).

use std::process::Stdio;

use liveside_core::config::StreamlinkConfig;

/// What starting Streamlink came to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// It played and was closed, or is still playing: nothing to say.
    Done,
    /// It ended with an error; the line that says why.
    Failed(String),
    /// It ended without an error, but so soon that the player cannot have
    /// played anything: it closed at once, which Streamlink does not
    /// count as a failure. What it said last, and how soon.
    ClosedAtOnce { seconds: u64, said: String },
}

/// How soon after starting an ending without an error means the player
/// never got going.
const AT_ONCE: std::time::Duration = std::time::Duration::from_secs(10);

/// The arguments Streamlink is given for `login` at `quality`.
pub fn arguments(config: &StreamlinkConfig, login: &str, quality: &str) -> Vec<String> {
    let mut args = vec![format!("https://www.twitch.tv/{login}"), quality.to_owned()];
    if !config.player.trim().is_empty() {
        args.push("--player".to_owned());
        args.push(config.player.trim().to_owned());
    }
    args.extend(split_arguments(&config.arguments));
    args
}

/// Splits a line into arguments the way a shell would for the simple
/// cases: on spaces, keeping what is inside quotes together, with a
/// backslash taking the next character as it is.
pub fn split_arguments(line: &str) -> Vec<String> {
    let mut args = Vec::new();
    let mut current = String::new();
    let mut started = false;
    let mut quote: Option<char> = None;
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        match (quote, c) {
            (_, '\\') if quote != Some('\'') => {
                if let Some(next) = chars.next() {
                    current.push(next);
                    started = true;
                }
            }
            (None, '"' | '\'') => {
                quote = Some(c);
                started = true;
            }
            (Some(q), c) if c == q => quote = None,
            (None, c) if c.is_whitespace() => {
                if started {
                    args.push(std::mem::take(&mut current));
                    started = false;
                }
            }
            (_, c) => {
                current.push(c);
                started = true;
            }
        }
    }
    if started {
        args.push(current);
    }
    args
}

/// Starts Streamlink for `login`, at `quality` or the configured one.
/// `ended` is called from another thread once it exits, which may be
/// hours later. An error here means it could not be started at all.
pub fn start(
    config: &StreamlinkConfig,
    login: &str,
    quality: Option<&str>,
    ended: impl FnOnce(Outcome) + Send + 'static,
) -> std::io::Result<()> {
    let quality = quality
        .filter(|quality| !quality.trim().is_empty())
        .unwrap_or(&config.quality);
    let child = liveside_core::host::command(config.command.trim(), None)
        .args(arguments(config, login, quality))
        // Nothing of it in the terminal the chat is drawn in.
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let began = std::time::Instant::now();
    std::thread::spawn(move || {
        let outcome = match child.wait_with_output() {
            Ok(output) if output.status.success() && began.elapsed() < AT_ONCE => {
                let text = format!(
                    "{}\n{}",
                    String::from_utf8_lossy(&output.stdout),
                    String::from_utf8_lossy(&output.stderr)
                );
                Outcome::ClosedAtOnce {
                    seconds: began.elapsed().as_secs(),
                    said: player_line(&text)
                        .or_else(|| why(&text))
                        .unwrap_or_default(),
                }
            }
            Ok(output) if output.status.success() => Outcome::Done,
            Ok(output) => {
                let text = format!(
                    "{}\n{}",
                    String::from_utf8_lossy(&output.stdout),
                    String::from_utf8_lossy(&output.stderr)
                );
                Outcome::Failed(why(&text).unwrap_or_else(|| output.status.to_string()))
            }
            Err(e) => Outcome::Failed(e.to_string()),
        };
        ended(outcome);
    });
    Ok(())
}

/// What Streamlink said about the player, when it said anything: the
/// line that names it, which is where a wrong player shows.
fn player_line(output: &str) -> Option<String> {
    output
        .lines()
        .map(str::trim)
        .find(|line| line.contains("Starting player"))
        .map(str::to_owned)
}

/// The line that says why Streamlink stopped: its own `error:` line, or
/// else the last thing it said.
fn why(output: &str) -> Option<String> {
    let lines: Vec<&str> = output
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    lines
        .iter()
        .rev()
        .find_map(|line| line.strip_prefix("error:"))
        .map(|line| line.trim().to_owned())
        .or_else(|| lines.last().map(|line| (*line).to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_channel_and_quality_are_given_and_the_player_when_set() {
        let mut config = StreamlinkConfig::default();
        assert_eq!(
            arguments(&config, "harborlight", "best"),
            ["https://www.twitch.tv/harborlight", "best"]
        );
        config.player = " mpv ".to_owned();
        assert_eq!(
            arguments(&config, "harborlight", "720p60,best"),
            [
                "https://www.twitch.tv/harborlight",
                "720p60,best",
                "--player",
                "mpv"
            ]
        );
    }

    #[test]
    fn more_arguments_are_split_as_a_shell_would() {
        assert_eq!(
            split_arguments(r#"--twitch-low-latency --player-args "--vo=wlshm --mute""#),
            ["--twitch-low-latency", "--player-args", "--vo=wlshm --mute"]
        );
        assert_eq!(
            split_arguments(r#"a\ b 'c "d"' """#),
            ["a b", "c \"d\"", ""]
        );
        assert!(split_arguments("   ").is_empty());
        let config = StreamlinkConfig {
            arguments: "--twitch-low-latency".to_owned(),
            ..StreamlinkConfig::default()
        };
        assert_eq!(
            arguments(&config, "x", "best").last().map(String::as_str),
            Some("--twitch-low-latency")
        );
    }

    #[test]
    fn its_own_error_line_says_why() {
        let said = "[cli][info] Found matching plugin twitch for URL https://www.twitch.tv/x\n\
                    error: No playable streams found on this URL: https://www.twitch.tv/x\n";
        assert_eq!(
            why(said).as_deref(),
            Some("No playable streams found on this URL: https://www.twitch.tv/x")
        );
        assert_eq!(why("just this\n").as_deref(), Some("just this"));
        assert_eq!(why("\n"), None);
    }

    #[test]
    fn a_failing_program_is_reported_with_its_error() {
        use std::os::unix::fs::PermissionsExt;
        // A stand-in for Streamlink that says what it was given and fails
        // the way Streamlink does for a channel that is off.
        let dir = std::env::temp_dir().join(format!("liveside-streamlink-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let fake = dir.join("streamlink");
        std::fs::write(
            &fake,
            "#!/bin/sh\necho \"[cli][info] $1 $2\"\necho \"error: No playable streams found on this URL: $1\" >&2\nexit 1\n",
        )
        .unwrap();
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
        let config = StreamlinkConfig {
            command: fake.display().to_string(),
            ..StreamlinkConfig::default()
        };
        let (tx, rx) = std::sync::mpsc::channel();
        start(&config, "harborlight", None, move |outcome| {
            tx.send(outcome).unwrap()
        })
        .unwrap();
        let outcome = rx.recv_timeout(std::time::Duration::from_secs(10)).unwrap();
        assert_eq!(
            outcome,
            Outcome::Failed(
                "No playable streams found on this URL: https://www.twitch.tv/harborlight"
                    .to_owned()
            )
        );

        // One that ends at once without an error: the player closed.
        std::fs::write(
            &fake,
            "#!/bin/sh\necho '[cli][info] Starting player: mpv'\necho '[cli][info] Player closed'\nexit 0\n",
        )
        .unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        start(&config, "harborlight", None, move |outcome| {
            tx.send(outcome).unwrap()
        })
        .unwrap();
        let outcome = rx.recv_timeout(std::time::Duration::from_secs(10)).unwrap();
        assert!(
            matches!(&outcome, Outcome::ClosedAtOnce { said, .. } if said.contains("Starting player: mpv")),
            "{outcome:?}"
        );
        std::fs::remove_dir_all(&dir).unwrap();

        let missing = StreamlinkConfig {
            command: "liveside-no-such-program".to_owned(),
            ..StreamlinkConfig::default()
        };
        assert!(start(&missing, "x", None, |_| {}).is_err());
    }
}
