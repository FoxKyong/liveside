//! The channels this account follows, in a window of their own: the ones
//! streaming first, with what they stream and for how long, then the rest
//! by name. Each row joins its channel; a channel already joined says so
//! instead.
//!
//! The window stays open and is redrawn whenever the list comes again:
//! Refresh asks for it, and so does the button that opened it. What the
//! engine hears every minute about who streams is drawn into it as well,
//! so an open window does not go stale.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk::prelude::*;
use liveside_client::followed::{about, with_live};
use liveside_client::lang::{Strings, fill, stream_stats};
use liveside_proto::Timestamp;
use liveside_proto::model::{Channel, FollowedChannel};

pub struct FollowedWindow {
    window: gtk::Window,
    rows: gtk::ListBox,
    strings: &'static Strings,
    joined: Rc<dyn Fn(&Channel) -> bool>,
    join: Rc<dyn Fn(&str)>,
    open: Rc<Cell<bool>>,
    /// What is drawn, for the next word about who streams.
    shown: RefCell<Vec<FollowedChannel>>,
}

impl FollowedWindow {
    /// Opens the window, empty until [`Self::show`]. `joined` says whether
    /// a channel is joined already, `join` joins one by its login, and
    /// `refresh` asks for the list again.
    pub fn open(
        parent: &gtk::ApplicationWindow,
        strings: &'static Strings,
        joined: Rc<dyn Fn(&Channel) -> bool>,
        join: Rc<dyn Fn(&str)>,
        refresh: impl Fn() + 'static,
    ) -> Rc<Self> {
        let rows = gtk::ListBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .build();
        let scroll = gtk::ScrolledWindow::builder()
            .child(&rows)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .build();
        let again = gtk::Button::with_label(strings.followed_refresh);
        again.set_halign(gtk::Align::End);
        again.set_margin_end(8);
        again.set_margin_bottom(8);
        again.connect_clicked(move |_| refresh());

        let outer = gtk::Box::new(gtk::Orientation::Vertical, 6);
        outer.append(&scroll);
        outer.append(&again);

        let window = gtk::Window::builder()
            .transient_for(parent)
            .modal(false)
            .title(fill(
                strings.followed_title,
                &[("count", "…"), ("live", "…")],
            ))
            .default_width(480)
            .default_height(520)
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
            rows,
            strings,
            joined,
            join,
            open,
            shown: RefCell::new(Vec::new()),
        });
        this.window.present();
        this
    }

    pub fn is_open(&self) -> bool {
        self.open.get()
    }

    pub fn present(&self) {
        self.window.present();
    }

    /// Draws who streams now into the list already shown.
    pub fn show_live(&self, live: &[FollowedChannel]) {
        let list = with_live(&self.shown.borrow(), live);
        self.show(&list);
    }

    /// Draws the list as the engine gave it: live ones first.
    pub fn show(&self, channels: &[FollowedChannel]) {
        *self.shown.borrow_mut() = channels.to_vec();
        while let Some(child) = self.rows.first_child() {
            self.rows.remove(&child);
        }
        let strings = self.strings;
        let live = channels.iter().filter(|f| f.live.is_some()).count();
        self.window.set_title(Some(&fill(
            strings.followed_title,
            &[
                ("count", &channels.len().to_string()),
                ("live", &live.to_string()),
            ],
        )));
        if channels.is_empty() {
            self.rows.append(&dim(strings.followed_none));
            return;
        }
        let now = Timestamp::now();
        let offline = channels.len() - live;
        if live > 0 {
            self.rows
                .append(&heading(&format!("{} ({live})", strings.followed_live)));
        }
        for (i, followed) in channels.iter().enumerate() {
            if i == live && offline > 0 {
                self.rows.append(&heading(&format!(
                    "{} ({offline})",
                    strings.followed_offline_heading
                )));
            }
            let line = gtk::Box::new(gtk::Orientation::Horizontal, 8);
            line.set_margin_start(8);
            line.set_margin_end(8);
            line.set_margin_top(4);
            line.set_margin_bottom(4);

            let words = gtk::Box::new(gtk::Orientation::Vertical, 2);
            words.set_hexpand(true);
            let name = gtk::Label::builder()
                .label(name_of(&followed.channel))
                .xalign(0.0)
                .build();
            words.append(&name);
            if let Some(stream) = &followed.live {
                name.add_css_class("heading");
                // What it streams, in a line that fits: the whole title is
                // in the tooltip.
                let about = about(stream, strings);
                let label = dim(&about);
                label.set_ellipsize(gtk::pango::EllipsizeMode::End);
                label.set_tooltip_text(Some(&about));
                words.append(&label);
                words.append(&dim(&stream_stats(strings, &stream.stats, now)));
            }
            line.append(&words);

            if (self.joined)(&followed.channel) {
                let label = dim(strings.moderated_joined);
                label.set_valign(gtk::Align::Center);
                line.append(&label);
            } else {
                let button = gtk::Button::with_label(strings.moderated_join);
                button.set_valign(gtk::Align::Center);
                let join = self.join.clone();
                let login = followed.channel.login.to_string();
                button.connect_clicked(move |button| {
                    // Once: the channel list shows it arriving.
                    button.set_sensitive(false);
                    join(&login);
                });
                line.append(&button);
            }
            self.rows.append(&line);
        }
    }
}

/// The display name, and the login beside it when the two differ by more
/// than case: that is what /join and the list both use.
fn name_of(channel: &Channel) -> String {
    if channel
        .display_name
        .eq_ignore_ascii_case(channel.login.as_str())
    {
        channel.display_name.clone()
    } else {
        format!("{} ({})", channel.display_name, channel.login)
    }
}

fn heading(text: &str) -> gtk::Label {
    gtk::Label::builder()
        .label(text)
        .xalign(0.0)
        .css_classes(["heading", "dim-label"])
        .margin_start(8)
        .margin_top(10)
        .margin_bottom(2)
        .build()
}

fn dim(text: &str) -> gtk::Label {
    gtk::Label::builder()
        .label(text)
        .xalign(0.0)
        .css_classes(["dim-label"])
        .build()
}
