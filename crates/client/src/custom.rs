//! Custom commands as a frontend runs them: a typed line turned into what
//! the command says, before the frontend does anything else with it, so
//! the result goes wherever a line typed by hand would.

use liveside_core::commands::{Context, CustomCommands};
use liveside_proto::Timestamp;
use liveside_proto::model::{StreamStats, ViewerHistory};

use crate::lang::Strings;

/// What became of a typed line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Typed {
    /// Not a custom command: it goes on as it was.
    Unchanged,
    /// What the command made of it.
    Replaced(String),
    /// Not run, and why, in the reader's language.
    Refused(String),
}

pub fn expand(
    commands: &CustomCommands,
    typed: &str,
    context: &Context,
    strings: &'static Strings,
) -> Typed {
    match commands.expand(typed, context) {
        None => Typed::Unchanged,
        Some(Ok(line)) => Typed::Replaced(line),
        Some(Err(problem)) => Typed::Refused(crate::lang::custom_problem(strings, &problem)),
    }
}

/// What a command may say about the stream: viewers and uptime while it
/// runs, and the title and category the last change named.
pub fn stream_context(
    context: &mut Context,
    stats: Option<&StreamStats>,
    history: Option<&ViewerHistory>,
    now: Timestamp,
) {
    if let Some(stats) = stats {
        context.viewers = Some(stats.viewers);
        let minutes = (now.0 - stats.started.0).max(0) / 60_000;
        context.uptime = Some(format!("{}:{:02}", minutes / 60, minutes % 60));
    }
    if let Some(mark) = history.and_then(|h| h.marks.last()) {
        context.title = (!mark.title.is_empty()).then(|| mark.title.clone());
        context.game = (!mark.game.is_empty()).then(|| mark.game.clone());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lang::Language;

    #[test]
    fn a_line_is_replaced_left_alone_or_refused_in_words() {
        let commands = CustomCommands::read(&["/slap /me slaps $$1".to_owned()]).unwrap();
        let text = Language::Czech.strings();
        let mut context = Context::default();
        let history = {
            let mut h = ViewerHistory::default();
            let stats = StreamStats {
                viewers: 5,
                started: Timestamp(0),
            };
            h.record(Timestamp(1), &stats);
            h.mark(Timestamp(2), "Rust", "Software");
            h
        };
        stream_context(
            &mut context,
            Some(&StreamStats {
                viewers: 42,
                started: Timestamp(0),
            }),
            Some(&history),
            Timestamp(75 * 60_000),
        );
        assert_eq!(
            (
                context.viewers,
                context.uptime.as_deref(),
                context.title.as_deref()
            ),
            (Some(42), Some("1:15"), Some("Rust"))
        );
        assert_eq!(
            expand(&commands, "/slap bot", &context, text),
            Typed::Replaced("/me slaps bot".into())
        );
        assert_eq!(expand(&commands, "hi", &context, text), Typed::Unchanged);
        assert_eq!(
            expand(&commands, "/slap", &context, text),
            Typed::Refused("/slap potřebuje $1".into())
        );
    }
}
