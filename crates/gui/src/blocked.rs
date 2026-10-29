//! The words and phrases a channel blocks, as a table: what, since when,
//! until when, and a button to take each one off. A field below adds one.
//!
//! The window stays open and is redrawn whenever the list comes again, so
//! after adding or removing it asks for the list once more rather than
//! guessing what Twitch did with the request.

use std::cell::Cell;
use std::rc::Rc;

use gtk::prelude::*;
use liveside_client::lang::{Strings, fill};
use liveside_proto::model::BlockedTerm;

/// What the window asks the channel to do.
pub enum Act {
    Block(String),
    Unblock(String),
}

pub struct BlockedWindow {
    window: gtk::Window,
    rows: gtk::Grid,
    strings: &'static Strings,
    act: Rc<dyn Fn(Act)>,
    open: Rc<Cell<bool>>,
}

impl BlockedWindow {
    pub fn open(
        parent: &gtk::ApplicationWindow,
        channel: &str,
        strings: &'static Strings,
        act: Rc<dyn Fn(Act)>,
    ) -> Rc<Self> {
        let rows = gtk::Grid::builder()
            .row_spacing(4)
            .column_spacing(16)
            .margin_start(12)
            .margin_end(12)
            .margin_top(12)
            .margin_bottom(12)
            .build();
        let scroll = gtk::ScrolledWindow::builder()
            .child(&rows)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .build();

        let field = gtk::Entry::builder()
            .placeholder_text(strings.blocked_add_hint)
            .hexpand(true)
            .build();
        let add = gtk::Button::with_label(strings.blocked_add);
        let adding = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        adding.set_margin_start(12);
        adding.set_margin_end(12);
        adding.set_margin_bottom(12);
        adding.append(&field);
        adding.append(&add);

        let outer = gtk::Box::new(gtk::Orientation::Vertical, 6);
        outer.append(&scroll);
        outer.append(&adding);

        let window = gtk::Window::builder()
            .transient_for(parent)
            .modal(false)
            .title(fill(strings.blocked_title, &[("channel", channel)]))
            .default_width(560)
            .default_height(420)
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

        let block = {
            let act = act.clone();
            let field = field.clone();
            move || {
                let text = field.text().trim().to_owned();
                if !text.is_empty() {
                    act(Act::Block(text));
                    field.set_text("");
                }
            }
        };
        field.connect_activate({
            let block = block.clone();
            move |_| block()
        });
        add.connect_clicked(move |_| block());

        let this = Rc::new(Self {
            window,
            rows,
            strings,
            act,
            open,
        });
        this.window.present();
        field.grab_focus();
        this
    }

    pub fn is_open(&self) -> bool {
        self.open.get()
    }

    /// Draws the list as Twitch gave it.
    pub fn show(&self, terms: &[BlockedTerm]) {
        while let Some(child) = self.rows.first_child() {
            self.rows.remove(&child);
        }
        let strings = self.strings;
        if terms.is_empty() {
            self.rows.attach(
                &gtk::Label::builder()
                    .label(strings.blocked_empty)
                    .xalign(0.0)
                    .css_classes(["dim-label"])
                    .build(),
                0,
                0,
                4,
                1,
            );
            return;
        }
        let heading = |text: &str| {
            gtk::Label::builder()
                .label(text)
                .xalign(0.0)
                .css_classes(["heading"])
                .build()
        };
        self.rows.attach(&heading(strings.blocked_term), 0, 0, 1, 1);
        self.rows
            .attach(&heading(strings.blocked_added), 1, 0, 1, 1);
        self.rows
            .attach(&heading(strings.blocked_expires), 2, 0, 1, 1);

        let mut sorted: Vec<&BlockedTerm> = terms.iter().collect();
        sorted.sort_by_key(|term| term.text.to_lowercase());
        for (row, term) in (1..).zip(sorted) {
            // The term in the chat's own font: a `*` or a space at its
            // edge is part of what it matches.
            let text = gtk::Label::builder()
                .label(&term.text)
                .xalign(0.0)
                .hexpand(true)
                .selectable(true)
                .wrap(true)
                .wrap_mode(gtk::pango::WrapMode::WordChar)
                .build();
            text.add_css_class("monospace");
            self.rows.attach(&text, 0, row, 1, 1);
            self.rows.attach(&dim(day(&term.created_at)), 1, row, 1, 1);
            let until = term.expires_at.as_deref().map_or_else(
                || strings.blocked_never.to_owned(),
                |until| day(until).to_owned(),
            );
            self.rows.attach(&dim(&until), 2, row, 1, 1);

            let remove = gtk::Button::with_label(strings.blocked_remove);
            remove.set_valign(gtk::Align::Center);
            let act = self.act.clone();
            let term_text = term.text.clone();
            remove.connect_clicked(move |button| {
                // Once: the list comes again when Twitch has done it.
                button.set_sensitive(false);
                act(Act::Unblock(term_text.clone()));
            });
            self.rows.attach(&remove, 3, row, 1, 1);
        }
    }

    pub fn present(&self) {
        self.window.present();
    }
}

/// The day of an RFC 3339 time: when a term was added is a matter of days.
fn day(time: &str) -> &str {
    time.split('T').next().unwrap_or(time)
}

fn dim(text: &str) -> gtk::Label {
    gtk::Label::builder()
        .label(text)
        .xalign(0.0)
        .css_classes(["dim-label"])
        .build()
}
