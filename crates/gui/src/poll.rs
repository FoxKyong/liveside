//! The poll in the channel: the bar that shows how it stands, and the
//! dialog that starts one.
//!
//! A poll is time-critical — it runs for a minute or two and the whole
//! point is watching the numbers move — so the running total belongs on
//! the screen the broadcaster is already reading, not behind a menu.

use std::cell::RefCell;
use std::rc::Rc;

use gtk::prelude::*;
use liveside_client::lang::{Strings, fill};
use liveside_proto::model::Poll;

/// The bar above the message line: the question, every choice with its
/// share, and the two ways to stop it.
pub struct PollBar {
    root: gtk::Box,
    title: gtk::Label,
    choices: gtk::Box,
    /// What is drawn right now, so an update can tell a new poll from a
    /// new count of the same one.
    shown: RefCell<Option<Poll>>,
}

impl PollBar {
    pub fn new(strings: &'static Strings, end: Rc<dyn Fn(bool)>) -> Rc<Self> {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 4);
        root.set_visible(false);

        let heading = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let title = gtk::Label::builder()
            .xalign(0.0)
            .hexpand(true)
            .wrap(true)
            .css_classes(["heading"])
            .build();
        heading.append(&title);
        for (label, archive) in [(strings.poll_end, false), (strings.poll_cancel, true)] {
            let button = gtk::Button::with_label(label);
            let end = end.clone();
            button.connect_clicked(move |_| end(archive));
            heading.append(&button);
        }
        root.append(&heading);

        let choices = gtk::Box::new(gtk::Orientation::Vertical, 2);
        root.append(&choices);

        Rc::new(Self {
            root,
            title,
            choices,
            shown: RefCell::new(None),
        })
    }

    pub fn root(&self) -> &gtk::Box {
        &self.root
    }

    /// Draws a poll, or hides the bar when there is none.
    pub fn show(&self, poll: Option<&Poll>, strings: &'static Strings) {
        let Some(poll) = poll else {
            self.root.set_visible(false);
            *self.shown.borrow_mut() = None;
            return;
        };
        self.title.set_text(&poll.title);

        // Rebuilt rather than updated: a poll has at most five choices,
        // and keeping two lists in step costs more than it saves.
        while let Some(child) = self.choices.first_child() {
            self.choices.remove(&child);
        }
        let total: u32 = poll.choices.iter().map(|choice| choice.votes).sum();
        for choice in &poll.choices {
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
            row.append(
                &gtk::Label::builder()
                    .label(&choice.title)
                    .xalign(0.0)
                    .width_chars(16)
                    .max_width_chars(24)
                    .ellipsize(gtk::pango::EllipsizeMode::End)
                    .build(),
            );
            let share = match total {
                0 => 0.0,
                total => f64::from(choice.votes) / f64::from(total),
            };
            row.append(
                &gtk::ProgressBar::builder()
                    .fraction(share)
                    .hexpand(true)
                    .valign(gtk::Align::Center)
                    .build(),
            );
            row.append(
                &gtk::Label::builder()
                    .label(fill(
                        strings.poll_votes,
                        &[("votes", &choice.votes.to_string())],
                    ))
                    .css_classes(["dim-label"])
                    .build(),
            );
            self.choices.append(&row);
        }
        self.root.set_visible(true);
        *self.shown.borrow_mut() = Some(poll.clone());
    }
}

/// Asks what to put to the channel: a question, up to five choices, and
/// how long voting stays open. Twitch wants two choices at least, so the
/// button stays dead until there are two.
pub fn ask(
    parent: &gtk::ApplicationWindow,
    strings: &'static Strings,
    start: impl Fn(String, Vec<String>, u32) + 'static,
) {
    let window = gtk::Window::builder()
        .transient_for(parent)
        .modal(true)
        .title(strings.poll_open.trim_end_matches('…'))
        .default_width(420)
        .build();
    crate::dialog::close_on_escape(&window);

    let rows = gtk::Box::new(gtk::Orientation::Vertical, 8);
    rows.set_margin_start(16);
    rows.set_margin_end(16);
    rows.set_margin_top(16);
    rows.set_margin_bottom(16);

    let title = gtk::Entry::builder()
        .placeholder_text(strings.poll_title)
        .activates_default(true)
        .build();
    rows.append(&title);

    // Twitch takes between two and five; the empty ones are left out.
    let choices: Vec<gtk::Entry> = (1..=5)
        .map(|n| {
            gtk::Entry::builder()
                .placeholder_text(fill(strings.poll_choice, &[("n", &n.to_string())]))
                .build()
        })
        .collect();
    for choice in &choices {
        rows.append(choice);
    }

    let seconds = crate::dialog::number(15.0, 180.0, 15.0);
    seconds.set_value(60.0);
    let seconds_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    seconds_row.append(&gtk::Label::new(Some(strings.poll_minutes)));
    seconds_row.append(&seconds);
    rows.append(&seconds_row);

    let create = gtk::Button::with_label(strings.poll_create);
    create.add_css_class("suggested-action");
    create.set_halign(gtk::Align::End);
    create.connect_clicked({
        let title = title.clone();
        let choices = choices.clone();
        let seconds = seconds.clone();
        let window = window.clone();
        move |_| {
            let question = title.text().trim().to_string();
            let answers: Vec<String> = choices
                .iter()
                .map(|entry| entry.text().trim().to_string())
                .filter(|text| !text.is_empty())
                .collect();
            if question.is_empty() || answers.len() < 2 {
                return;
            }
            start(question, answers, seconds.value() as u32);
            window.close();
        }
    });
    rows.append(&create);

    window.set_child(Some(&rows));
    window.present();
}
