//! The user's own bots, as both frontends edit them: a bot's command is
//! typed as one line and kept as a list of words, so a path with a space
//! in it survives being typed, saved and shown again.

use liveside_proto::model::BotState;

/// Splits a typed command into words: spaces separate them, quotes
/// (`"…"` or `'…'`) keep spaces inside a word, and a backslash takes the
/// next character as it is. Nothing else is interpreted; the words are
/// run as they are, not by a shell.
pub fn split_command(line: &str) -> Result<Vec<String>, String> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut in_word = false;
    let mut quote: Option<char> = None;
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (None, '"' | '\'') => {
                quote = Some(c);
                in_word = true;
            }
            (_, '\\') => match chars.next() {
                Some(next) => {
                    word.push(next);
                    in_word = true;
                }
                None => return Err("a backslash at the end".to_owned()),
            },
            (None, c) if c.is_whitespace() => {
                if in_word {
                    words.push(std::mem::take(&mut word));
                    in_word = false;
                }
            }
            (_, c) => {
                word.push(c);
                in_word = true;
            }
        }
    }
    if let Some(q) = quote {
        return Err(format!("an unclosed {q}"));
    }
    if in_word {
        words.push(word);
    }
    Ok(words)
}

/// The words as a line that `split_command` reads back the same.
pub fn join_command(words: &[String]) -> String {
    words
        .iter()
        .map(|word| {
            let plain = !word.is_empty()
                && !word
                    .chars()
                    .any(|c| c.is_whitespace() || matches!(c, '"' | '\'' | '\\'));
            if plain {
                word.clone()
            } else {
                let escaped = word.replace('\\', "\\\\").replace('"', "\\\"");
                format!("\"{escaped}\"")
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Channels as typed: separated by spaces or commas, `#` or not.
pub fn split_channels(line: &str) -> Vec<String> {
    line.split(|c: char| c.is_whitespace() || c == ',')
        .map(|channel| channel.trim_start_matches('#').to_lowercase())
        .filter(|channel| !channel.is_empty())
        .collect()
}

/// How a bot stands, in the reader's language: "crashed (exit code 1)".
pub fn state_said(strings: &crate::lang::Strings, state: &BotState) -> String {
    use crate::lang::fill;
    match state {
        BotState::Running => strings.bots_running.to_owned(),
        BotState::Stopped => strings.bots_stopped.to_owned(),
        BotState::Ended { code: Some(0) } => strings.bots_finished.to_owned(),
        BotState::Ended { code: Some(code) } => {
            fill(strings.bots_crashed, &[("code", &code.to_string())])
        }
        BotState::Ended { code: None } => strings.bots_killed.to_owned(),
        BotState::Failed { .. } => strings.bots_failed.to_owned(),
        BotState::Elsewhere => strings.bots_elsewhere.to_owned(),
    }
}

/// Whether a state is one to tell the reader about where they are: the
/// bot fell over or would not start.
pub fn went_wrong(state: &BotState) -> bool {
    matches!(state, BotState::Ended { code } if *code != Some(0))
        || matches!(state, BotState::Failed { .. })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn words_split_at_spaces_and_quotes_keep_them() {
        assert_eq!(
            split_command(r#"python3  "/home/me/my bots/weather.py" --city 'Nové Město' a\ b"#)
                .unwrap(),
            [
                "python3",
                "/home/me/my bots/weather.py",
                "--city",
                "Nové Město",
                "a b"
            ]
        );
        assert_eq!(split_command(r#"echo """#).unwrap(), ["echo", ""]);
        assert!(split_command("echo \"open").is_err());
        assert!(split_command("echo \\").is_err());
        assert!(split_command("   ").unwrap().is_empty());
    }

    #[test]
    fn a_command_reads_back_as_it_was() {
        for words in [
            vec!["python3".to_owned(), "/home/me/bot.py".to_owned()],
            vec![
                "sh".to_owned(),
                "-c".to_owned(),
                r#"echo "hi" \ 'x'"#.to_owned(),
            ],
            vec!["prog".to_owned(), String::new(), "a b".to_owned()],
        ] {
            assert_eq!(split_command(&join_command(&words)).unwrap(), words);
        }
        assert_eq!(
            join_command(&["python3".to_owned(), "bot.py".to_owned()]),
            "python3 bot.py"
        );
    }

    #[test]
    fn channels_are_logins() {
        assert_eq!(
            split_channels("#BrightMaple, harborlight  #x"),
            ["brightmaple", "harborlight", "x"]
        );
    }
}
