//! The channel's point rewards: what there is, and adding or removing one.
//!
//! Adding is the point of this window rather than a convenience. Twitch
//! lets only the application that created a reward answer its
//! redemptions, so a reward made here is the only kind whose redemptions
//! can ever reach the queue in this client.

use std::cell::RefCell;
use std::rc::Rc;

use gtk::prelude::*;
use liveside_client::lang::{Strings, fill};
use liveside_proto::model::Reward;

/// Opens the list. `add` and `remove` go to the engine; the window is
/// redrawn from the answer rather than guessing what it did.
/// Changing a reward: its id, and what of it to change.
type ChangeReward = Rc<dyn Fn(&str, Change)>;

/// Adding one: name, cost, and what it asks.
type AddReward = Rc<dyn Fn(String, u64, String)>;

pub struct RewardWindow {
    window: gtk::Window,
    rows: gtk::ListBox,
    remove: Rc<dyn Fn(&str)>,
    /// Changing one: the id, and what of it to change.
    change: ChangeReward,
    strings: RefCell<&'static Strings>,
}

/// What an edit asks for. Only what is `Some` is sent, so pausing a
/// reward does not rewrite its name on the way.
#[derive(Default)]
pub struct Change {
    pub title: Option<String>,
    pub cost: Option<u64>,
    pub prompt: Option<String>,
    pub paused: Option<bool>,
}

impl RewardWindow {
    pub fn open(
        parent: &gtk::ApplicationWindow,
        strings: &'static Strings,
        add: AddReward,
        remove: Rc<dyn Fn(&str)>,
        change: ChangeReward,
    ) -> Rc<Self> {
        let rows = gtk::ListBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .build();
        let scroll = gtk::ScrolledWindow::builder()
            .child(&rows)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .build();

        // What to add: a name, what it costs, and what to ask for, which
        // most rewards leave empty.
        let title = gtk::Entry::builder()
            .placeholder_text(strings.reward_name)
            .hexpand(true)
            .build();
        let cost = crate::dialog::number(1.0, 1_000_000.0, 50.0);
        cost.set_value(100.0);
        let prompt = gtk::Entry::builder()
            .placeholder_text(strings.reward_prompt)
            .hexpand(true)
            .build();
        let button = gtk::Button::with_label(strings.reward_add);
        button.add_css_class("suggested-action");
        button.connect_clicked({
            let title = title.clone();
            let cost = cost.clone();
            let prompt = prompt.clone();
            move |_| {
                let name = title.text().trim().to_string();
                if name.is_empty() {
                    return;
                }
                add(name, cost.value() as u64, prompt.text().trim().to_string());
                title.set_text("");
                prompt.set_text("");
            }
        });

        let adding = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        adding.append(&title);
        adding.append(&cost);
        adding.append(&prompt);
        adding.append(&button);

        let page = gtk::Box::new(gtk::Orientation::Vertical, 8);
        page.append(&scroll);
        page.append(&adding);
        page.set_margin_start(12);
        page.set_margin_end(12);
        page.set_margin_top(12);
        page.set_margin_bottom(12);

        let window = gtk::Window::builder()
            .transient_for(parent)
            .modal(false)
            .title(strings.reward_window)
            .default_width(560)
            .default_height(420)
            .child(&page)
            .build();
        crate::dialog::close_on_escape(&window);
        window.present();

        Rc::new(Self {
            window,
            rows,
            remove,
            change,
            strings: RefCell::new(strings),
        })
    }

    pub fn is_open(&self) -> bool {
        self.window.is_visible()
    }

