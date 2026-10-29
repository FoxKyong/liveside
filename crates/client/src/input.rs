//! The line being typed: what was sent before, and Tab completion.
//!
//! Both frontends edit text in their own way — the terminal keeps a string
//! and a cursor, the window a `GtkEntry` — but walking back through sent
//! lines and cycling through completions should feel the same in both, so
//! the rules live here and work on plain text and a byte cursor.

/// Lines of input remembered. Chatty keeps everything; this is plenty for
/// repeating a command or fixing a typo.
const REMEMBERED: usize = 200;

/// Lines sent from one frontend, walked with Up and Down.
#[derive(Debug, Default)]
pub struct History {
    /// Oldest first.
    lines: Vec<String>,
    /// Where Up/Down are in `lines`; `None` means the line being typed.
    pos: Option<usize>,
    /// The line that was being typed when the user started walking back,
    /// so Down can bring it back.
    draft: String,
}

impl History {
    /// Remembers a sent line and returns to the newest position.
    pub fn remember(&mut self, text: &str) {
        self.pos = None;
        self.draft.clear();
        // Repeating the same line twice is not worth two entries.
        if self.lines.last().map(String::as_str) == Some(text) {
            return;
        }
        self.lines.push(text.to_owned());
        if self.lines.len() > REMEMBERED {
            self.lines.remove(0);
        }
    }

    /// Walks back through what was sent. `typed` is the line as it stands,
    /// kept for the way down. Returns the line to show, or `None` when
    /// there is nothing further back.
    pub fn earlier(&mut self, typed: &str) -> Option<String> {
        let next = match self.pos {
            _ if self.lines.is_empty() => return None,
            None => {
                self.draft = typed.to_owned();
                self.lines.len() - 1
            }
            Some(0) => return None,
            Some(pos) => pos - 1,
        };
        self.pos = Some(next);
        Some(self.lines[next].clone())
    }

    /// ... and forward again, ending at the line that was being typed.
    /// `None` when not walking at all.
    pub fn later(&mut self) -> Option<String> {
        let pos = self.pos?;
        if pos + 1 < self.lines.len() {
            self.pos = Some(pos + 1);
            Some(self.lines[pos + 1].clone())
        } else {
            self.pos = None;
            Some(std::mem::take(&mut self.draft))
        }
    }

    /// Forgets where Up/Down were, for a line cleared by hand.
    pub fn reset(&mut self) {
        self.pos = None;
        self.draft.clear();
    }
}

/// The text and cursor after a completion step.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Completed {
    pub text: String,
    /// Byte index of the cursor in `text`.
    pub cursor: usize,
}

/// One Tab cycle: the word being completed and what it can become.
#[derive(Debug, Default)]
pub struct Completion {
    running: Option<Cycle>,
}

#[derive(Debug)]
struct Cycle {
    /// Byte index where the completed word starts.
    start: usize,
    candidates: Vec<String>,
    index: usize,
    /// What the text looks like after the last insertion, to notice that
    /// the user typed something in between.
    text: String,
    /// What follows a candidate: a name starting the line is addressed
    /// with a colon, anything else is followed by a space.
    addressing: bool,
}

/// What a completion step did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    /// The word was replaced; show this.
    Changed(Completed),
    /// There was a word, but nothing matches it: leave it alone.
    NoMatch,
    /// Nothing typed before the cursor, so the key keeps its other meaning.
    Nothing,
}

impl Completion {
    /// Ends a running cycle; any key but Tab does that.
    pub fn stop(&mut self) {
        self.running = None;
    }

