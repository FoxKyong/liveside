//! What lit up and what was hidden, across every channel: the lists a
//! frontend shows apart from the chat, as Chatty's Highlighted and Ignored
//! windows do. The engine keeps them too, so a frontend that attaches
//! later starts with what was there.

use std::collections::VecDeque;

use liveside_proto::model::{Channel, CollectedLine, Line, LineContent, SharedSource};

use crate::lang::{Strings, from_engine};

/// How many lines each list keeps, as the engine does.
pub const KEPT: usize = 500;

#[derive(Debug, Clone, Default)]
pub struct Collected {
    pub highlighted: VecDeque<CollectedLine>,
    pub ignored: VecDeque<CollectedLine>,
}

impl Collected {
    pub fn from_snapshot(highlighted: Vec<CollectedLine>, ignored: Vec<CollectedLine>) -> Self {
        Self {
            highlighted: highlighted.into(),
            ignored: ignored.into(),
        }
    }

    /// Notes a channel line, if it lit up now: history is not news.
    /// Whether it was kept.
    pub fn line(&mut self, channel: &Channel, line: &Line) -> bool {
        if !line.flags.highlighted || line.flags.historical {
            return false;
        }
        keep(&mut self.highlighted, channel, line);
        true
    }

    pub fn ignored(&mut self, channel: &Channel, line: &Line) {
        keep(&mut self.ignored, channel, line);
    }
}

fn keep(list: &mut VecDeque<CollectedLine>, channel: &Channel, line: &Line) {
    list.push_back(CollectedLine {
        channel: channel.clone(),
        line: line.clone(),
    });
    while list.len() > KEPT {
        list.pop_front();
    }
}

/// The line as a list shows it: with the channel it was said in in front,
/// the way a Shared Chat message names the channel it came from.
pub fn shown(item: &CollectedLine, text: &'static Strings) -> Line {
    let mut line = item.line.clone();
    let channel = &item.channel;
    match &mut line.content {
        LineContent::Message(message) => {
            // A Shared Chat message already says where it came from.
            if message.shared_source.is_none() {
                message.shared_source = Some(Box::new(SharedSource {
                    channel: channel.clone(),
                    message_id: message.id.clone(),
                    badges: Vec::new(),
                }));
            }
        }
        LineContent::UserNotice { system_text, .. } => {
            *system_text = format!("#{} {system_text}", channel.login);
        }
        LineContent::Notice {
            text: said,
            code,
            args,
            ..
        } => {
            // Translated here, since the channel goes in front of it.
            *said = format!("#{} {}", channel.login, from_engine(text, code, args, said));
            code.clear();
            args.clear();
        }
    }
    line
}

#[cfg(test)]
mod tests {
    use super::*;
    use liveside_proto::Timestamp;
    use liveside_proto::model::{LineFlags, NoticeLevel};

    fn channel() -> Channel {
        Channel {
            id: "1".into(),
            login: "harborlight".into(),
            display_name: "Harborlight".into(),
        }
    }

    fn notice(flags: LineFlags) -> Line {
        Line {
            time: Timestamp(0),
            content: LineContent::Notice {
                level: NoticeLevel::Info,
                text: "3 messages hidden by the ignore rules".into(),
                code: "ignore.hidden".into(),
                args: vec![("count".into(), "3".into())],
            },
            flags,
        }
    }

    #[test]
    fn only_what_lights_up_now_is_listed_and_the_lists_are_bounded() {
        let mut collected = Collected::default();
        let lit = LineFlags {
            highlighted: true,
            ..LineFlags::default()
        };
        assert!(!collected.line(&channel(), &notice(LineFlags::default())));
        assert!(!collected.line(
            &channel(),
            &notice(LineFlags {
                historical: true,
                ..lit.clone()
            })
        ));
        for _ in 0..KEPT + 3 {
            assert!(collected.line(&channel(), &notice(lit.clone())));
            collected.ignored(&channel(), &notice(LineFlags::default()));
        }
        assert_eq!(collected.highlighted.len(), KEPT);
        assert_eq!(collected.ignored.len(), KEPT);
    }

    #[test]
    fn a_listed_line_names_its_channel_in_the_readers_language() {
        let text = crate::lang::Language::Czech.strings();
        let shown = shown(
            &CollectedLine {
                channel: channel(),
                line: notice(LineFlags::default()),
            },
            text,
        );
        let LineContent::Notice { text, code, .. } = &shown.content else {
            panic!("a notice");
        };
        assert_eq!(text, "#harborlight Skryto podle pravidel pro ignorování: 3");
        assert!(code.is_empty());
    }
}
