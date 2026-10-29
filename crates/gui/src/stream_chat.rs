//! Stream Chat: a channel's messages alone, in a window of one colour,
//! for a streaming program to capture and show on stream. Only what
//! people write, no notices; without times; each message for as long as
//! the settings say. What a moderator removes is taken off at once rather
//! than struck through: it is the stream's audience that reads this.
//!
//! As Chatty's Stream Chat, written from its help.

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::rc::Rc;

use gtk::glib;
use gtk::prelude::*;
use liveside_client::config::StreamChatConfig;
use liveside_client::lang::{Strings, fill};
use liveside_proto::model::{Line, LineContent};
use liveside_proto::{MessageId, Rgb, Timestamp, UserId};

use crate::chat::ChatView;
use crate::emotes::Emotes;

/// How many messages are kept at most, whatever the time.
const KEPT: usize = 200;

pub struct StreamChat {
    window: gtk::Window,
    view: RefCell<ChatView>,
    /// What is shown, oldest first: the lines are drawn again from these
    /// when one goes.
    lines: RefCell<VecDeque<Line>>,
    seconds: u32,
    strings: &'static Strings,
    open: Rc<Cell<bool>>,
}

impl StreamChat {
    pub fn open(
        parent: &gtk::ApplicationWindow,
        channel: &str,
        settings: &StreamChatConfig,
        emotes: Rc<Emotes>,
        strings: &'static Strings,
    ) -> Rc<Self> {
        let background = Rgb::parse_hex(&settings.background).unwrap_or(Rgb { r: 0, g: 0, b: 0 });
        let mut view = ChatView::new(KEPT, emotes, crate::theme::for_background(background));
        view.set_times(false);
        view.set_links(false);
        let root = view.root().clone();
        root.set_vexpand(true);
        root.add_css_class("liveside-stream-chat");
        // One colour, edge to edge, and the text at the size asked for.
        let css = gtk::CssProvider::new();
        css.load_from_data(&format!(
            ".liveside-stream-chat textview, .liveside-stream-chat textview text, \
             .liveside-stream-chat scrolledwindow {{ background-color: #{:02x}{:02x}{:02x}; \
             font-size: {}pt; }} \
             .liveside-stream-chat scrollbar {{ opacity: 0; }}",
            background.r,
            background.g,
            background.b,
            settings.font_size.clamp(8, 48)
        ));
        if let Some(display) = gtk::gdk::Display::default() {
            gtk::style_context_add_provider_for_display(
                &display,
                &css,
                gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
            );
        }
        let window = gtk::ApplicationWindow::builder()
            .title(fill(strings.stream_chat_title, &[("channel", channel)]))
            .default_width(420)
            .default_height(600)
            .child(&root)
            .build();
        if let Some(application) = parent.application() {
            window.set_application(Some(&application));
        }
        let open = Rc::new(Cell::new(true));
        window.connect_close_request({
            let open = open.clone();
            move |_| {
                open.set(false);
                glib::Propagation::Proceed
            }
        });
        let this = Rc::new(Self {
            window: window.upcast(),
            view: RefCell::new(view),
            lines: RefCell::new(VecDeque::new()),
            seconds: settings.seconds,
            strings,
            open,
        });
        // Old messages go once a second, while the window is open.
        if this.seconds > 0 {
            let weak = Rc::downgrade(&this);
            glib::timeout_add_seconds_local(1, move || match weak.upgrade() {
                Some(chat) if chat.is_open() => {
                    chat.expire(Timestamp::now());
                    glib::ControlFlow::Continue
                }
                _ => glib::ControlFlow::Break,
            });
        }
        this.window.present();
        this
    }

    pub fn is_open(&self) -> bool {
        self.open.get()
    }

    pub fn present(&self) {
        self.window.present();
    }

    pub fn close(&self) {
        self.window.close();
    }

    /// A line from the channel: shown if somebody wrote it just now.
    pub fn push(&self, line: &Line) {
        if !shown_on_stream(line) {
            return;
        }
        let mut lines = self.lines.borrow_mut();
        lines.push_back(line.clone());
        while lines.len() > KEPT {
            lines.pop_front();
        }
        self.view.borrow_mut().push(line, self.strings);
        self.view.borrow().jump_to_bottom();
    }

    pub fn delete_message(&self, id: &MessageId) {
        self.remove(|line| matches!(&line.content, LineContent::Message(m) if &m.id == id));
    }

    pub fn delete_user(&self, user: &UserId) {
        self.remove(
            |line| matches!(&line.content, LineContent::Message(m) if &m.sender.id == user),
        );
    }

    pub fn clear(&self) {
        self.remove(|_| true);
    }

    /// Takes off what is older than the settings allow.
    fn expire(&self, now: Timestamp) {
        let seconds = self.seconds;
        self.remove(|line| too_old(line, now, seconds));
    }

    fn remove(&self, which: impl Fn(&Line) -> bool) {
        let mut lines = self.lines.borrow_mut();
        let before = lines.len();
        lines.retain(|line| !which(line));
        if lines.len() == before {
            return;
        }
        let mut view = self.view.borrow_mut();
        view.clear();
        for line in lines.iter() {
            view.push(line, self.strings);
        }
        view.jump_to_bottom();
    }
}

/// Whether a line has been up longer than the settings allow; 0 keeps
/// everything.
fn too_old(line: &Line, now: Timestamp, seconds: u32) -> bool {
    seconds > 0 && now.0 - line.time.0 > i64::from(seconds) * 1000
}

/// What a stream's audience is shown: messages people write, as they
/// come, and not what was deleted or loaded from history.
fn shown_on_stream(line: &Line) -> bool {
    matches!(line.content, LineContent::Message(_)) && !line.flags.historical && !line.flags.deleted
}

#[cfg(test)]
mod tests {
    use super::*;
    use liveside_proto::model::{LineFlags, NoticeLevel};

    #[test]
    fn only_live_messages_go_on_stream() {
        let notice = Line {
            time: Timestamp(0),
            content: LineContent::Notice {
                level: NoticeLevel::Info,
                text: "Joined".into(),
                code: String::new(),
                args: Vec::new(),
            },
            flags: LineFlags::default(),
        };
        assert!(!shown_on_stream(&notice));
        let at = |ms| Line {
            time: Timestamp(ms),
            ..notice.clone()
        };
        assert!(!too_old(&at(0), Timestamp(20_000), 20), "just in time");
        assert!(too_old(&at(0), Timestamp(20_001), 20));
        assert!(!too_old(&at(0), Timestamp(9_999_999), 0), "0 keeps them");
    }
}
