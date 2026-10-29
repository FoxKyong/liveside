//! The moderating panel: what is waiting for a decision, and what every
//! moderator has done lately.
//!
//! Both already appear in the chat, which is enough in a quiet channel and
//! useless in a busy one — a held message scrolls past while it is being
//! read. Here they stay put, per channel, until they are answered.

use std::collections::{HashMap, VecDeque};
use std::rc::Rc;

use gtk::prelude::*;
use liveside_client::lang::{Strings, fill};
use liveside_proto::{MessageId, UserId};

/// How many log lines a channel keeps. The log is for "what just
/// happened", not an archive; the chat log on disk is the archive.
const LOG_LINES: usize = 300;

/// What answering something does: the panel is handed this, so it needs
/// to know nothing about the engine.
type Decide = Rc<dyn Fn(&UserId, &Waiting, bool)>;

/// Something waiting for a moderator to answer it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Waiting {
    /// A message AutoMod or a blocked term caught.
    Held {
        id: MessageId,
        user: String,
        text: String,
        detail: String,
    },
    /// Somebody banned asking to be let back in.
    Unban {
        id: String,
        user: String,
        text: String,
    },
    /// Somebody who spent channel points on a reward made from here.
    Redeemed {
        id: String,
        /// Twitch wants the reward's id to answer it, and a reader wants
        /// its name; both are kept, because neither does for both jobs.
        reward_id: String,
        reward: String,
        user: String,
        cost: u64,
        text: String,
    },
}

impl Waiting {
    /// What answering it refers to; ids of the two kinds never collide,
    /// since one comes from a message and the other from a request.
    fn id(&self) -> &str {
        match self {
            Waiting::Held { id, .. } => id.as_str(),
            Waiting::Unban { id, .. } => id,
            Waiting::Redeemed { id, .. } => id,
        }
    }

    /// What the two buttons say. A redemption is not allowed or denied,
    /// it is done or paid back.
    fn buttons(&self, strings: &'static Strings) -> (&'static str, &'static str) {
        match self {
            Waiting::Redeemed { .. } => (strings.reward_fulfil, strings.reward_refund),
            _ => (strings.held_allow, strings.held_deny),
        }
    }

    fn said(&self, strings: &'static Strings) -> String {
        match self {
            Waiting::Held {
                user, text, detail, ..
            } => fill(
                strings.held_message,
                &[("user", user), ("detail", detail), ("text", text)],
            ),
            Waiting::Unban { user, text, .. } => {
                fill(strings.unban_request, &[("user", user), ("text", text)])
            }
            Waiting::Redeemed {
                reward,
                user,
                cost,
                text,
                ..
            } => fill(
                strings.reward_waiting,
                &[
                    ("user", user),
                    ("reward", reward),
                    ("cost", &cost.to_string()),
                    // What they typed into the reward, where there is
                    // something: that is usually the whole request.
                    (
                        "text",
                        &match text.is_empty() {
                            true => String::new(),
                            false => format!(": {text}"),
                        },
                    ),
                ],
            ),
        }
    }
}

/// One channel's worth of it.
#[derive(Default)]
struct Pending {
    waiting: Vec<Waiting>,
    log: VecDeque<String>,
}

/// The panel itself.
pub struct Review {
    root: gtk::Box,
    queue_heading: gtk::Label,
    queue: gtk::ListBox,
    log_heading: gtk::Label,
    log: gtk::TextView,
    log_scroll: gtk::ScrolledWindow,
    /// Per channel, because a moderator of three channels has three
    /// queues and mixing them would be worse than useless.
    channels: HashMap<UserId, Pending>,
    shown: Option<UserId>,
    /// What answering does. The panel knows nothing of the engine.
    decide: Decide,
}

impl Review {
    pub fn new(strings: &'static Strings, decide: Decide) -> Self {
        let queue_heading = gtk::Label::builder()
            .label(strings.review_queue)
            .xalign(0.0)
            .css_classes(["heading"])
            .build();
        let queue = gtk::ListBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .build();
        let queue_scroll = gtk::ScrolledWindow::builder()
            .child(&queue)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .build();

        let log_heading = gtk::Label::builder()
            .label(strings.review_log)
            .xalign(0.0)
            .css_classes(["heading"])
            .build();
        let log = gtk::TextView::builder()
            .editable(false)
            .cursor_visible(false)
            .wrap_mode(gtk::WrapMode::WordChar)
            .left_margin(4)
            .right_margin(4)
            .build();
        crate::chat::swallow_context_menu(&log);
        let log_scroll = gtk::ScrolledWindow::builder()
            .child(&log)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .build();

        // The log keeps a strip at the bottom and the queue takes the
        // rest: what is waiting is what has to be read, and a log of the
        // last few actions is enough to glance at. The divider still
        // drags either way.
        log_scroll.set_size_request(-1, 150);
        let below = gtk::Box::new(gtk::Orientation::Vertical, 2);
        below.append(&log_heading);
        below.append(&log_scroll);
        let split = gtk::Paned::builder()
            .orientation(gtk::Orientation::Vertical)
            .start_child(&queue_scroll)
            .end_child(&below)
            .resize_start_child(true)
            .resize_end_child(false)
            .shrink_end_child(false)
            .build();

        let root = gtk::Box::new(gtk::Orientation::Vertical, 6);
        root.append(&queue_heading);
        root.append(&split);
        root.set_margin_start(6);
        root.set_size_request(260, -1);

        Self {
            root,
            queue_heading,
            queue,
            log_heading,
            log,
            log_scroll,
            channels: HashMap::new(),
            shown: None,
            decide,
        }
    }