    /// Completes the word before `cursor` (a byte index), or moves on to
    /// the next candidate when the same key is pressed again. `direction`
    /// is 1 for Tab and -1 for Shift-Tab.
    ///
    /// A word starting the line with `/` is completed from `commands`;
    /// anything else from `candidates`, which is asked for only when a new
    /// cycle starts, with the typed word (an `@` taken off).
    pub fn step(
        &mut self,
        text: &str,
        cursor: usize,
        direction: isize,
        commands: &[&str],
        candidates: impl FnOnce(&str) -> Vec<String>,
    ) -> Step {
        // A cycle in progress: the text is still what we left behind.
        if let Some(mut running) = self.running.take()
            && running.text == text
            && !running.candidates.is_empty()
        {
            let len = running.candidates.len() as isize;
            running.index = (running.index as isize + direction).rem_euclid(len) as usize;
            return Step::Changed(self.apply(running, text, cursor));
        }

        let before = &text[..cursor];
        let start = before.rfind(' ').map_or(0, |i| i + 1);
        let word = &before[start..];
        if word.is_empty() {
            return Step::Nothing;
        }
        let mut addressing = start == 0;
        let found = if start == 0
            && let Some(prefix) = word.strip_prefix('/')
        {
            addressing = false;
            let prefix = prefix.to_ascii_lowercase();
            commands
                .iter()
                .filter(|name| name.starts_with(&prefix))
                .map(|name| format!("/{name}"))
                .collect()
        } else if let Some(code) = word.strip_prefix(':')
            && code.trim_end_matches(':').chars().count() >= 2
        {
            // `:sm` becomes the emoji itself; the shortcode is only a way
            // of typing it, and is not what anybody reads.
            addressing = false;
            emoji(code.trim_end_matches(':'))
        } else {
            candidates(word.strip_prefix('@').unwrap_or(word))
        };
        if found.is_empty() {
            return Step::NoMatch;
        }
        let index = if direction < 0 { found.len() - 1 } else { 0 };
        let cycle = Cycle {
            start,
            candidates: found,
            index,
            text: String::new(),
            addressing,
        };
        Step::Changed(self.apply(cycle, text, cursor))
    }

    fn apply(&mut self, mut running: Cycle, text: &str, cursor: usize) -> Completed {
        let candidate = &running.candidates[running.index];
        // Addressing someone at the start of a line ends in a colon, as in
        // every other chat client; anything else just gets a space.
        let suffix = if running.addressing { ": " } else { " " };
        let replacement = format!("{candidate}{suffix}");
        let mut text = text.to_owned();
        text.replace_range(running.start..cursor, &replacement);
        let cursor = running.start + replacement.len();
        running.text = text.clone();
        self.running = Some(running);
        Completed { text, cursor }
    }
}

/// Emoji whose shortcode (GitHub's, as in `:smile:`) starts with `prefix`:
/// an exact match first, then the shortest codes, each emoji once.
pub fn emoji(prefix: &str) -> Vec<String> {
    /// Enough to cycle through, and a short list to look at.
    const MOST: usize = 50;
    let prefix = prefix.to_lowercase();
    let mut found: Vec<(bool, usize, &str, &str)> = emojis::iter()
        .filter_map(|emoji| {
            // The best of an emoji's codes: the exact one, else the shortest.
            emoji
                .shortcodes()
                .filter(|code| code.starts_with(&prefix))
                .map(|code| (code != prefix, code.len(), code, emoji.as_str()))
                .min()
        })
        .collect();
    found.sort();
    found
        .into_iter()
        .take(MOST)
        .map(|(_, _, _, emoji)| emoji.to_owned())
        .collect()
}

