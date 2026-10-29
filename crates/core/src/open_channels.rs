//! The channels that were open when Liveside last ran, so it can open them
//! again at the next start.
//!
//! State, not a setting: it changes with every join, so it is kept apart
//! from `config.toml`, in `$XDG_STATE_HOME/liveside/channels.json`, beside
//! the window's layout. Only logins are written, and only with
//! `chat.reopen_channels` on; the file says which channels are read, so it
//! is the user's alone (0600). A file that cannot be read opens nothing
//! rather than stopping the start.

use std::io::Write;
use std::path::{Path, PathBuf};

/// `$XDG_STATE_HOME/liveside/channels.json`, or under `~/.local/state`.
/// The demo has a file of its own, so trying it out does not change what
/// the real client opens.
pub fn default_path(demo: bool) -> PathBuf {
    let dir = match std::env::var_os("XDG_STATE_HOME") {
        Some(state) if !state.is_empty() => PathBuf::from(state),
        _ => {
            let home = std::env::var_os("HOME").unwrap_or_default();
            PathBuf::from(home).join(".local").join("state")
        }
    };
    let name = if demo {
        "channels-demo.json"
    } else {
        "channels.json"
    };
    dir.join("liveside").join(name)
}

/// The logins in the file, in the order they were joined. Nothing when
/// there is no file yet, or one that cannot be read.
pub fn load(path: &Path) -> Vec<String> {
    std::fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

/// Writes the logins, replacing the file whole: written beside it and
/// renamed over it, so a crash leaves the old list or the new one.
pub fn save(path: &Path, logins: &[&str]) -> std::io::Result<()> {
    use std::os::unix::fs::OpenOptionsExt;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let partial = path.with_extension("json.partial");
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&partial)?;
    file.write_all(&serde_json::to_vec(logins).map_err(std::io::Error::other)?)?;
    file.sync_all()?;
    std::fs::rename(&partial, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "liveside-open-channels-test-{name}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn what_was_saved_is_loaded_in_order() {
        let dir = temp_dir("order");
        let path = dir.join("liveside").join("channels.json");
        assert!(load(&path).is_empty(), "no file opens nothing");

        save(&path, &["tidewave", "orbitcast"]).unwrap();
        assert_eq!(load(&path), ["tidewave", "orbitcast"]);

        save(&path, &[]).unwrap();
        assert!(load(&path).is_empty());

        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_broken_file_opens_nothing() {
        let dir = temp_dir("broken");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("channels.json");
        std::fs::write(&path, "[\"a\"").unwrap();
        assert!(load(&path).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