    pub fn root(&self) -> &gtk::Box {
        &self.root
    }

    pub fn set_visible(&self, visible: bool) {
        self.root.set_visible(visible);
    }

    /// Draws another channel's queue and log.
    pub fn show_channel(&mut self, channel: Option<&UserId>, strings: &'static Strings) {
        self.shown = channel.cloned();
        self.redraw(strings);
    }

    /// Notes something to answer. The same thing twice — which a
    /// reconnect can bring — is kept once.
    pub fn add_waiting(&mut self, channel: &UserId, waiting: Waiting, strings: &'static Strings) {
        let pending = self.channels.entry(channel.clone()).or_default();
        if pending
            .waiting
            .iter()
            .any(|other| other.id() == waiting.id())
        {
            return;
        }
        pending.waiting.push(waiting);
        if self.shown.as_ref() == Some(channel) {
            self.redraw(strings);
        }
    }

    /// Takes it off the list, however it was answered.
    pub fn resolve(&mut self, channel: &UserId, id: &str, strings: &'static Strings) {
        if let Some(pending) = self.channels.get_mut(channel) {
            pending.waiting.retain(|waiting| waiting.id() != id);
        }
        if self.shown.as_ref() == Some(channel) {
            self.redraw(strings);
        }
    }

    /// Adds a line to the channel's moderation log.
    pub fn add_log(&mut self, channel: &UserId, said: String) {
        let pending = self.channels.entry(channel.clone()).or_default();
        pending.log.push_back(said.clone());
        while pending.log.len() > LOG_LINES {
            pending.log.pop_front();
        }
        if self.shown.as_ref() == Some(channel) {
            self.append_log(&said);
        }
    }

    /// Forgets a channel that was left.
    pub fn forget(&mut self, channel: &UserId, strings: &'static Strings) {
        self.channels.remove(channel);
        if self.shown.as_ref() == Some(channel) {
            self.shown = None;
            self.redraw(strings);
        }
    }

    fn redraw(&mut self, strings: &'static Strings) {
        self.queue_heading.set_label(strings.review_queue);
        self.log_heading.set_label(strings.review_log);
        while let Some(child) = self.queue.first_child() {
            self.queue.remove(&child);
        }
        let (waiting, log) = match self.shown.as_ref().and_then(|id| self.channels.get(id)) {
            Some(pending) => (pending.waiting.clone(), pending.log.clone()),
            None => (Vec::new(), VecDeque::new()),
        };
        if waiting.is_empty() {
            self.queue.append(
                &gtk::Label::builder()
                    .label(strings.review_empty)
                    .xalign(0.0)
                    .css_classes(["dim-label"])
                    .margin_start(6)
                    .margin_top(6)
                    .build(),
            );
        }
        for item in &waiting {
            self.queue.append(&self.row(item, strings));
        }
        self.log.buffer().set_text("");
        for line in &log {
            self.append_log(line);
        }
    }

    fn append_log(&self, said: &str) {
        let buffer = self.log.buffer();
        let mut end = buffer.end_iter();
        if buffer.char_count() > 0 {
            buffer.insert(&mut end, "\n");
        }
        buffer.insert(&mut buffer.end_iter(), said);
        // Follow the newest line only when the reader is already at it.
        let adjustment = self.log_scroll.vadjustment();
        if adjustment.value() >= adjustment.upper() - adjustment.page_size() - 1.0 {
            glib::idle_add_local_once(move || {
                adjustment.set_value(adjustment.upper() - adjustment.page_size());
            });
        }
    }

    fn row(&self, waiting: &Waiting, strings: &'static Strings) -> gtk::Box {
        let line = gtk::Box::new(gtk::Orientation::Vertical, 4);
        line.set_margin_start(6);
        line.set_margin_end(6);
        line.set_margin_top(6);
        line.set_margin_bottom(6);
        line.append(
            &gtk::Label::builder()
                .label(waiting.said(strings))
                .xalign(0.0)
                .wrap(true)
                .wrap_mode(gtk::pango::WrapMode::WordChar)
                .build(),
        );
        let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        let (yes, no) = waiting.buttons(strings);
        for (label, allow) in [(yes, true), (no, false)] {
            let button = gtk::Button::builder().label(label).build();
            let decide = self.decide.clone();
            let waiting = waiting.clone();
            let channel = self.shown.clone();
            let buttons_for_click = buttons.clone();
            button.connect_clicked(move |_| {
                let Some(channel) = channel.clone() else {
                    return;
                };
                // The answer is given once; the row goes when the engine
                // says it was taken.
                buttons_for_click.set_sensitive(false);
                decide(&channel, &waiting, allow);
            });
            buttons.append(&button);
        }
        line.append(&buttons);
        line
    }
}
