//! Where the graphical client's channels were on screen: which were docked
//! beside the chat, in what order and how wide, and which were in windows
//! of their own and how big, so the window comes back as it was left.
//!
//! State, not a setting: it changes with every drag, so it is kept apart
//! from `config.toml`, which people edit and copy, in
//! `$XDG_STATE_HOME/liveside/layout.json`. A file that cannot be read is
//! reported and left out rather than stopping the window, as a broken
//! settings file does: losing where a pane was costs a drag, not a setting
//! that silently does not apply.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// `$XDG_STATE_HOME/liveside/layout.json`, or under `~/.local/state`. The
/// demo has a file of its own, so trying it out does not undo the real
/// window's layout.
pub fn default_path(demo: bool) -> PathBuf {
    let dir = match std::env::var_os("XDG_STATE_HOME") {
        Some(state) if !state.is_empty() => PathBuf::from(state),
        _ => {
            let home = std::env::var_os("HOME").unwrap_or_default();
            PathBuf::from(home).join(".local").join("state")
        }
    };
    let name = if demo {
        "layout-demo.json"
    } else {
        "layout.json"
    };
    dir.join("liveside").join(name)
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Layout {
    /// The channels docked beside the chat, left to right.
    pub docked: Vec<Docked>,
    /// The channels in windows of their own.
    pub windows: Vec<Popout>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Popout {
    /// The channel's login.
    pub channel: String,
    /// The window's size in pixels, as it was when the program closed. Not
    /// where it was: on Wayland a program cannot place its windows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<(u32, u32)>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Docked {
    /// The channel's login.
    pub channel: String,
    /// How much of the width it takes, in pixels, the handle before it
    /// included. None until it is dragged: then it gets a fair share.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<u32>,
}

impl Layout {
    /// The layout in `path`; empty if there is none, or if it cannot be
    /// read, which is said on stderr.
    pub fn load(path: &Path) -> Self {
        match std::fs::read_to_string(path) {
            Ok(text) => serde_json::from_str(&text).unwrap_or_else(|e| {
                eprintln!(
                    "liveside: {} could not be read, ignored: {e}",
                    path.display()
                );
                Self::default()
            }),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Self::default(),
            Err(e) => {
                eprintln!(
                    "liveside: {} could not be read, ignored: {e}",
                    path.display()
                );
                Self::default()
            }
        }
    }

    /// Writes it to `path`, whole or not at all: into a file beside it,
    /// then renamed over it.
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let text = serde_json::to_string_pretty(self).map_err(std::io::Error::other)?;
        let mut partial = path.as_os_str().to_owned();
        partial.push(".partial");
        std::fs::write(&partial, text + "\n")?;
        std::fs::rename(&partial, path)
    }

    fn index(&self, channel: &str) -> Option<usize> {
        self.docked
            .iter()
            .position(|docked| docked.channel.eq_ignore_ascii_case(channel))
    }

    pub fn is_docked(&self, channel: &str) -> bool {
        self.index(channel).is_some()
    }

    /// Docks a channel on the right of the others. Whether that changed
    /// anything.
    pub fn dock(&mut self, channel: &str) -> bool {
        if self.is_docked(channel) {
            return false;
        }
        // A channel is in one place at a time.
        self.close_window(channel);
        self.docked.push(Docked {
            channel: channel.to_owned(),
            width: None,
        });
        true
    }

    /// Whether it was docked.
    pub fn undock(&mut self, channel: &str) -> bool {
        match self.index(channel) {
            Some(at) => {
                self.docked.remove(at);
                true
            }
            None => false,
        }
    }

    pub fn width(&self, channel: &str) -> Option<u32> {
        self.index(channel).and_then(|at| self.docked[at].width)
    }

    /// Whether that changed anything, so a drag that ends where it started
    /// writes nothing.
    pub fn set_width(&mut self, channel: &str, width: u32) -> bool {
        match self.index(channel) {
            Some(at) if self.docked[at].width != Some(width) => {
                self.docked[at].width = Some(width);
                true
            }
            _ => false,
        }
    }

    fn window_index(&self, channel: &str) -> Option<usize> {
        self.windows
            .iter()
            .position(|window| window.channel.eq_ignore_ascii_case(channel))
    }

    pub fn is_window(&self, channel: &str) -> bool {
        self.window_index(channel).is_some()
    }

    /// Puts a channel in a window of its own. Whether that changed
    /// anything.
    pub fn open_window(&mut self, channel: &str) -> bool {
        if self.is_window(channel) {
            return false;
        }
        self.undock(channel);
        self.windows.push(Popout {
            channel: channel.to_owned(),
            size: None,
        });
        true
    }

    /// Whether it was in one.
    pub fn close_window(&mut self, channel: &str) -> bool {
        match self.window_index(channel) {
            Some(at) => {
                self.windows.remove(at);
                true
            }
            None => false,
        }
    }

    pub fn window_size(&self, channel: &str) -> Option<(u32, u32)> {
        self.window_index(channel)
            .and_then(|at| self.windows[at].size)
    }

    /// Whether that changed anything.
    pub fn set_window_size(&mut self, channel: &str, size: (u32, u32)) -> bool {
        match self.window_index(channel) {
            Some(at) if self.windows[at].size != Some(size) => {
                self.windows[at].size = Some(size);
                true
            }
            _ => false,
        }
    }

    /// Where a channel goes among those on screen already, `shown` left to
    /// right: after every one the layout has before it. Channels are
    /// joined in no fixed order, so the panes come back in the order they
    /// were left in only if each is put in its place as it arrives.
    pub fn place<S: AsRef<str>>(&self, channel: &str, shown: &[S]) -> usize {
        let Some(at) = self.index(channel) else {
            return shown.len();
        };
        shown
            .iter()
            .filter(|other| self.index(other.as_ref()).is_some_and(|i| i < at))
            .count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "liveside-layout-test-{name}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn writes_and_reads_back() {
        let dir = temp_dir("roundtrip");
        let path = dir.join("nested").join("layout.json");
        let mut layout = Layout::default();
        layout.dock("alpha");
        layout.dock("beta");
        layout.set_width("beta", 320);
        layout.save(&path).unwrap();
        assert_eq!(Layout::load(&path), layout);
        // Nothing left half-written beside it.
        let names: Vec<_> = std::fs::read_dir(path.parent().unwrap())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(names, ["layout.json"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_or_broken_file_is_empty() {
        let dir = temp_dir("broken");
        let path = dir.join("layout.json");
        assert_eq!(Layout::load(&path), Layout::default());
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(&path, "{ not json").unwrap();
        assert_eq!(Layout::load(&path), Layout::default());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn docks_once_and_forgets_on_undock() {
        let mut layout = Layout::default();
        assert!(layout.dock("alpha"));
        assert!(!layout.dock("Alpha"));
        assert!(layout.set_width("alpha", 300));
        assert!(!layout.set_width("alpha", 300));
        assert!(!layout.set_width("beta", 300));
        assert_eq!(layout.width("ALPHA"), Some(300));
        assert!(layout.undock("alpha"));
        assert!(!layout.undock("alpha"));
        assert!(!layout.is_docked("alpha"));
    }

    #[test]
    fn panes_come_back_in_order_whatever_joins_first() {
        let mut layout = Layout::default();
        for channel in ["a", "b", "c"] {
            layout.dock(channel);
        }
        let mut shown: Vec<&str> = Vec::new();
        for channel in ["c", "a", "b"] {
            let at = layout.place(channel, &shown);
            shown.insert(at, channel);
        }
        assert_eq!(shown, ["a", "b", "c"]);
        // One docked since goes on the right.
        assert_eq!(layout.place("d", &shown), 3);
    }

    #[test]
    fn windows_are_kept_with_their_size() {
        let dir = temp_dir("windows");
        let path = dir.join("layout.json");
        let mut layout = Layout::default();
        assert!(layout.open_window("alpha"));
        assert!(!layout.open_window("ALPHA"));
        assert!(layout.set_window_size("alpha", (480, 640)));
        assert!(!layout.set_window_size("alpha", (480, 640)));
        assert!(!layout.set_window_size("beta", (480, 640)));
        layout.save(&path).unwrap();
        let back = Layout::load(&path);
        assert_eq!(back.window_size("alpha"), Some((480, 640)));
        assert!(layout.close_window("alpha"));
        assert!(!layout.is_window("alpha"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_channel_is_in_one_place() {
        let mut layout = Layout::default();
        layout.dock("alpha");
        layout.open_window("alpha");
        assert!(!layout.is_docked("alpha"));
        layout.dock("alpha");
        assert!(!layout.is_window("alpha"));
    }

    #[test]
    fn a_file_from_before_windows_still_reads() {
        let dir = temp_dir("older");
        let path = dir.join("layout.json");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(&path, r#"{"docked":[{"channel":"alpha","width":300}]}"#).unwrap();
        let layout = Layout::load(&path);
        assert_eq!(layout.width("alpha"), Some(300));
        assert!(layout.windows.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_demo_keeps_its_own_file() {
        assert_ne!(default_path(true), default_path(false));
    }
}
