//! A list of people in a window of its own: the channel's moderators, its
//! VIPs, whoever is banned from it, its followers or its subscribers.
//!
//! A line of chat can hold half a dozen names and not four hundred, and a
//! name in a list is usually the beginning of a question rather than the
//! end of one — so each row opens that person's card, and a banned one can
//! be let back in from here.

use std::rc::Rc;

use gtk::prelude::*;
use liveside_client::lang::Strings;
use liveside_proto::event::{PeopleList, Person};

/// Opens the list. `picked` is called with a name when a row is clicked,
/// `unban` with one when somebody is to be let back in.
#[allow(clippy::too_many_arguments)]
pub fn open(
    parent: &gtk::ApplicationWindow,
    list: PeopleList,
    people: &[Person],
    total: Option<u64>,
    points: Option<u64>,
    strings: &'static Strings,
    picked: Rc<dyn Fn(&str)>,
    unban: Rc<dyn Fn(&str)>,
) {
    let title = liveside_client::lang::people_heading(strings, list, people.len(), total, points);

    let rows = gtk::ListBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .build();
    // The heading above says it all when Twitch counts followers but will
    // not name them: "nobody" would be wrong.
    if people.is_empty() && !total.is_some_and(|t| t > 0) {
        rows.append(
            &gtk::Label::builder()
                .label(strings.list_empty)
                .xalign(0.0)
                .css_classes(["dim-label"])
                .margin_start(8)
                .margin_top(8)
                .build(),
        );
    }
    for person in people {
        let line = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        line.set_margin_start(8);
        line.set_margin_end(8);
        line.set_margin_top(4);
        line.set_margin_bottom(4);

        // The name opens the card: a list of names is where a question
        // about one of them starts.
        let name = gtk::Button::builder()
            .label(&person.name)
            .css_classes(["flat"])
            .hexpand(true)
            .halign(gtk::Align::Start)
            .build();
        let picked = picked.clone();
        let who = person.name.clone();
        name.connect_clicked(move |_| picked(&who));
        line.append(&name);

        if let Some(detail) = liveside_client::lang::person_detail(strings, list, person) {
            line.append(
                &gtk::Label::builder()
                    .label(detail)
                    .css_classes(["dim-label"])
                    .build(),
            );
        }
        if list == PeopleList::Banned {
            let button = gtk::Button::with_label(strings.action_unban);
            let unban = unban.clone();
            let who = person.name.clone();
            button.connect_clicked(move |button| {
                // Said once; the list is a snapshot and will not refresh
                // itself, so a second click would only fail.
                button.set_sensitive(false);
                unban(&who);
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
        .title(&title)
        .default_width(360)
        .default_height(480)
        .child(&{
            // The heading again, wrapped: how many there are, and why
            // only some are named, is longer than a title bar.
            let column = gtk::Box::new(gtk::Orientation::Vertical, 0);
            column.append(
                &gtk::Label::builder()
                    .label(&title)
                    .wrap(true)
                    .xalign(0.0)
                    .css_classes(["heading"])
                    .margin_start(12)
                    .margin_end(12)
                    .margin_top(8)
                    .margin_bottom(8)
                    .build(),
            );
            column.append(&scroll);
            column
        })
        .build();
    crate::dialog::close_on_escape(&window);
    window.present();
}
