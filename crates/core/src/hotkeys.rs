//! Hotkeys: a key that runs a line as if it were typed, one binding a
//! line in the settings:
//!
//! ```text
//! F5 /info
//! Ctrl+Shift+B /ban $$1 spam
//! Alt+H Ahoj $(chan)!
//! ```
//!
//! What follows the key is read like a custom command's body (see
//! [`crate::commands`]): `$1` is the user selected where the key is
//! pressed, the same as Chatty's hotkey context, and the `$(chan)` family
//! is there too. The line it makes then goes where a typed one goes, so a
//! hotkey can run a custom command.
//!
//! Function keys take any modifiers; a letter or a digit needs Ctrl or
//! Alt, so a binding can never take a key away from typing. F2 is the
//! settings in both frontends and cannot be bound: a binding there could
//! lock someone out of the place to undo it.

use std::fmt;

use crate::commands::{CommandProblem, Context, CustomCommand};

/// A key with its modifiers, as a frontend reports it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Key {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub code: KeyCode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum KeyCode {
    /// F1 to F12.
    F(u8),
    /// A letter (lowercase) or a digit.
    Char(char),
}

impl fmt::Display for Key {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.ctrl {
            f.write_str("Ctrl+")?;
        }
        if self.alt {
            f.write_str("Alt+")?;
        }
        if self.shift {
            f.write_str("Shift+")?;
        }
        match self.code {
            KeyCode::F(n) => write!(f, "F{n}"),
            KeyCode::Char(c) => write!(f, "{}", c.to_ascii_uppercase()),
        }
    }
}

fn problem(code: &'static str, args: &[(&'static str, &str)], message: String) -> CommandProblem {
    CommandProblem {
        code,
        args: args.iter().map(|(k, v)| (*k, (*v).to_owned())).collect(),
        message,
    }
}

impl Key {
    /// Reads `Ctrl+Shift+F5`, `Alt+H`, `F1`: modifiers in any order, any
    /// case.
    pub fn parse(written: &str) -> Result<Key, CommandProblem> {
        let bad = || {
            problem(
                "hotkey.key",
                &[("key", written)],
                format!(
                    "“{written}” is not a key this takes: F1 to F12, or Ctrl or Alt with a \
                     letter or digit"
                ),
            )
        };
        let mut key = Key {
            ctrl: false,
            alt: false,
            shift: false,
            code: KeyCode::F(0),
        };
        let mut parts: Vec<&str> = written.split('+').map(str::trim).collect();
        let name = parts.pop().filter(|n| !n.is_empty()).ok_or_else(bad)?;
        for modifier in parts {
            match modifier.to_ascii_lowercase().as_str() {
                "ctrl" | "control" => key.ctrl = true,
                "alt" => key.alt = true,
                "shift" => key.shift = true,
                _ => return Err(bad()),
            }
        }
        let lower = name.to_ascii_lowercase();
        key.code = match lower.strip_prefix('f').map(str::parse::<u8>) {
            Some(Ok(n)) if (1..=12).contains(&n) => KeyCode::F(n),
            _ => {
                let mut chars = lower.chars();
                match (chars.next(), chars.next()) {
                    (Some(c), None) if c.is_ascii_alphanumeric() => KeyCode::Char(c),
                    _ => return Err(bad()),
                }
            }
        };
        if matches!(key.code, KeyCode::Char(_)) && !key.ctrl && !key.alt {
            return Err(problem(
                "hotkey.typing",
                &[("key", written)],
                format!("“{written}” is a key for typing; add Ctrl or Alt"),
            ));
        }
        if key.code == KeyCode::F(2) && !key.ctrl && !key.alt && !key.shift {
            return Err(problem(
                "hotkey.reserved",
                &[("key", written)],
                format!("{written} opens the settings and cannot be bound"),
            ));
        }
        Ok(key)
    }
}

/// One binding, read.
#[derive(Debug, Clone)]
pub struct Hotkey {
    pub key: Key,
    pub source: String,
    command: CustomCommand,
}

impl Hotkey {
    /// Reads `KEY what to do`; `None` for a comment or nothing.
    pub fn parse(line: &str) -> Result<Option<Hotkey>, CommandProblem> {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            return Ok(None);
        }
        let (key, body) = line.split_once(char::is_whitespace).unwrap_or((line, ""));
        let key = Key::parse(key)?;
        if body.trim().is_empty() {
            return Err(problem(
                "hotkey.empty",
                &[("key", &key.to_string())],
                format!("{key} needs something to do"),
            ));
        }
        // Read as a custom command's body, under a name of its own.
        let command =
            CustomCommand::parse(&format!("/hotkey {}", body.trim()))?.expect("a body was given");
        Ok(Some(Hotkey {
            key,
            source: line.to_owned(),
            command,
        }))
    }

    /// The line it makes, with the selected user as `$1`.
    pub fn run(&self, selected: Option<&str>, context: &Context) -> Result<String, CommandProblem> {
        self.command
            .run(selected.unwrap_or_default(), context)
            .map_err(|e| match e.code {
                // Said by the key, not by the name it runs under inside.
                "custom.missing" => {
                    let value = e
                        .args
                        .iter()
                        .find(|(k, _)| *k == "value")
                        .map(|(_, v)| v.clone())
                        .unwrap_or_default();
                    let key = self.key.to_string();
                    problem(
                        "hotkey.missing",
                        &[("key", &key), ("value", &value)],
                        format!("{key} needs {value}"),
                    )
                }
                _ => e,
            })
    }
}

