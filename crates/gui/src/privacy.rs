//! What this client does with the reader's data, in a window of its own.
//!
//! The same report the terminal prints for `/privacy`: who gets what with
//! the settings as they stand, whether it is happening at all, and how to
//! switch it off. A window can lay it out as a table, which is what a
//! list of "who gets what" wants to be.

use gtk::prelude::*;
use liveside_client::lang::Strings;
use liveside_proto::{DataFlow, DataFlowKind};

/// Opens the report. `flows` is what the engine last said; empty means it
/// has not said anything yet, which is itself worth saying.
///
/// It is held above `parent`, and modal when that is: opened from the
/// modal settings, a window above the main one could be neither scrolled
/// nor closed, since the settings took all input.
pub fn open(parent: &impl IsA<gtk::Window>, flows: &[DataFlow], strings: &'static Strings) {
    let page = gtk::Box::new(gtk::Orientation::Vertical, 12);
    page.set_margin_start(16);
    page.set_margin_end(16);
    page.set_margin_top(16);
    page.set_margin_bottom(16);

    page.append(
        &gtk::Label::builder()
            .label(strings.privacy_title)
            .xalign(0.0)
            .wrap(true)
            .css_classes(["title-4"])
            .build(),
    );

    if flows.is_empty() {
        page.append(
            &gtk::Label::builder()
                .label(strings.privacy_unknown)
                .xalign(0.0)
                .wrap(true)
                .css_classes(["dim-label"])
                .build(),
        );
    }

    let rows = gtk::ListBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .css_classes(["boxed-list"])
        .build();
    for flow in flows {
        let row = gtk::Box::new(gtk::Orientation::Vertical, 2);
        row.set_margin_start(8);
        row.set_margin_end(8);
        row.set_margin_top(8);
        row.set_margin_bottom(8);

        // Who, and whether it is happening: the one that is switched on is
        // the one worth reading first, so it is the one that is marked.
        let heading = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        heading.append(
            &gtk::Label::builder()
                .label(liveside_client::lang::from_engine(
                    strings,
                    &flow.party_code,
                    &[],
                    &flow.party,
                ))
                .xalign(0.0)
                .hexpand(true)
                .css_classes(["heading"])
                .build(),
        );
        heading.append(
            &gtk::Label::builder()
                .label(match flow.kind {
                    DataFlowKind::Network => strings.privacy_network,
                    DataFlowKind::Disk => strings.privacy_disk,
                })
                .css_classes(["dim-label"])
                .build(),
        );
        heading.append(
            &gtk::Label::builder()
                .label(if flow.enabled {
                    strings.privacy_on.trim()
                } else {
                    strings.privacy_off.trim()
                })
                .css_classes(if flow.enabled {
                    vec!["warning", "heading"]
                } else {
                    vec!["dim-label"]
                })
                .build(),
        );
        row.append(&heading);

        row.append(
            &gtk::Label::builder()
                .label(liveside_client::lang::from_engine(
                    strings,
                    &flow.code,
                    &flow.args,
                    &flow.detail,
                ))
                .xalign(0.0)
                .wrap(true)
                .build(),
        );
        if !flow.switch.is_empty() {
            row.append(
                &gtk::Label::builder()
                    .label(&flow.switch)
                    .xalign(0.0)
                    .wrap(true)
                    .css_classes(["dim-label", "monospace"])
                    .build(),
            );
        }
        rows.append(&row);
    }
    page.append(&rows);

    let scroll = gtk::ScrolledWindow::builder()
        .child(&page)
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vexpand(true)
        .build();
    let window = gtk::Window::builder()
        .transient_for(parent)
        .modal(parent.is_modal())
        .title(strings.privacy_title)
        .default_width(520)
        .default_height(520)
        .child(&scroll)
        .build();
    crate::dialog::close_on_escape(&window);
    window.present();
}
