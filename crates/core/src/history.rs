//! Local chat history: the engine's own log of what it saw.
//!
//! Nothing is fetched from anywhere: a channel's history is what this
//! machine recorded while it was joined. The first join of a channel is
//! therefore empty, and it fills up from then on.
//!
//! One file per channel, JSON Lines, in the user's data directory with
//! restrictive permissions. JSON rather than readable text because it
//! round-trips the whole line (badges, emotes, replies) without parsing
//! rules of its own; a text export for humans can come later.

use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use liveside_proto::model::{Line, LineContent};

/// How much of the end of a file is read when loading history. Generous:
/// at ~300 bytes per line this holds far more than `lines`.
const TAIL_BYTES: u64 = 1024 * 1024;

#[derive(Debug, Clone)]
pub struct HistoryConfig {
    /// Where the logs live. `None` disables reading and writing entirely.
    pub dir: Option<PathBuf>,
    /// Lines restored when joining a channel.
    pub lines: usize,
    /// Size at which a channel's log is rotated to `<login>.jsonl.1`.
    pub max_bytes: u64,
}

impl Default for HistoryConfig {
    fn default() -> Self {
        Self {
            dir: Some(default_dir()),
            lines: 50,
            max_bytes: 16 * 1024 * 1024,
        }
    }
}

impl HistoryConfig {
    /// Keeps nothing on disk and restores nothing.
    pub fn disabled() -> Self {
        Self {
            dir: None,
            ..Self::default()
        }
    }
}

/// `$XDG_DATA_HOME/liveside/history`, or `~/.local/share/liveside/history`.
pub fn default_dir() -> PathBuf {
    let base = match std::env::var_os("XDG_DATA_HOME") {
        Some(data) if !data.is_empty() => PathBuf::from(data),
        _ => {
            let home = std::env::var_os("HOME").unwrap_or_default();
            PathBuf::from(home).join(".local").join("share")
        }
    };
    base.join("liveside").join("history")
}

/// Reads and writes the local logs.
#[derive(Debug, Clone)]
pub struct History {
    config: HistoryConfig,
}

impl History {
    pub fn new(config: HistoryConfig) -> Self {
        Self { config }
    }

    pub fn is_enabled(&self) -> bool {
        self.config.dir.is_some() && self.config.lines > 0
    }

    /// Appends one line to a channel's log. Problems are returned, not
    /// printed: the caller decides how to tell the user.
    pub fn append(&self, channel: &str, line: &Line) -> std::io::Result<()> {
        let Some(path) = self.path(channel) else {
            return Ok(());
        };
        if let Some(dir) = path.parent() {
            create_private_dir(dir)?;
        }
        let mut serialized = serde_json::to_vec(line).map_err(std::io::Error::other)?;
        serialized.push(b'\n');
        let mut file = OpenOptions::new()
            .create(true)
            .read(true)
            .append(true)
            .open(&path)
            .and_then(private_file)?;
        // A crash or a full disk mid-append leaves a line without its end.
        // Starting a fresh one keeps that damage to the cut line; written
        // straight after it, this line would be lost with it.
        if !ends_a_line(&mut file)? {
            serialized.insert(0, b'\n');
        }
        file.write_all(&serialized)?;
        if file.metadata()?.len() > self.config.max_bytes {
            // One generation is kept, so a channel costs at most twice the
            // limit and old talk eventually falls off the end.
            std::fs::rename(&path, rotated(&path))?;
        }
        Ok(())
    }

    /// The last lines of a channel's log, oldest first. Marked historical,
    /// whatever they were when they were written.
    ///
    /// The rotated generation is read first, so a rotation that just
    /// happened does not leave the channel looking empty.
    pub fn load(&self, channel: &str) -> std::io::Result<Vec<Line>> {
        let Some(path) = self.path(channel) else {
            return Ok(Vec::new());
        };
        if self.config.lines == 0 {
            return Ok(Vec::new());
        }
        let mut lines = read_tail(&rotated(&path))?;
        lines.append(&mut read_tail(&path)?);
        // The same message can be in the file twice: two engines on this
        // machine (a daemon and a frontend with its own engine) write to
        // it, and each appends what it saw.
        let mut seen = std::collections::HashSet::new();
        lines.reverse();
        lines.retain(|line| match &line.content {
            LineContent::Message(message) if !message.id.as_str().is_empty() => {
                seen.insert(message.id.clone())
            }
            _ => true,
        });
        lines.reverse();
        let keep = lines.len().saturating_sub(self.config.lines);
        Ok(lines.split_off(keep))
    }

