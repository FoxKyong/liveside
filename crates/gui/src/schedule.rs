//! When the stream is next: the planned broadcasts, and the two things a
//! broadcaster does to them in a hurry — calling one off, and saying they
//! are away.
//!
//! Twitch distinguishes calling off one occurrence of a weekly broadcast
//! from removing it altogether, and so does this: the first is "something
//! came up this week", the second is "this is not a thing any more".

use std::rc::Rc;

use gtk::prelude::*;
use liveside_client::lang::{Strings, fill};
use liveside_proto::model::Schedule;

/// What the window asks the engine to do.
pub enum Ask {
    Cancel {
        segment: String,
        canceled: bool,
    },
    Delete {
        segment: String,
    },
    Add {
        start: String,
        minutes: u32,
        title: String,
        weekly: bool,
    },
    Vacation(Option<(String, String)>),
}

type Act = Rc<dyn Fn(Ask)>;

pub struct ScheduleWindow {
    window: gtk::Window,
    rows: gtk::ListBox,
    away: gtk::Label,
    act: Act,
}

impl ScheduleWindow {
    pub fn open(parent: &gtk::ApplicationWindow, strings: &'static Strings, act: Act) -> Rc<Self> {
        let rows = gtk::ListBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .build();
        let scroll = gtk::ScrolledWindow::builder()
            .child(&rows)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .build();

        let away = gtk::Label::builder()
            .xalign(0.0)
            .wrap(true)
            .css_classes(["dim-label"])
            .build();

        // Adding one: when, how long, what it is called, and whether it
        // comes round every week.
        let start = gtk::Entry::builder()
            .placeholder_text(strings.schedule_start)
            .hexpand(true)
            .build();
        let minutes = crate::dialog::number(30.0, 1380.0, 30.0);
        minutes.set_value(120.0);
        let title = gtk::Entry::builder()
            .placeholder_text(strings.schedule_title)
            .hexpand(true)
            .build();
        let weekly = gtk::CheckButton::with_label(strings.schedule_every_week);
        let add = gtk::Button::with_label(strings.schedule_add);
        add.add_css_class("suggested-action");
        add.connect_clicked({
            let act = act.clone();
            let start = start.clone();
            let minutes = minutes.clone();
            let title = title.clone();
            let weekly = weekly.clone();
            move |_| {
                let Some(when) = rfc3339(&start.text()) else {
                    // Said by the placeholder; an unreadable time is left
                    // in the field rather than guessed at.
                    return;
                };
                act(Ask::Add {
                    start: when,
                    minutes: minutes.value() as u32,
                    title: title.text().trim().to_string(),
                    weekly: weekly.is_active(),
                });
                start.set_text("");
                title.set_text("");
            }
        });

        let adding = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        adding.append(&start);
        adding.append(&minutes);
        adding.append(&title);
        adding.append(&weekly);
        adding.append(&add);

        // Being away is a date range, typed the same way.
        let from = gtk::Entry::builder()
            .placeholder_text(strings.schedule_start)
            .hexpand(true)
            .build();
        let to = gtk::Entry::builder()
            .placeholder_text(strings.schedule_start)
            .hexpand(true)
            .build();
        let vacation = gtk::Button::with_label(strings.schedule_vacation_on);
        vacation.connect_clicked({
            let act = act.clone();
            let from = from.clone();
            let to = to.clone();
            move |_| {
                let (Some(start), Some(end)) = (rfc3339(&from.text()), rfc3339(&to.text())) else {
                    return;
                };
                act(Ask::Vacation(Some((start, end))));
            }
        });
        let back = gtk::Button::with_label(strings.schedule_vacation_off);
        back.connect_clicked({
            let act = act.clone();
            move |_| act(Ask::Vacation(None))
        });

        let away_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        away_row.append(&from);
        away_row.append(&to);
        away_row.append(&vacation);
        away_row.append(&back);

        let page = gtk::Box::new(gtk::Orientation::Vertical, 8);
        page.append(&away);
        page.append(&scroll);
        page.append(&adding);
        page.append(&away_row);
        page.set_margin_start(12);
        page.set_margin_end(12);
        page.set_margin_top(12);
        page.set_margin_bottom(12);

        let window = gtk::Window::builder()
            .transient_for(parent)
            .modal(false)
            .title(strings.schedule_window)
            .default_width(680)
            .default_height(420)
            .child(&page)
            .build();
        crate::dialog::close_on_escape(&window);
        window.present();

        Rc::new(Self {
            window,
            rows,
            away,
            act,
        })
    }

