//! The emotes picked or sent last, for the top of the list to pick from.
//!
//! State, like the layout: it changes all the time and is nobody's
//! setting, so it is kept in `$XDG_STATE_HOME/liveside/recent-emotes.json`
//! rather than in `config.toml`. A file that cannot be read starts the
//! list afresh; losing it costs a few clicks.

use std::path::{Path, PathBuf};

use liveside_proto::model::EmoteRef;

/// How many are kept: about two rows of the list.
pub const KEPT: usize = 24;

/// `$XDG_STATE_HOME/liveside/recent-emotes.json`, or under `~/.local/state`.
/// The demo keeps its own, so trying it out leaves the real list alone.
pub fn default_path(demo: bool) -> PathBuf {
    let dir = match std::env::var_os("XDG_STATE_HOME") {
        Some(state) if !state.is_empty() => PathBuf::from(state),
        _ => {
            let home = std::env::var_os("HOME").unwrap_or_default();
            PathBuf::from(home).join(".local").join("state")
        }
    };
    let name = if demo {
        "recent-emotes-demo.json"
    } else {
        "recent-emotes.json"
    };
    dir.join("liveside").join(name)
}

/// The list as it was saved, newest first; empty when there is none.
pub fn load(path: &Path) -> Vec<EmoteRef> {
    std::fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

/// Writes the list, replacing the file whole.
pub fn save(path: &Path, recent: &[EmoteRef]) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let partial = path.with_extension("json.partial");
    std::fs::write(&partial, serde_json::to_vec(recent)?)?;
    std::fs::rename(partial, path)
}

/// Puts `emote` first, once, and keeps no more than [`KEPT`].
pub fn remember(recent: &mut Vec<EmoteRef>, emote: &EmoteRef) {
    recent.retain(|known| !(known.provider == emote.provider && known.id == emote.id));
    recent.insert(0, emote.clone());
    recent.truncate(KEPT);
}

#[cfg(test)]
mod tests {
    use super::*;
    use liveside_proto::model::EmoteProvider;

    fn emote(id: &str) -> EmoteRef {
        EmoteRef {
            provider: EmoteProvider::Twitch,
            id: id.to_owned(),
            code: format!("code{id}"),
            animated: false,
            zero_width: false,
        }
    }

    #[test]
    fn the_newest_comes_first_once_and_the_list_stays_short() {
        let mut recent = Vec::new();
        for i in 0..30 {
            remember(&mut recent, &emote(&i.to_string()));
        }
        assert_eq!(recent.len(), KEPT);
        assert_eq!(recent[0].id, "29");
        remember(&mut recent, &emote("20"));
        assert_eq!(recent[0].id, "20");
        assert_eq!(recent.iter().filter(|e| e.id == "20").count(), 1);
    }

    #[test]
    fn the_list_is_read_back_as_written() {
        let path = std::env::temp_dir().join(format!(
            "liveside-recent-emotes-{}.json",
            std::process::id()
        ));
        assert!(!path.exists());
        assert!(load(&path).is_empty(), "no file is an empty list");
        let recent = vec![emote("1"), emote("2")];
        save(&path, &recent).unwrap();
        assert_eq!(load(&path), recent);
        std::fs::write(&path, "not json").unwrap();
        assert!(load(&path).is_empty(), "a broken file starts afresh");
        std::fs::remove_file(&path).unwrap();
    }
}