/// Names and emote codes matching `prefix` case-insensitively, names first,
/// each once. `said` gives each message's sender and the emote codes in it,
/// newest message first, the way one usually wants to answer the person
/// who just wrote.
pub fn names_and_emotes<'a>(
    prefix: &str,
    said: impl IntoIterator<Item = (&'a str, Vec<&'a str>)>,
) -> Vec<String> {
    let prefix = prefix.to_lowercase();
    let mut names: Vec<String> = Vec::new();
    let mut emotes: Vec<String> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for (name, codes) in said {
        if name.to_lowercase().starts_with(&prefix) && seen.insert(name.to_owned()) {
            names.push(name.to_owned());
        }
        for code in codes {
            if code.to_lowercase().starts_with(&prefix) && seen.insert(code.to_owned()) {
                emotes.push(code.to_owned());
            }
        }
    }
    // Names first: answering someone is the common case.
    names.append(&mut emotes);
    names
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn history_walks_back_and_returns_the_draft() {
        let mut history = History::default();
        history.remember("one");
        history.remember("two");
        history.remember("two");
        assert_eq!(history.earlier("typing").as_deref(), Some("two"));
        assert_eq!(history.earlier("two").as_deref(), Some("one"));
        assert_eq!(history.earlier("one"), None);
        assert_eq!(history.later().as_deref(), Some("two"));
        assert_eq!(history.later().as_deref(), Some("typing"));
        assert_eq!(history.later(), None);
    }

    #[test]
    fn empty_history_gives_nothing() {
        let mut history = History::default();
        assert_eq!(history.earlier("x"), None);
        assert_eq!(history.later(), None);
    }

    fn people(prefix: &str) -> Vec<String> {
        names_and_emotes(
            prefix,
            [
                ("Alice", vec!["Kappa"]),
                ("albert", vec![]),
                ("Alice", vec![]),
            ],
        )
    }

    #[test]
    fn completes_a_name_at_the_start_with_a_colon_and_cycles() {
        let mut completion = Completion::default();
        let Step::Changed(first) = completion.step("al", 2, 1, &[], people) else {
            panic!("expected a completion");
        };
        assert_eq!(first.text, "Alice: ");
        let Step::Changed(second) = completion.step(&first.text, first.cursor, 1, &[], people)
        else {
            panic!("expected the next one");
        };
        assert_eq!(second.text, "albert: ");
        let Step::Changed(back) = completion.step(&second.text, second.cursor, -1, &[], people)
        else {
            panic!("expected the previous one");
        };
        assert_eq!(back.text, "Alice: ");
    }

    #[test]
    fn completes_inside_a_line_with_a_space_and_drops_the_at() {
        let mut completion = Completion::default();
        let text = "hi @ka there";
        let Step::Changed(done) = completion.step(text, 6, 1, &[], people) else {
            panic!("expected a completion");
        };
        assert_eq!(done.text, "hi Kappa  there");
        assert_eq!(done.cursor, 9);
    }

    #[test]
    fn completes_commands_only_at_the_start() {
        let mut completion = Completion::default();
        let commands = ["ban", "timeout", "unban"];
        let Step::Changed(done) = completion.step("/ba", 3, 1, &commands, people) else {
            panic!("expected a command");
        };
        assert_eq!(done.text, "/ban ");
        completion.stop();
        assert_eq!(
            completion.step("say /ba", 7, 1, &commands, people),
            Step::NoMatch
        );
    }

    #[test]
    fn typing_in_between_starts_a_new_cycle() {
        let mut completion = Completion::default();
        let Step::Changed(done) = completion.step("al", 2, 1, &[], people) else {
            panic!("expected a completion");
        };
        // "Alice: a" is a new word, so the cycle starts over from it.
        let edited = format!("{}a", done.text);
        let Step::Changed(again) = completion.step(&edited, edited.len(), 1, &[], people) else {
            panic!("expected a fresh completion");
        };
        assert_eq!(again.text, "Alice: Alice ");
    }

    #[test]
    fn a_shortcode_becomes_the_emoji() {
        let mut completion = Completion::default();
        let Step::Changed(done) = completion.step(":smile", 6, 1, &[], people) else {
            panic!("expected an emoji");
        };
        // The exact code first, followed by a space even at the start.
        assert_eq!(done.text, "😄 ");
        let Step::Changed(next) = completion.step(&done.text, done.cursor, 1, &[], people) else {
            panic!("expected the next one");
        };
        assert_ne!(next.text, done.text);
        assert!(!next.text.contains(':'), "{}", next.text);

        // Inside a line, and with the closing colon typed as well.
        let mut completion = Completion::default();
        let Step::Changed(done) = completion.step("gg :heart:", 10, 1, &[], people) else {
            panic!("expected an emoji");
        };
        assert_eq!(done.text, "gg ❤️ ");
    }

    #[test]
    fn a_short_or_unknown_shortcode_is_left_alone() {
        let mut completion = Completion::default();
        // One letter after the colon is not enough to go on.
        assert_eq!(completion.step(":s", 2, 1, &[], people), Step::NoMatch);
        assert_eq!(completion.step(":zzqx", 5, 1, &[], people), Step::NoMatch);
        assert!(emoji("thumbs").iter().any(|e| e == "👍"));
    }

    #[test]
    fn nothing_before_the_cursor_leaves_the_key_alone() {
        let mut completion = Completion::default();
        assert_eq!(completion.step("", 0, 1, &[], people), Step::Nothing);
        assert_eq!(completion.step("hi ", 3, 1, &[], people), Step::Nothing);
    }
}