    pub fn is_open(&self) -> bool {
        self.window.is_visible()
    }

    pub fn show(&self, schedule: &Schedule, strings: &'static Strings) {
        match &schedule.vacation {
            Some((start, end)) => {
                self.away.set_text(&fill(
                    strings.schedule_away,
                    &[("start", &day(start)), ("end", &day(end))],
                ));
                self.away.set_visible(true);
            }
            None => self.away.set_visible(false),
        }

        while let Some(child) = self.rows.first_child() {
            self.rows.remove(&child);
        }
        if schedule.segments.is_empty() {
            self.rows.append(
                &gtk::Label::builder()
                    .label(strings.schedule_none)
                    .xalign(0.0)
                    .css_classes(["dim-label"])
                    .margin_start(8)
                    .margin_top(8)
                    .build(),
            );
        }
        for segment in &schedule.segments {
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
            row.set_margin_start(8);
            row.set_margin_end(8);
            row.set_margin_top(6);
            row.set_margin_bottom(6);

            let mut said = fill(
                strings.schedule_line,
                &[
                    ("start", &when(&segment.start)),
                    (
                        "title",
                        &match segment.title.is_empty() {
                            true => segment.category.clone(),
                            false => segment.title.clone(),
                        },
                    ),
                ],
            );
            if segment.weekly {
                said.push(' ');
                said.push_str(strings.schedule_weekly);
            }
            if segment.canceled {
                said.push(' ');
                said.push_str(strings.schedule_canceled);
            }
            row.append(
                &gtk::Label::builder()
                    .label(said)
                    .xalign(0.0)
                    .hexpand(true)
                    .wrap(true)
                    .build(),
            );

            let off = gtk::Button::with_label(match segment.canceled {
                true => strings.schedule_restore,
                false => strings.schedule_cancel,
            });
            let act = self.act.clone();
            let id = segment.id.clone();
            let now = segment.canceled;
            off.connect_clicked(move |_| {
                act(Ask::Cancel {
                    segment: id.clone(),
                    canceled: !now,
                })
            });
            row.append(&off);

            let remove = gtk::Button::with_label(strings.schedule_delete);
            let act = self.act.clone();
            let id = segment.id.clone();
            remove.connect_clicked(move |_| {
                act(Ask::Delete {
                    segment: id.clone(),
                })
            });
            row.append(&remove);

            self.rows.append(&row);
        }
    }
}

/// `2026-10-07 18:00` as Twitch wants it. Anything else is refused, so a
/// half-typed time never becomes a broadcast at midnight.
fn rfc3339(typed: &str) -> Option<String> {
    let typed = typed.trim();
    let (day, time) = typed.split_once(char::is_whitespace)?;
    if day.len() != 10 || !day.starts_with(|c: char| c.is_ascii_digit()) {
        return None;
    }
    let time = match time.len() {
        5 => format!("{time}:00"),
        8 => time.to_owned(),
        _ => return None,
    };
    Some(format!("{day}T{time}Z"))
}

fn day(stamp: &str) -> String {
    stamp.split('T').next().unwrap_or(stamp).to_owned()
}

fn when(stamp: &str) -> String {
    match stamp.split_once('T') {
        Some((day, rest)) => format!("{day} {}", rest.chars().take(5).collect::<String>()),
        None => stamp.to_owned(),
    }
}
