//! What is known about one person, in a window of its own.
//!
//! A moderator opens this before deciding anything: how old the account
//! is, since when they follow, whether they are already banned here, and
//! what they have actually been saying. The facts come from the engine;
//! the messages come from the chat already on screen, which is the one
//! thing Twitch cannot answer.

use std::rc::Rc;

use gtk::prelude::*;
use liveside_client::lang::{Strings, fill};
use liveside_proto::event::UserCard;

/// Twitch writes its times as `2019-04-01T12:00:00Z`; the day is what a
/// person reads, and the rest is noise in a card.
fn day(stamp: &str) -> String {
    stamp.split('T').next().unwrap_or(stamp).to_owned()
}

/// Opens the card. `act` runs a moderation command for that person.
pub fn open(
    parent: &gtk::ApplicationWindow,
    card: &UserCard,
    messages: &[String],
    strings: &'static Strings,
    act: Rc<dyn Fn(&str)>,
) {
    let window = gtk::Window::builder()
        .transient_for(parent)
        .modal(false)
        .title(card.user.display_name.clone())
        .default_width(420)
        .default_height(420)
        .build();
    crate::dialog::close_on_escape(&window);

    let facts = gtk::Box::new(gtk::Orientation::Vertical, 4);
    let line = |text: String, dim: bool| {
        gtk::Label::builder()
            .label(text)
            .xalign(0.0)
            .wrap(true)
            .css_classes(if dim { vec!["dim-label"] } else { Vec::new() })
            .build()
    };
    facts.append(&line(
        fill(
            strings.card_user,
            &[
                ("user", &card.user.display_name),
                ("login", card.user.login.as_str()),
            ],
        ),
        false,
    ));
    if !card.created_at.is_empty() {
        facts.append(&line(
            fill(strings.card_created, &[("date", &day(&card.created_at))]),
            true,
        ));
    }
    facts.append(&line(
        match (&card.follows_since, card.follows_known) {
            (Some(date), _) => fill(strings.card_follows, &[("date", &day(date))]),
            (None, true) => strings.card_not_following.to_owned(),
            // Twitch refused the question: that is not a no.
            (None, false) => strings.card_unknown.to_owned(),
        },
        true,
    ));
    facts.append(&line(
        match (card.sub_tier.is_empty(), card.sub_known) {
            (true, false) => strings.card_unknown.to_owned(),
            (true, true) => strings.card_not_subscribed.to_owned(),
            (false, _) => fill(
                match card.sub_gift {
                    true => strings.card_sub_gift,
                    false => strings.card_subscribed,
                },
                &[("tier", &card.sub_tier)],
            ),
        },
        true,
    ));
    if card.banned {
        facts.append(&line(
            match &card.banned_until {
                Some(date) => fill(strings.card_timed_out, &[("date", &day(date))]),
                None => strings.card_banned.to_owned(),
            },
            false,
        ));
    }

    // What they have said, which is what the decision is really about.
    let said = gtk::TextView::builder()
        .editable(false)
        .cursor_visible(false)
        .wrap_mode(gtk::WrapMode::WordChar)
        .left_margin(4)
        .right_margin(4)
        .build();
    said.buffer().set_text(&messages.join("\n"));
    crate::chat::swallow_context_menu(&said);
    let said_scroll = gtk::ScrolledWindow::builder()
        .child(&said)
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vexpand(true)
        .build();

    // The commands are the ones already in the message menu, aimed at a
    // person rather than at one of their messages.
    let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    for (label, command) in [
        (strings.action_timeout, "/timeout {user} 10m"),
        (strings.action_ban, "/ban {user} "),
    ] {
        let button = gtk::Button::with_label(label);
        let act = act.clone();
        let command = command.replace("{user}", card.user.login.as_str());
        let window = window.clone();
        button.connect_clicked(move |_| {
            act(&command);
            window.close();
        });
        buttons.append(&button);
    }
    buttons.set_halign(gtk::Align::End);

    let page = gtk::Box::new(gtk::Orientation::Vertical, 8);
    page.append(&facts);
    page.append(
        &gtk::Label::builder()
            .label(strings.card_messages)
            .xalign(0.0)
            .css_classes(["heading"])
            .build(),
    );
    page.append(&said_scroll);
    page.append(&buttons);
    page.set_margin_start(12);
    page.set_margin_end(12);
    page.set_margin_top(12);
    page.set_margin_bottom(12);
    window.set_child(Some(&page));
    window.present();
}
