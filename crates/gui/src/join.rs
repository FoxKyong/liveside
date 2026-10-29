//! The small window that asks which channel to join. A field in the
//! channel panel had room for neither a channel's name nor the button's
//! word; a window of its own has room for both.

use gtk::prelude::*;
use liveside_client::lang::Strings;

/// Asks for a channel's name; `join` gets what was typed, without a
/// leading `#`. Several names separated by commas, or an address book
/// category, go through as typed: the engine reads them as `/join` does.
pub fn ask(
    parent: &gtk::ApplicationWindow,
    strings: &'static Strings,
    join: impl Fn(String) + 'static,
) {
    let window = gtk::Window::builder()
        .transient_for(parent)
        .modal(true)
        .title(strings.join_title)
        .default_width(360)
        .resizable(false)
        .build();
    crate::dialog::close_on_escape(&window);

    let rows = gtk::Box::new(gtk::Orientation::Vertical, 12);
    rows.set_margin_top(18);
    rows.set_margin_bottom(18);
    rows.set_margin_start(18);
    rows.set_margin_end(18);

    let entry = gtk::Entry::builder()
        .placeholder_text(strings.join_placeholder)
        .hexpand(true)
        .build();
    rows.append(&entry);

    let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    buttons.set_halign(gtk::Align::End);
    let cancel = gtk::Button::with_label(strings.cancel);
    let join_button = gtk::Button::with_label(strings.join_button);
    join_button.add_css_class("suggested-action");
    // Nothing to join until something is typed.
    join_button.set_sensitive(false);
    buttons.append(&cancel);
    buttons.append(&join_button);
    rows.append(&buttons);

    cancel.connect_clicked({
        let window = window.clone();
        move |_| window.close()
    });
    entry.connect_changed({
        let join_button = join_button.clone();
        move |entry| join_button.set_sensitive(!name(entry).is_empty())
    });
    let submit = std::rc::Rc::new({
        let window = window.clone();
        let entry = entry.clone();
        move || {
            let name = name(&entry);
            if name.is_empty() {
                return;
            }
            window.close();
            join(name);
        }
    });
    entry.connect_activate({
        let submit = submit.clone();
        move |_| submit()
    });
    join_button.connect_clicked(move |_| submit());

    window.set_child(Some(&rows));
    entry.grab_focus();
    window.present();
}

/// What was typed, as a channel's name.
fn name(entry: &gtk::Entry) -> String {
    entry.text().trim().trim_start_matches('#').to_owned()
}
