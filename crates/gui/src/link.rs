//! Opening a link from chat: asking first, and saying where it really goes.
//!
//! The question shows the whole address and picks out the host, which is
//! the part that decides where the browser ends up. A host spelled with
//! letters from another alphabet is also shown the way the browser will
//! ask for it, in punycode, which makes a Cyrillic "а" in "pаypal" visible.

use gtk::glib;
use gtk::prelude::*;
use liveside_client::lang::{Strings, fill};
use liveside_client::links::{host, looks_foreign};

/// Asks whether to open `url`, written in chat as `text`. `open` runs if
/// the answer is yes. `tell`, when the link can be reported to the chat it
/// was in, tells that chat what its address is made of.
pub fn confirm(
    parent: &gtk::ApplicationWindow,
    text: &str,
    url: &str,
    strings: &'static Strings,
    open: impl Fn() + 'static,
    tell: Option<Box<dyn Fn()>>,
) {
    let window = gtk::Window::builder()
        .transient_for(parent)
        .modal(true)
        .title(strings.link_title)
        .default_width(520)
        .resizable(false)
        .build();
    crate::dialog::close_on_escape(&window);

    let rows = gtk::Box::new(gtk::Orientation::Vertical, 12);
    rows.set_margin_top(18);
    rows.set_margin_bottom(18);
    rows.set_margin_start(18);
    rows.set_margin_end(18);

    // The whole address, as it will be opened, never shortened: the
    // part that matters is often at the end.
    let address = gtk::Label::builder()
        .label(url)
        .xalign(0.0)
        .wrap(true)
        .wrap_mode(gtk::pango::WrapMode::Char)
        .selectable(true)
        .build();
    address.add_css_class("monospace");
    rows.append(&address);

    let shown_host = host(text);
    let goes_to = gtk::Label::builder()
        .use_markup(true)
        .label(fill(
            strings.link_host,
            &[(
                "host",
                &format!("<b>{}</b>", glib::markup_escape_text(shown_host)),
            )],
        ))
        .xalign(0.0)
        .wrap(true)
        .build();
    rows.append(&goes_to);

    if looks_foreign(url) {
        let warning = gtk::Label::builder()
            .use_markup(true)
            .label(fill(
                strings.link_punycode,
                &[(
                    "punycode",
                    &format!("<b>{}</b>", glib::markup_escape_text(host(url))),
                )],
            ))
            .xalign(0.0)
            .wrap(true)
            .css_classes(["error"])
            .build();
        rows.append(&warning);
    }

    let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    buttons.set_halign(gtk::Align::End);
    buttons.set_margin_top(6);
    let cancel = gtk::Button::with_label(strings.link_cancel);
    cancel.connect_clicked({
        let window = window.clone();
        move |_| window.close()
    });
    let copy = gtk::Button::with_label(strings.link_copy);
    copy.connect_clicked({
        let window = window.clone();
        let url = url.to_owned();
        move |_| {
            window.clipboard().set_text(&url);
            window.close();
        }
    });
    let open_button = gtk::Button::with_label(strings.link_open);
    open_button.add_css_class("suggested-action");
    open_button.connect_clicked({
        let window = window.clone();
        move |_| {
            window.close();
            open();
        }
    });
    if let Some(tell) = tell {
        // On the far side from Open, and once only: the chat is told one
        // time, and the window stays for the rest of the question.
        buttons.set_halign(gtk::Align::Fill);
        let button = gtk::Button::with_label(strings.link_tell);
        button.set_hexpand(true);
        button.set_halign(gtk::Align::Start);
        button.connect_clicked(move |button| {
            tell();
            button.set_sensitive(false);
        });
        buttons.append(&button);
    }
    buttons.append(&cancel);
    buttons.append(&copy);
    buttons.append(&open_button);
    rows.append(&buttons);

    window.set_child(Some(&rows));
    // Enter must not open a link the reader has not read: the focus starts
    // on Cancel.
    cancel.grab_focus();
    window.present();
}

/// Why a link did not open.
pub enum LaunchError {
    /// Nothing on the system is set to open web links. Seen under WSL2,
    /// which has neither a browser of its own nor a portal.
    NoBrowser,
    /// Anything else, as GTK said it.
    Other(String),
}

/// Hands the address to the browser the desktop has chosen.
pub fn launch(
    parent: &gtk::ApplicationWindow,
    url: &str,
    failed: impl FnOnce(LaunchError) + 'static,
) {
    let scheme = url.split(':').next().unwrap_or_default().to_owned();
    gtk::UriLauncher::new(url).launch(
        Some(parent),
        None::<&gtk::gio::Cancellable>,
        move |result| {
            if let Err(error) = result {
                // GTK passes on GIO's reason only as words, in the
                // system's language, so it is asked again here. Inside
                // the Flatpak the portal opens links and the sandbox has
                // no handler of its own to find.
                let sandboxed = std::path::Path::new("/.flatpak-info").exists();
                let none = gtk::gio::AppInfo::default_for_uri_scheme(&scheme).is_none();
                failed(if none && !sandboxed {
                    LaunchError::NoBrowser
                } else {
                    LaunchError::Other(error.to_string())
                });
            }
        },
    );
}
