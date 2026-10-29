//! The list of people active in the channel on screen.
//!
//! It shows the same people the terminal frontend shows, worked out by the
//! same shared code: whoever has talked while their message is still in the
//! buffer, plus anyone the engine says is present. Twitch only tells
//! moderators the latter, so most of the time this is who has talked.

use std::cell::RefCell;
use std::rc::Rc;

use gtk::prelude::*;
use liveside_client::chatters::Chatter;
use liveside_client::lang::{Strings, counted};

use crate::emotes::Emotes;
use crate::theme::Palette;

/// How tall a badge is drawn beside a name.
const BADGE_SIZE: i32 = 16;

/// One name on screen.
struct Row {
    /// What the list holds: a `GtkListBox` wraps whatever is appended in
    /// a row of its own, and removing means removing that wrapper, not
    /// the button inside it.
    row: gtk::ListBoxRow,
    /// Twitch's badge pictures, or the letters that stand in for them.
    badges: gtk::Box,
    label: gtk::Label,
    /// What clicking it writes; replaced when the row is reused.
    name: Rc<RefCell<String>>,
}

/// The panel beside the chat.
pub struct UserList {
    root: gtk::Box,
    heading: gtk::Label,
    list: gtk::ListBox,
    /// The rows on screen, reused as the list changes. With the `irc`
    /// user list a channel can hold hundreds of people, and building that
    /// many widgets again every time somebody joins or leaves would show.
    rows: Vec<Row>,
    /// What is drawn right now, so an unchanged list is left alone rather
    /// than rebuilt under the pointer every time someone speaks.
    shown: Vec<Chatter>,
    palette: &'static Palette,
    /// What clicking a name does: it starts writing to that person.
    picked: Rc<dyn Fn(&str)>,
    /// Where badge pictures come from; shared with the chat.
    emotes: Rc<Emotes>,
}

impl UserList {
    pub fn new(palette: &'static Palette, picked: Rc<dyn Fn(&str)>, emotes: Rc<Emotes>) -> Self {
        let heading = gtk::Label::builder()
            .xalign(0.0)
            .css_classes(["heading"])
            .build();
        let list = gtk::ListBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .build();
        let scroll = gtk::ScrolledWindow::builder()
            .child(&list)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .build();
        let root = gtk::Box::new(gtk::Orientation::Vertical, 6);
        root.append(&heading);
        root.append(&scroll);
        root.set_margin_start(6);
        root.set_size_request(160, -1);
        Self {
            root,
            heading,
            list,
            rows: Vec::new(),
            shown: Vec::new(),
            palette,
            picked,
            emotes,
        }
    }

    pub fn root(&self) -> &gtk::Box {
        &self.root
    }

    /// Whether the panel is on screen. A `GtkPaned` gives its space back
    /// to the chat when the child is hidden, so this is all it takes.
    pub fn visible(&self) -> bool {
        self.root.is_visible()
    }

    pub fn set_visible(&self, visible: bool) {
        self.root.set_visible(visible);
    }

    /// Draws the list, unless it already says this. Rows that have not
    /// changed are left alone, so the name under the pointer stays where
    /// it is and a click lands on the person it was aimed at.
    pub fn show(&mut self, chatters: Vec<Chatter>, strings: &'static Strings) {
        self.heading.set_text(&counted(
            strings.active_count_one,
            strings.active_count,
            chatters.len(),
        ));
        if chatters == self.shown {
            return;
        }
        for (i, chatter) in chatters.iter().enumerate() {
            match self.rows.get(i) {
                Some(_) if self.shown.get(i) == Some(chatter) => {}
                Some(row) => {
                    row.label.set_text(&chatter.name);
                    row.row.set_tooltip_text(Some(&chatter.name));
                    self.fill_badges(&row.badges, chatter);
                    *row.name.borrow_mut() = chatter.name.clone();
                }
                None => {
                    let row = self.row(chatter);
                    self.list.append(&row.row);
                    self.rows.push(row);
                }
            }
        }
        for row in self.rows.drain(chatters.len()..) {
            self.list.remove(&row.row);
        }
        self.shown = chatters;
    }

    /// Repaints for another theme; only the badge symbol carries a colour.
    pub fn set_palette(&mut self, palette: &'static Palette, strings: &'static Strings) {
        self.palette = palette;
        // Taking the list out makes `show` draw it again rather than see
        // its own work and leave the old colour where it is.
        let chatters = std::mem::take(&mut self.shown);
        self.show(chatters, strings);
    }

    fn row(&self, chatter: &Chatter) -> Row {
        let label = gtk::Label::builder()
            .xalign(0.0)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .label(&chatter.name)
            .build();
        let badges = gtk::Box::new(gtk::Orientation::Horizontal, 2);
        self.fill_badges(&badges, chatter);
        let line = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        line.append(&badges);
        line.append(&label);
        // A flat button rather than a plain label: clicking a name is how
        // one starts writing to that person, and a button says so by
        // lighting up under the pointer.
        let button = gtk::Button::builder()
            .child(&line)
            .css_classes(["flat"])
            .build();
        // The name is held rather than copied into the handler, so a row
        // can be given to somebody else without leaving the old click on it.
        let name = Rc::new(RefCell::new(chatter.name.clone()));
        button.connect_clicked({
            let picked = self.picked.clone();
            let name = name.clone();
            move |_| picked(&name.borrow())
        });
        // A long name is cut short to fit the column, so the whole of it
        // is on the row itself: whoever keeps the column narrow can still
        // read who is there.
        let row = gtk::ListBoxRow::builder().child(&button).build();
        row.set_tooltip_text(Some(&chatter.name));
        Row {
            row,
            badges,
            label,
            name,
        }
    }

    /// Draws someone's badges: Twitch's own pictures where the backend
    /// found one, and the letter the terminal uses where it did not. The
    /// picture is put in place empty and filled when it arrives, so the
    /// name beside it does not move once the list is drawn.
    fn fill_badges(&self, into: &gtk::Box, chatter: &Chatter) {
        while let Some(child) = into.first_child() {
            into.remove(&child);
        }
        for badge in &chatter.badges {
            let symbol = crate::chat::badge_symbol(badge);
            if badge.image.is_empty() {
                let Some(symbol) = symbol else { continue };
                into.append(
                    &gtk::Label::builder()
                        .use_markup(true)
                        .label(format!(
                            "<span foreground=\"{}\">{symbol}</span>",
                            self.palette.badge
                        ))
                        .build(),
                );
                continue;
            }
            // A GtkImage rather than a GtkPicture: it draws at the size
            // it is told and nothing else, which is what a row of names
            // beside one another needs. A picture would take its own
            // natural size and tower over the text.
            let picture = gtk::Image::builder()
                .pixel_size(BADGE_SIZE)
                .tooltip_text(if badge.title.is_empty() {
                    badge.set_id.clone()
                } else {
                    badge.title.clone()
                })
                .build();
            into.append(&picture);
            let waiting = picture.clone();
            self.emotes.request_url(
                crate::chat::badge_key(badge),
                badge.image.clone(),
                move |animation| waiting.set_paintable(Some(animation.still())),
            );
        }
    }
}
