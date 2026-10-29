//! The prediction in the channel: the bar that shows the betting, and the
//! dialog that starts one.
//!
//! A prediction has one state a poll does not: betting closes, and then
//! somebody has to say who was right — within the day, or Twitch calls it
//! off and gives the points back. That decision is what the bar is for,
//! so once it is locked every outcome carries its own "pay out" button.

use std::rc::Rc;

use gtk::prelude::*;
use liveside_client::lang::{Strings, fill};
use liveside_proto::model::{Prediction, PredictionStatus};

/// What the bar asks the window to do.
pub enum Ask {
    Lock,
    Cancel,
    Pay(String),
}

pub struct PredictionBar {
    root: gtk::Box,
    title: gtk::Label,
    outcomes: gtk::Box,
    lock: gtk::Button,
    act: Rc<dyn Fn(Ask)>,
}

impl PredictionBar {
    pub fn new(strings: &'static Strings, act: Rc<dyn Fn(Ask)>) -> Rc<Self> {
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

        let lock = gtk::Button::with_label(strings.pred_lock);
        lock.connect_clicked({
            let act = act.clone();
            move |_| act(Ask::Lock)
        });
        heading.append(&lock);

        let cancel = gtk::Button::with_label(strings.pred_cancel);
        cancel.connect_clicked({
            let act = act.clone();
            move |_| act(Ask::Cancel)
        });
        heading.append(&cancel);
        root.append(&heading);

        let outcomes = gtk::Box::new(gtk::Orientation::Vertical, 2);
        root.append(&outcomes);

        Rc::new(Self {
            root,
            title,
            outcomes,
            lock,
            act,
        })
    }

    pub fn root(&self) -> &gtk::Box {
        &self.root
    }

    pub fn show(&self, prediction: Option<&Prediction>, strings: &'static Strings) {
        let Some(prediction) = prediction else {
            self.root.set_visible(false);
            return;
        };
        let locked = prediction.status == PredictionStatus::Locked;
        self.title.set_text(&match locked {
            // Once betting is closed the question is no longer the
            // question; what is needed is a decision.
            true => format!("{} — {}", prediction.title, strings.pred_waiting),
            false => prediction.title.clone(),
        });
        self.lock.set_visible(!locked);

        while let Some(child) = self.outcomes.first_child() {
            self.outcomes.remove(&child);
        }
        let total: u64 = prediction.outcomes.iter().map(|o| o.points).sum();
        for outcome in &prediction.outcomes {
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
            row.append(
                &gtk::Label::builder()
                    .label(&outcome.title)
                    .xalign(0.0)
                    .width_chars(16)
                    .max_width_chars(24)
                    .ellipsize(gtk::pango::EllipsizeMode::End)
                    .build(),
            );
            let share = match total {
                0 => 0.0,
                total => outcome.points as f64 / total as f64,
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
                        strings.pred_points,
                        &[
                            ("users", &outcome.users.to_string()),
                            ("points", &outcome.points.to_string()),
                        ],
                    ))
                    .css_classes(["dim-label"])
                    .build(),
            );
            // The whole reason this is in the chat window rather than the
            // dashboard: paying out is one click, next to the numbers it
            // is decided from.
            if locked {
                let pay = gtk::Button::with_label(strings.pred_pay);
                pay.add_css_class("suggested-action");
                let act = self.act.clone();
                let id = outcome.id.clone();
                pay.connect_clicked(move |_| act(Ask::Pay(id.clone())));
                row.append(&pay);
            }
            self.outcomes.append(&row);
        }
        self.root.set_visible(true);
    }
}

/// Asks what to predict: a question, up to ten outcomes, and how long
/// betting stays open. Twitch wants at least two outcomes.
pub fn ask(
    parent: &gtk::ApplicationWindow,
    strings: &'static Strings,
    start: impl Fn(String, Vec<String>, u32) + 'static,
) {
    let window = gtk::Window::builder()
        .transient_for(parent)
        .modal(true)
        .title(strings.pred_open.trim_end_matches('…'))
        .default_width(420)
        .build();
    crate::dialog::close_on_escape(&window);

    let rows = gtk::Box::new(gtk::Orientation::Vertical, 8);
    rows.set_margin_start(16);
    rows.set_margin_end(16);
    rows.set_margin_top(16);
    rows.set_margin_bottom(16);

    let title = gtk::Entry::builder()
        .placeholder_text(strings.pred_title)
        .activates_default(true)
        .build();
    rows.append(&title);

    // Four of the ten Twitch allows: more than that is a form, and two is
    // what nearly every prediction uses.
    let outcomes: Vec<gtk::Entry> = (1..=4)
        .map(|n| {
            gtk::Entry::builder()
                .placeholder_text(fill(strings.pred_outcome, &[("n", &n.to_string())]))
                .build()
        })
        .collect();
    for outcome in &outcomes {
        rows.append(outcome);
    }

    let seconds = crate::dialog::number(30.0, 1800.0, 30.0);
    seconds.set_value(120.0);
    let seconds_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    seconds_row.append(&gtk::Label::new(Some(strings.pred_seconds)));
    seconds_row.append(&seconds);
    rows.append(&seconds_row);

    let create = gtk::Button::with_label(strings.pred_create);
    create.add_css_class("suggested-action");
    create.set_halign(gtk::Align::End);
    create.connect_clicked({
        let title = title.clone();
        let outcomes = outcomes.clone();
        let seconds = seconds.clone();
        let window = window.clone();
        move |_| {
            let question = title.text().trim().to_string();
            let answers: Vec<String> = outcomes
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
