//! The channels this account moderates, in a window of their own: the
//! natural answer to "what should I join". Each row joins its channel; a
//! channel already joined says so instead.

use std::rc::Rc;

use gtk::prelude::*;
use liveside_client::lang::{Strings, fill};
use liveside_proto::model::Channel;

/// Opens the list. `joined` says whether a channel is joined already;
/// `join` joins one by its login.
pub fn open(
    parent: &gtk::ApplicationWindow,
    channels: &[Channel],
    strings: &'static Strings,
    joined: impl Fn(&Channel) -> bool,
    join: Rc<dyn Fn(&str)>,
) {
    let rows = gtk::ListBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .build();
    if channels.is_empty() {
        rows.append(
            &gtk::Label::builder()
                .label(strings.moderated_none)
                .xalign(0.0)
                .css_classes(["dim-label"])
                .margin_start(8)
                .margin_top(8)
                .build(),
        );
    }
    for channel in channels {
        let line = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        line.set_margin_start(8);
        line.set_margin_end(8);
        line.set_margin_top(4);
        line.set_margin_bottom(4);
        // The display name, and the login beside it when the two differ
        // by more than case: that is what /join and the list both use.
        let name = if channel
            .display_name
            .eq_ignore_ascii_case(channel.login.as_str())
        {
            channel.display_name.clone()
        } else {
            format!("{} ({})", channel.display_name, channel.login)
        };
        line.append(
            &gtk::Label::builder()
                .label(name)
                .xalign(0.0)
                .hexpand(true)
                .build(),
        );
        if joined(channel) {
            line.append(
                &gtk::Label::builder()
                    .label(strings.moderated_joined)
                    .css_classes(["dim-label"])
                    .build(),
            );
        } else {
            let button = gtk::Button::with_label(strings.moderated_join);
            let join = join.clone();
            let login = channel.login.to_string();
            button.connect_clicked(move |button| {
                // Once: the channel list shows it arriving.
                button.set_sensitive(false);
                join(&login);
            });
            line.append(&button);
        }
        rows.append(&line);
    }

    let scroll = gtk::ScrolledWindow::builder()
        .child(&rows)
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vexpand(true)
        .build();
    let window = gtk::Window::builder()
        .transient_for(parent)
        .modal(false)
        .title(fill(
            strings.moderated_title,
            &[("count", &channels.len().to_string())],
        ))
        .default_width(360)
        .default_height(420)
        .child(&scroll)
        .build();
    crate::dialog::close_on_escape(&window);
    window.present();
}