    /// Draws what the engine last said the channel offers.
    pub fn show(&self, rewards: &[Reward], strings: &'static Strings) {
        *self.strings.borrow_mut() = strings;
        while let Some(child) = self.rows.first_child() {
            self.rows.remove(&child);
        }
        if rewards.is_empty() {
            self.rows.append(
                &gtk::Label::builder()
                    .label(strings.reward_none)
                    .xalign(0.0)
                    .css_classes(["dim-label"])
                    .margin_start(8)
                    .margin_top(8)
                    .build(),
            );
        }
        for reward in rewards {
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
            row.set_margin_start(8);
            row.set_margin_end(8);
            row.set_margin_top(6);
            row.set_margin_bottom(6);
            row.append(
                &gtk::Label::builder()
                    .label(&reward.title)
                    .xalign(0.0)
                    .hexpand(true)
                    .wrap(true)
                    .build(),
            );
            // What is off or paused says so: a reward nobody can redeem
            // looks exactly like one nobody wants to.
            let state = match (reward.enabled, reward.paused) {
                (false, _) => format!(" {}", strings.reward_off),
                (_, true) => format!(" {}", strings.reward_paused),
                _ => String::new(),
            };
            row.append(
                &gtk::Label::builder()
                    .label(format!(
                        "{}{state}",
                        fill(strings.reward_cost, &[("cost", &reward.cost.to_string())])
                    ))
                    .css_classes(["dim-label"])
                    .build(),
            );
            // A reward made elsewhere can be listed and nothing more:
            // Twitch refuses both answering and deleting it.
            if reward.ours {
                // Pausing is the one change a stream actually interrupts
                // for, so it is a button rather than a field in a dialog.
                let pause = gtk::Button::with_label(match reward.paused {
                    true => strings.reward_resume,
                    false => strings.reward_pause,
                });
                let change = self.change.clone();
                let id = reward.id.clone();
                let now = reward.paused;
                pause.connect_clicked(move |_| {
                    change(
                        &id,
                        Change {
                            paused: Some(!now),
                            ..Change::default()
                        },
                    )
                });
                row.append(&pause);

                let edit = gtk::Button::with_label(strings.reward_edit);
                let change = self.change.clone();
                let editing = reward.clone();
                let window = self.window.clone();
                let strings_now = *self.strings.borrow();
                edit.connect_clicked(move |_| {
                    edit_reward(&window, &editing, strings_now, change.clone());
                });
                row.append(&edit);

                let button = gtk::Button::with_label(strings.reward_remove);
                let remove = self.remove.clone();
                let id = reward.id.clone();
                button.connect_clicked(move |_| remove(&id));
                row.append(&button);
            } else {
                row.append(
                    &gtk::Label::builder()
                        .label(strings.reward_theirs)
                        .css_classes(["dim-label"])
                        .build(),
                );
            }
            self.rows.append(&row);
        }
    }
}

/// The small form behind "Edit": the name, what it costs, and what it
/// asks. What is left alone is not sent.
fn edit_reward(
    parent: &gtk::Window,
    reward: &Reward,
    strings: &'static Strings,
    change: ChangeReward,
) {
    let rows = gtk::Box::new(gtk::Orientation::Vertical, 8);
    rows.set_margin_start(16);
    rows.set_margin_end(16);
    rows.set_margin_top(16);
    rows.set_margin_bottom(16);

    let title = gtk::Entry::builder().text(&reward.title).build();
    let cost = crate::dialog::number(1.0, 1_000_000.0, 50.0);
    cost.set_value(reward.cost as f64);
    let prompt = gtk::Entry::builder()
        .text(&reward.prompt)
        .placeholder_text(strings.reward_prompt)
        .build();
    rows.append(&title);
    rows.append(&cost);
    rows.append(&prompt);

    let window = gtk::Window::builder()
        .transient_for(parent)
        .modal(true)
        .title(reward.title.clone())
        .default_width(380)
        .build();
    crate::dialog::close_on_escape(&window);
    let save = gtk::Button::with_label(strings.reward_save);
    save.add_css_class("suggested-action");
    save.set_halign(gtk::Align::End);
    save.connect_clicked({
        let id = reward.id.clone();
        let was = reward.clone();
        let title = title.clone();
        let cost = cost.clone();
        let prompt = prompt.clone();
        let window = window.clone();
        move |_| {
            let now_title = title.text().trim().to_string();
            let now_prompt = prompt.text().trim().to_string();
            let now_cost = cost.value() as u64;
            change(
                &id,
                Change {
                    title: (now_title != was.title && !now_title.is_empty()).then_some(now_title),
                    cost: (now_cost != was.cost).then_some(now_cost),
                    prompt: (now_prompt != was.prompt).then_some(now_prompt),
                    paused: None,
                },
            );
            window.close();
        }
    });
    rows.append(&save);
    window.set_child(Some(&rows));
    window.present();
}