    /// `<dir>/<channel>.jsonl`, or `None` when logging is off.
    ///
    /// Twitch logins are ASCII letters, digits and underscores; anything
    /// else would be a bug or a hostile name, and must not escape the
    /// directory.
    fn path(&self, channel: &str) -> Option<PathBuf> {
        let dir = self.config.dir.as_ref()?;
        if channel.is_empty()
            || !channel
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_')
        {
            return None;
        }
        Some(dir.join(format!("{channel}.jsonl")))
    }
}

/// Where a rotated log goes: `<login>.jsonl.1`.
fn rotated(path: &Path) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".1");
    path.with_file_name(name)
}

/// Whether the file is empty or ends with a newline. Appends go to the end
/// whatever the position, so the seek here does not move the next write.
fn ends_a_line(file: &mut File) -> std::io::Result<bool> {
    if file.metadata()?.len() == 0 {
        return Ok(true);
    }
    file.seek(SeekFrom::End(-1))?;
    let mut last = [0u8];
    file.read_exact(&mut last)?;
    Ok(last[0] == b'\n')
}

/// Parses the end of one log file, skipping anything unreadable.
fn read_tail(path: &Path) -> std::io::Result<Vec<Line>> {
    let mut file = match File::open(path) {
        Ok(file) => file,
        // Never written: no history, not an error.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };
    let len = file.metadata()?.len();
    let from = len.saturating_sub(TAIL_BYTES);
    file.seek(SeekFrom::Start(from))?;
    let mut lines = Vec::new();
    for (i, text) in BufReader::new(file).lines().enumerate() {
        let text = text?;
        // Seeking lands mid-line unless we started at the beginning.
        if i == 0 && from > 0 {
            continue;
        }
        // A half-written or newer-format line is skipped, not fatal.
        if let Ok(mut line) = serde_json::from_str::<Line>(&text) {
            line.flags.historical = true;
            lines.push(line);
        }
    }
    Ok(lines)
}

/// Chat logs are private: the directory is created as 0700.
fn create_private_dir(dir: &Path) -> std::io::Result<()> {
    if dir.is_dir() {
        return Ok(());
    }
    std::fs::create_dir_all(dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

/// ... and so are the files themselves: 0600.
fn private_file(file: File) -> std::io::Result<File> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = file.metadata()?.permissions();
        if permissions.mode() & 0o077 != 0 {
            permissions.set_mode(0o600);
            file.set_permissions(permissions)?;
        }
    }
    Ok(file)
}

#[cfg(test)]
mod tests {
    use super::*;
    use liveside_proto::ids::Timestamp;
    use liveside_proto::model::{LineContent, LineFlags, NoticeLevel};

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "liveside-history-test-{name}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    fn history(dir: &Path, lines: usize) -> History {
        History::new(HistoryConfig {
            dir: Some(dir.to_path_buf()),
            lines,
            max_bytes: 1024 * 1024,
        })
    }

    fn notice(text: &str) -> Line {
        Line {
            time: Timestamp(1),
            content: LineContent::Notice {
                level: NoticeLevel::Info,
                text: text.to_owned(),
                code: String::new(),
                args: Vec::new(),
            },
            flags: LineFlags::default(),
        }
    }

    fn message(id: &str, text: &str) -> liveside_proto::model::ChatMessage {
        use liveside_proto::model::{ChatMessage, Fragment, MessageKind, User};
        ChatMessage {
            id: id.into(),
            sender: User {
                id: "7".into(),
                login: "someone".into(),
                display_name: "Someone".into(),
            },
            color: None,
            badges: Vec::new(),
            fragments: vec![Fragment::Text {
                text: text.to_owned(),
            }],
            kind: MessageKind::Text,
            bits: None,
            reply: None,
            shared_source: None,
        }
    }

    fn texts(lines: &[Line]) -> Vec<String> {
        lines
            .iter()
            .map(|line| match &line.content {
                LineContent::Notice { text, .. } => text.clone(),
                other => panic!("unexpected content {other:?}"),
            })
            .collect()
    }

    #[test]
    fn writes_and_reads_back_the_tail() {
        let dir = temp_dir("roundtrip");
        let history = history(&dir, 3);
        for i in 0..5 {
            history
                .append("orbitcast", &notice(&format!("line {i}")))
                .unwrap();
        }
        let loaded = history.load("orbitcast").unwrap();
        assert_eq!(texts(&loaded), ["line 2", "line 3", "line 4"]);
        assert!(
            loaded.iter().all(|line| line.flags.historical),
            "loaded lines must be marked historical"
        );
        // Another channel is a separate file.
        assert!(history.load("someone_else").unwrap().is_empty());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_message_logged_twice_comes_back_once() {
        let dir = temp_dir("duplicates");
        let history = history(&dir, 10);
        let line = |id: &str, text: &str| {
            let mut line = notice(text);
            line.content = LineContent::Message(message(id, text));
            line
        };
        history.append("chan", &line("m1", "first")).unwrap();
        history.append("chan", &line("m2", "second")).unwrap();
        // As a second engine would have written it.
        history.append("chan", &line("m1", "first")).unwrap();

        let loaded = history.load("chan").unwrap();
        assert_eq!(loaded.len(), 2, "{loaded:?}");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_channel_never_seen_before_is_empty_not_an_error() {
        let dir = temp_dir("missing");
        assert!(history(&dir, 10).load("nobody").unwrap().is_empty());
    }

    #[test]
    fn damaged_lines_are_skipped() {
        let dir = temp_dir("damaged");
        let history = history(&dir, 10);
        history.append("chan", &notice("good one")).unwrap();
        // A half-written line, as a crash mid-append would leave behind.
        let mut file = OpenOptions::new()
            .append(true)
            .open(dir.join("chan.jsonl"))
            .unwrap();
        file.write_all(b"{\"time\":1,\"content\":{\"type\"\n")
            .unwrap();
        drop(file);
        history.append("chan", &notice("after the damage")).unwrap();
        assert_eq!(
            texts(&history.load("chan").unwrap()),
            ["good one", "after the damage"]
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_line_cut_off_mid_write_costs_only_itself() {
        let dir = temp_dir("cut");
        let history = history(&dir, 10);
        history.append("chan", &notice("good one")).unwrap();
        // What a crash or a full disk leaves: no newline at the end.
        let mut file = OpenOptions::new()
            .append(true)
            .open(dir.join("chan.jsonl"))
            .unwrap();
        file.write_all(b"{\"time\":1,\"content\":{\"type\"")
            .unwrap();
        drop(file);
        history.append("chan", &notice("after the cut")).unwrap();
        history.append("chan", &notice("and one more")).unwrap();
        assert_eq!(
            texts(&history.load("chan").unwrap()),
            ["good one", "after the cut", "and one more"]
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn rotates_when_the_log_grows_too_large() {
        let dir = temp_dir("rotate");
        let history = History::new(HistoryConfig {
            dir: Some(dir.clone()),
            lines: 10,
            max_bytes: 200,
        });
        for i in 0..20 {
            history
                .append("chan", &notice(&format!("line {i}")))
                .unwrap();
        }
        assert!(dir.join("chan.jsonl.1").exists(), "the old log is kept");
        let live = std::fs::metadata(dir.join("chan.jsonl"))
            .map(|m| m.len())
            .unwrap_or(0);
        assert!(live <= 200, "the live log stays under the limit");
        // Rotation must not make the channel look empty: the previous
        // generation is read too, and the newest lines survive.
        let loaded = texts(&history.load("chan").unwrap());
        assert!(
            loaded.contains(&"line 19".to_owned()),
            "expected the newest line in {loaded:?}"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn rejects_names_that_could_escape_the_directory() {
        let dir = temp_dir("escape");
        let history = history(&dir, 10);
        for bad in ["../secrets", "a/b", "", ".", "chan.jsonl"] {
            assert!(history.path(bad).is_none(), "{bad} must be refused");
            // Appending such a name writes nothing at all.
            history.append(bad, &notice("nope")).unwrap();
        }
        assert!(!dir.exists(), "nothing was written");
    }

    #[test]
    fn disabled_history_touches_no_disk() {
        let history = History::new(HistoryConfig::disabled());
        assert!(!history.is_enabled());
        history.append("chan", &notice("ignored")).unwrap();
        assert!(history.load("chan").unwrap().is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn logs_are_private() {
        use std::os::unix::fs::PermissionsExt;
        let dir = temp_dir("permissions");
        let history = history(&dir, 10);
        history.append("chan", &notice("private")).unwrap();
        let mode = |path: PathBuf| std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(dir.clone()), 0o700);
        assert_eq!(mode(dir.join("chan.jsonl")), 0o600);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
