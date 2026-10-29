//! The settings window: the same rows as the terminal client, drawn as the
//! widgets a graphical program uses.
//!
//! The rows come from `liveside_client::settings`, so neither frontend can offer
//! a setting the other does not have, and both write the same file.

use std::cell::RefCell;
use std::rc::Rc;

use gtk::prelude::*;
use liveside_client::lang::{Language, Strings, fill};
use liveside_client::settings::{Settings, Value};

/// Opens the window. `applied` runs after a save, so the caller can follow
/// a change of language without restarting.
pub fn open(
    parent: &gtk::ApplicationWindow,
    settings: Rc<RefCell<Settings>>,
    text: &'static Strings,
    applied: impl Fn(Language, &liveside_client::Config) + 'static,
    privacy: impl Fn(&gtk::Window) + 'static,
) {
    let window = gtk::Window::builder()
        .transient_for(parent)
        .modal(true)
        .title(strip(text.settings_title))
        .default_width(560)
        .build();
    crate::dialog::close_on_escape(&window);
    // What was changed last time and not saved is not shown as if it was.
    settings.borrow_mut().reload();

    let rows = gtk::Box::new(gtk::Orientation::Vertical, 12);
    rows.set_margin_top(12);
    // Room for the scrollbar, which would otherwise sit on the fields.
    rows.set_margin_end(12);

    for (index, row) in settings.borrow().rows.iter().enumerate() {
        if let Some(section) = settings.borrow().heading(index) {
            let heading = gtk::Label::builder()
                .label(section.title(text))
                .xalign(0.0)
                .css_classes(["heading"])
                .build();
            // Space above every heading but the first, so a section reads
            // as a group of its own.
            if index > 0 {
                heading.set_margin_top(12);
            }
            rows.append(&heading);
        }
        let line = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        // The help belongs to the whole row: a setting that opens another
        // connection to Twitch should say so wherever the pointer rests,
        // not only over its name.
        line.set_tooltip_text(Some(row.help));
        let label = gtk::Label::builder()
            .label(row.label)
            .xalign(0.0)
            .width_chars(22)
            .build();
        line.append(&label);

        // One widget per kind of setting, so the file cannot be edited into
        // a state the window cannot show.
        match &row.value {
            Value::Switch(on) => {
                let switch = gtk::Switch::builder()
                    .active(*on)
                    .valign(gtk::Align::Center)
                    .build();
                switch.connect_active_notify({
                    let settings = settings.clone();
                    move |switch| {
                        if let Value::Switch(on) = &mut settings.borrow_mut().rows[index].value {
                            *on = switch.is_active();
                        }
                    }
                });
                line.append(&switch);
            }
            Value::Number {
                value,
                min,
                max,
                step,
            } => {
                let spin = crate::dialog::number(*min as f64, *max as f64, *step as f64);
                spin.set_value(*value as f64);
                spin.connect_value_changed({
                    let settings = settings.clone();
                    move |spin| {
                        if let Value::Number { value, .. } =
                            &mut settings.borrow_mut().rows[index].value
                        {
                            *value = spin.value() as usize;
                        }
                    }
                });
                line.append(&spin);
            }
            Value::Words(words) => {
                let entry = gtk::Entry::builder()
                    .text(words.join(", "))
                    .hexpand(true)
                    .placeholder_text(text.settings_none)
                    .build();
                entry.connect_changed({
                    let settings = settings.clone();
                    move |entry| {
                        let typed: Vec<String> = entry
                            .text()
                            .split(',')
                            .map(|word| word.trim().trim_start_matches('#').to_owned())
                            .filter(|word| !word.is_empty())
                            .collect();
                        if let Value::Words(words) = &mut settings.borrow_mut().rows[index].value {
                            *words = typed;
                        }
                    }
                });
                line.append(&entry);
            }
            Value::Text(current) => {
                let entry = gtk::Entry::builder()
                    .text(current.as_str())
                    .hexpand(true)
                    .placeholder_text(row.when_empty(text))
                    .build();
                entry.connect_changed({
                    let settings = settings.clone();
                    move |entry| {
                        if let Value::Text(target) = &mut settings.borrow_mut().rows[index].value {
                            *target = entry.text().trim().to_owned();
                        }
                    }
                });
                line.append(&entry);
            }
            // The stored key is never put back on screen: the field starts
            // empty, says whether there is one, and what is typed replaces
            // it on saving.
            Value::Secret { stored, .. } => {
                let entry = gtk::PasswordEntry::builder()
                    .hexpand(true)
                    .show_peek_icon(true)
                    .placeholder_text(if *stored {
                        text.key_set
                    } else {
                        text.key_not_set
                    })
                    .build();
                entry.connect_changed({
                    let settings = settings.clone();
                    move |entry| {
                        let typed = entry.text().trim().to_owned();
                        if let Value::Secret { typed: target, .. } =
                            &mut settings.borrow_mut().rows[index].value
                        {
                            *target = (!typed.is_empty()).then_some(typed);
                        }
                    }
                });
                line.append(&entry);
            }
            // A rule a line, checked as it is typed: what cannot be read
            // is said below the field, rule by rule, before saving refuses
            // it.
            Value::Rules(rules) => {
                // Tab moves on, as in every other field: a rule never
                // has a tab in it, and a field that keeps the key traps
                // whoever came in with it.
                let view = gtk::TextView::builder()
                    .accepts_tab(false)
                    .monospace(true)
                    .wrap_mode(gtk::WrapMode::WordChar)
                    .top_margin(4)
                    .bottom_margin(4)
                    .left_margin(6)
                    .right_margin(6)
                    .build();
                view.buffer().set_text(&rules.join("\n"));
                crate::dialog::leave_at_edges(&view);
                let frame = gtk::ScrolledWindow::builder()
                    .child(&view)
                    .hscrollbar_policy(gtk::PolicyType::Never)
                    .min_content_height(80)
                    .max_content_height(200)
                    .propagate_natural_height(true)
                    .has_frame(true)
                    .build();
                let problems = gtk::Label::builder()
                    .xalign(0.0)
                    .wrap(true)
                    .visible(false)
                    .css_classes(["error"])
                    .build();
                let column = gtk::Box::new(gtk::Orientation::Vertical, 4);
                column.set_hexpand(true);
                column.append(&frame);
                column.append(&problems);
                let key = row.key;
                view.buffer().connect_changed({
                    let settings = settings.clone();
                    move |buffer| {
                        let written = buffer.text(&buffer.start_iter(), &buffer.end_iter(), false);
                        let typed: Vec<String> = written
                            .lines()
                            .map(|line| line.trim().to_owned())
                            .filter(|line| !line.is_empty())
                            .collect();
                        let said: Vec<String> = typed
                            .iter()
                            .enumerate()
                            .filter_map(|(i, line)| {
                                let error =
                                    liveside_client::settings::check_line(key, line, text).err()?;
                                Some(format!("{}. {error}", i + 1))
                            })
                            .collect();
                        problems.set_text(&said.join("\n"));
                        problems.set_visible(!said.is_empty());
                        if let Value::Rules(rules) = &mut settings.borrow_mut().rows[index].value {
                            *rules = typed;
                        }
                    }
                });
                line.append(&column);
            }
            Value::Choice { options, index: at } => {
                let names: Vec<&str> = options.iter().map(|(_, meaning)| *meaning).collect();
                let list = gtk::StringList::new(&names);
                let drop = gtk::DropDown::builder()
                    .model(&list)
                    .selected(*at as u32)
                    .hexpand(true)
                    .build();
                drop.connect_selected_notify({
                    let settings = settings.clone();
                    move |drop| {
                        if let Value::Choice { index: at, .. } =
                            &mut settings.borrow_mut().rows[index].value
                        {
                            *at = drop.selected() as usize;
                        }
                    }
                });
                line.append(&drop);
            }
        }
        if row.restart {
            line.append(
                &gtk::Label::builder()
                    .label(text.settings_restart.trim())
                    .css_classes(["dim-label"])
                    .build(),
            );
        }
        rows.append(&line);
    }

    let message = gtk::Label::builder().xalign(0.0).wrap(true).build();
    // The settings scroll; what they say on saving and the buttons stay
    // in sight below them, however many settings there come to be.
    let scroll = gtk::ScrolledWindow::builder()
        .child(&rows)
        .hscrollbar_policy(gtk::PolicyType::Never)
        .propagate_natural_height(true)
        .max_content_height(560)
        .vexpand(true)
        .build();
    let outer = gtk::Box::new(gtk::Orientation::Vertical, 12);
    outer.set_margin_bottom(12);
    outer.set_margin_start(12);
    outer.set_margin_end(12);
    outer.append(&scroll);
    outer.append(&message);

    let save = gtk::Button::with_label(text.settings_save);
    save.add_css_class("suggested-action");
    save.connect_clicked({
        let settings = settings.clone();
        let message = message.clone();
        let window = window.clone();
        move |_| {
            let mut settings = settings.borrow_mut();
            let saved = settings.save();
            if let Some(said) = &settings.message {
                message.set_text(said);
            }
            if !saved {
                return;
            }
            let language = Language::parse(&settings.config().ui.language);
            if let Some(language) = language {
                settings.set_language(language);
                applied(language, settings.config());
            }
            // The window stays open so the message can be read.
            let _ = &window;
        }
    });
    // What the settings above actually mean for the reader's data; the
    // switches named in that report are these ones.
    let privacy_button = gtk::Button::with_label(text.privacy_open);
    privacy_button.connect_clicked({
        let window = window.clone();
        move |_| privacy(&window)
    });

    // Which version this is, for a bug report among other things.
    let about_button = gtk::Button::with_label(text.about_open);
    about_button.connect_clicked({
        let window = window.clone();
        move |_| crate::about::open(&window, text)
    });

    let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    buttons.set_halign(gtk::Align::End);
    buttons.append(&about_button);
    buttons.append(&privacy_button);
    buttons.append(&save);
    outer.append(&buttons);

    window.set_child(Some(&outer));
    window.present();
}

/// The catalogue's title has the file path in it; the window shows a plain
/// name and keeps the path in its subtitle.
fn strip(pattern: &str) -> String {
    fill(pattern, &[("path", "")])
        .trim()
        .trim_end_matches('—')
        .trim()
        .to_owned()
}