/// The bindings, read.
#[derive(Debug, Clone, Default)]
pub struct Hotkeys {
    list: Vec<Hotkey>,
}

/// Which line is wrong, and how.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HotkeyProblem {
    pub number: usize,
    pub line: String,
    pub problem: CommandProblem,
}

impl fmt::Display for HotkeyProblem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "hotkey {} ({}): {}",
            self.number, self.line, self.problem
        )
    }
}

impl Hotkeys {
    /// Every problem is reported, a key bound twice included.
    pub fn read(lines: &[String]) -> Result<Self, Vec<HotkeyProblem>> {
        let mut list: Vec<Hotkey> = Vec::new();
        let mut problems = Vec::new();
        for (i, line) in lines.iter().enumerate() {
            let read = Hotkey::parse(line).and_then(|hotkey| match hotkey {
                Some(hotkey) if list.iter().any(|h| h.key == hotkey.key) => Err(problem(
                    "hotkey.twice",
                    &[("key", &hotkey.key.to_string())],
                    format!("{} is bound twice", hotkey.key),
                )),
                other => Ok(other),
            });
            match read {
                Ok(Some(hotkey)) => list.push(hotkey),
                Ok(None) => {}
                Err(problem) => problems.push(HotkeyProblem {
                    number: i + 1,
                    line: line.clone(),
                    problem,
                }),
            }
        }
        match problems.is_empty() {
            true => Ok(Hotkeys { list }),
            false => Err(problems),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.list.is_empty()
    }

    pub fn find(&self, key: &Key) -> Option<&Hotkey> {
        self.list.iter().find(|h| &h.key == key)
    }

    pub fn keys(&self) -> Vec<String> {
        self.list.iter().map(|h| h.key.to_string()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(ctrl: bool, alt: bool, shift: bool, code: KeyCode) -> Key {
        Key {
            ctrl,
            alt,
            shift,
            code,
        }
    }

    #[test]
    fn keys_are_read_in_any_order_and_case() {
        assert_eq!(
            Key::parse("F5").unwrap(),
            key(false, false, false, KeyCode::F(5))
        );
        assert_eq!(
            Key::parse("shift+CTRL+b").unwrap(),
            key(true, false, true, KeyCode::Char('b'))
        );
        assert_eq!(
            Key::parse("Alt+7").unwrap(),
            key(false, true, false, KeyCode::Char('7'))
        );
        assert_eq!(
            Key::parse("Ctrl+Shift+B").unwrap().to_string(),
            "Ctrl+Shift+B"
        );
        let code = |k: &str| Key::parse(k).unwrap_err().code;
        assert_eq!(code("B"), "hotkey.typing");
        assert_eq!(code("Shift+B"), "hotkey.typing");
        assert_eq!(code("F13"), "hotkey.key");
        assert_eq!(code("Super+B"), "hotkey.key");
        assert_eq!(code("Ctrl+Enter"), "hotkey.key");
        assert_eq!(code("F2"), "hotkey.reserved");
        assert!(Key::parse("Ctrl+F2").is_ok());
    }

    #[test]
    fn a_binding_runs_its_line_with_the_selected_user() {
        let hotkeys = Hotkeys::read(&[
            "# a comment".to_owned(),
            "F5 /info".to_owned(),
            "Ctrl+Shift+B /ban $$1 spam in $(chan)".to_owned(),
        ])
        .unwrap();
        let context = Context {
            channel: Some("harborlight".into()),
            ..Context::default()
        };
        let ban = hotkeys
            .find(&key(true, false, true, KeyCode::Char('b')))
            .unwrap();
        assert_eq!(
            ban.run(Some("troll"), &context).unwrap(),
            "/ban troll spam in harborlight"
        );
        let missing = ban.run(None, &context).unwrap_err();
        assert_eq!(
            (missing.code, missing.message.as_str()),
            ("hotkey.missing", "Ctrl+Shift+B needs $1")
        );
        assert_eq!(
            hotkeys
                .find(&key(false, false, false, KeyCode::F(5)))
                .unwrap()
                .run(None, &context)
                .unwrap(),
            "/info"
        );
        assert!(
            hotkeys
                .find(&key(false, false, false, KeyCode::F(6)))
                .is_none()
        );
        assert_eq!(hotkeys.keys(), ["F5", "Ctrl+Shift+B"]);
    }

    #[test]
    fn every_wrong_line_is_said_with_its_place() {
        let problems = Hotkeys::read(&[
            "F5 /info".to_owned(),
            "F5 /other".to_owned(),
            "B nope".to_owned(),
            "F6".to_owned(),
            "F7 $(nope)".to_owned(),
        ])
        .unwrap_err();
        let found: Vec<(usize, &str)> = problems
            .iter()
            .map(|p| (p.number, p.problem.code))
            .collect();
        assert_eq!(
            found,
            [
                (2, "hotkey.twice"),
                (3, "hotkey.typing"),
                (4, "hotkey.empty"),
                (5, "custom.identifier"),
            ]
        );
    }
}
