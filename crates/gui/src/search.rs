//! Searching the chat on screen: a bar above it, opened with Ctrl-F.
//!
//! What it finds is marked in the text, and the newest match is shown
//! first: in a chat the line wanted is usually the latest one that says
//! it. Enter goes further back, Shift+Enter forward again, Escape closes
//! the bar and takes the marks away. Going to a match scrolls the view
//! up, which stops it following new lines, so what was found stays put.

use gtk::prelude::*;
use liveside_client::lang::{Strings, fill};

/// The bar and its parts.
pub struct SearchBar {
    pub root: gtk::SearchBar,
    pub entry: gtk::SearchEntry,
    count: gtk::Label,
    pub older: gtk::Button,
    pub newer: gtk::Button,
}

impl SearchBar {
    pub fn new(strings: &'static Strings) -> Self {
        let entry = gtk::SearchEntry::builder()
            .placeholder_text(strings.search_placeholder)
            .hexpand(true)
            .build();
        let count = gtk::Label::builder()
            .css_classes(["dim-label"])
            .width_chars(10)
            .build();
        let older = gtk::Button::builder()
            .icon_name("go-up-symbolic")
            .tooltip_text(strings.search_older)
            .build();
        let newer = gtk::Button::builder()
            .icon_name("go-down-symbolic")
            .tooltip_text(strings.search_newer)
            .build();
        let line = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        line.append(&entry);
        line.append(&count);
        line.append(&older);
        line.append(&newer);
        let root = gtk::SearchBar::builder()
            .child(&line)
            .show_close_button(true)
            .build();
        root.connect_entry(&entry);
        Self {
            root,
            entry,
            count,
            older,
            newer,
        }
    }

    pub fn is_open(&self) -> bool {
        self.root.is_search_mode()
    }

    /// Opens the bar with the field ready to type into.
    pub fn open(&self) {
        self.root.set_search_mode(true);
        self.entry.grab_focus();
        // What was searched last stays, selected, so typing replaces it.
        self.entry.select_region(0, -1);
    }

    pub fn close(&self) {
        self.root.set_search_mode(false);
    }

    /// Says where the search is: "3 of 12", or that nothing was found.
    pub fn show(&self, found: Option<(usize, usize)>, strings: &'static Strings) {
        let said = match found {
            Some((at, total)) => fill(
                strings.search_count,
                &[("at", &at.to_string()), ("total", &total.to_string())],
            ),
            None if self.entry.text().is_empty() => String::new(),
            None => strings.search_none.to_owned(),
        };
        self.count.set_text(&said);
    }
}
