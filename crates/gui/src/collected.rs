//! What lit up and what was hidden, in every channel: a window with a page
//! for each, drawn like the chat, with the channel in front of each line.
//! Hidden messages are not anywhere else, which is what a moderator who
//! ignores somebody needs to be able to look at.
//!
//! The window stays open and takes lines as they come.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk::prelude::*;
use liveside_client::collected::{Collected, shown};
use liveside_client::lang::Strings;
use liveside_proto::Timestamp;
use liveside_proto::model::{CollectedLine, Line, LineContent, LineFlags, NoticeLevel};

use crate::chat::ChatView;
use crate::emotes::Emotes;
use crate::theme::Palette;

/// Which page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Highlighted,
    Ignored,
}

impl Kind {
    fn name(self) -> &'static str {
        match self {
            Kind::Highlighted => "highlighted",
            Kind::Ignored => "ignored",
        }
    }
}

pub struct CollectedWindow {
    window: gtk::Window,
    stack: gtk::Stack,
    highlighted: RefCell<ChatView>,
    ignored: RefCell<ChatView>,
    /// Whether a page still shows only that there is nothing on it.
    empty: [Cell<bool>; 2],
    strings: &'static Strings,
    open: Rc<Cell<bool>>,
}

impl CollectedWindow {
    pub fn open(
        parent: &gtk::ApplicationWindow,
        collected: &Collected,
        page: Kind,
        emotes: Rc<Emotes>,
        palette: &'static Palette,
        strings: &'static Strings,
    ) -> Rc<Self> {
        let limit = liveside_client::collected::KEPT;
        let mut highlighted = ChatView::new(limit, emotes.clone(), palette);
        let mut ignored = ChatView::new(limit, emotes, palette);
        // Read here, not acted on: a link must not look like one to click.
        highlighted.set_links(false);
        ignored.set_links(false);
        let empty = [Cell::new(false), Cell::new(false)];
        fill(
            &mut highlighted,
            &collected.highlighted,
            strings,
            strings.collected_highlighted_none,
            &empty[0],
        );
        fill(
            &mut ignored,
            &collected.ignored,
            strings,
            strings.collected_ignored_none,
            &empty[1],
        );

        let stack = gtk::Stack::new();
        stack.add_titled(
            highlighted.root(),
            Some(Kind::Highlighted.name()),
            strings.collected_highlighted,
        );
        stack.add_titled(
            ignored.root(),
            Some(Kind::Ignored.name()),
            strings.collected_ignored,
        );
        stack.set_vexpand(true);
        let switcher = gtk::StackSwitcher::builder()
            .stack(&stack)
            .halign(gtk::Align::Center)
            .margin_top(6)
            .margin_bottom(6)
            .build();
        let outer = gtk::Box::new(gtk::Orientation::Vertical, 0);
        outer.append(&switcher);
        outer.append(&stack);

        let window = gtk::Window::builder()
            .transient_for(parent)
            .modal(false)
            .title(strings.collected_title)
            .default_width(640)
            .default_height(480)
            .child(&outer)
            .build();
        crate::dialog::close_on_escape(&window);
        let open = Rc::new(Cell::new(true));
        window.connect_close_request({
            let open = open.clone();
            move |_| {
                open.set(false);
                gtk::glib::Propagation::Proceed
            }
        });
        let this = Rc::new(Self {
            window,
            stack,
            highlighted: RefCell::new(highlighted),
            ignored: RefCell::new(ignored),
            empty,
            strings,
            open,
        });
        this.show(page);
        this
    }

    pub fn is_open(&self) -> bool {
        self.open.get()
    }

    /// Brings the window up at a page.
    pub fn show(&self, page: Kind) {
        self.stack.set_visible_child_name(page.name());
        self.window.present();
    }

    /// A line that has just lit up or been hidden.
    pub fn push(&self, page: Kind, item: &CollectedLine) {
        let (view, empty) = match page {
            Kind::Highlighted => (&self.highlighted, &self.empty[0]),
            Kind::Ignored => (&self.ignored, &self.empty[1]),
        };
        let mut view = view.borrow_mut();
        // "Nothing yet" goes once there is something.
        if empty.replace(false) {
            view.clear();
        }
        view.push(&shown(item, self.strings), self.strings);
    }

    pub fn set_palette(&self, palette: &'static Palette) {
        self.highlighted.borrow_mut().set_palette(palette);
        self.ignored.borrow_mut().set_palette(palette);
    }
}

fn fill(
    view: &mut ChatView,
    list: &std::collections::VecDeque<CollectedLine>,
    strings: &'static Strings,
    none: &str,
    empty: &Cell<bool>,
) {
    for item in list {
        view.push(&shown(item, strings), strings);
    }
    if list.is_empty() {
        view.push(
            &Line {
                time: Timestamp::now(),
                content: LineContent::Notice {
                    level: NoticeLevel::Info,
                    text: none.to_owned(),
                    code: String::new(),
                    args: Vec::new(),
                },
                flags: LineFlags::default(),
            },
            strings,
        );
        empty.set(true);
    }
}
