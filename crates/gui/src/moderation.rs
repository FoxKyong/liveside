//! Moderation: what to do with one message, and the restrictions on a room.
//!
//! A menu item is a deliberate click, so deleting and timing out happen at
//! once. A ban asks first: it is the one action that is awkward to undo in
//! front of an audience.

use std::rc::Rc;

use gtk::gio;
use gtk::prelude::*;
use liveside_client::lang::{Strings, fill};
use liveside_proto::{Command, Login, MessageId, UserId};

/// What the window must be able to do for these menus to work.
pub trait Moderate {
    fn text(&self) -> &'static Strings;
    fn send(&self, command: Command);
    /// The channel a menu acts in, with the name to show.
    fn channel(&self) -> Option<(UserId, Login)>;
    /// Starts a reply to a message in the input line.
    fn start_reply(&self, message: MessageId, user: Login);
    /// The message the pointer was last over, for the menu items.
    fn clicked(&self) -> Option<(MessageId, Login)>;
    /// Puts what is selected on the clipboard.
    fn copy_selection(&self);
    /// Opens a link from chat, asking first if the settings say so.
    fn open_link(self: Rc<Self>, text: &str, url: &str);
    /// Whether links can be opened at all.
    fn links_open(&self) -> bool;
    /// Puts this text on the clipboard.
    fn copy_text(&self, text: &str);
    /// Whether links can be checked with VirusTotal.
    fn virustotal(&self) -> bool;
    /// Whether VirusTotal has said it does not know this link.
    fn link_unknown(&self, url: &str) -> bool;
    /// Asks VirusTotal about a link; `scan` sends it to be scanned.
    fn check_link(&self, url: &str, scan: bool);
    /// Whether the chat of the message under the pointer can be told
    /// about its link: there is such a message, and an account to write
    /// with.
    fn can_tell(&self) -> bool;
    /// Asks VirusTotal about a link, written as `shown`, if it has not
    /// answered already, and tells the chat what it says.
    fn check_and_tell(self: Rc<Self>, shown: &str, url: &str);
    /// Whether links may be followed to say where they lead.
    fn follows_links(&self) -> bool;
    /// Follows a link's redirects from this machine.
    fn follow_link(&self, url: &str);
    /// Puts the whole message under the pointer on the clipboard, so that
    /// nobody has to catch a moving line with the mouse.
    fn copy_message(&self);
    /// Hides everything this person writes, everywhere, from now on.
    fn ignore_user(self: Rc<Self>, user: Login);
}

/// What the menu should offer, given where the pointer is and what is
/// selected.
pub struct MenuFor<'a> {
    /// Something is selected, so there is something to copy.
    pub has_selection: bool,
    /// The pointer is over this message.
    pub message: Option<&'a Login>,
    /// The selection, if any, lies inside that one message, so acting on
    /// it does what the user sees.
    pub one_message: bool,
    /// The pointer is over a link: the text as written and its address.
    pub link: Option<(String, String)>,
}

/// The menu for a chat view, built for the moment it is opened.
///
/// It is made of widgets rather than from a menu model: a model's items are
/// driven by actions, and an action the popover cannot find leaves an item
/// that quietly does nothing, which is what happened here. Every item now
/// carries what it does.
///
/// The view's own menu is not used at all: in text that cannot be edited,
/// Cut, Paste, Delete, Undo and Redo can never do anything. Items appear
/// only when they would act on something, which keeps the menu short in a
/// chat that is moving.
///
/// Returns `None` when there would be nothing in it.
pub fn message_menu<W: Moderate + 'static>(
    window: &gtk::ApplicationWindow,
    ui: Rc<W>,
    what: MenuFor<'_>,
) -> Option<gtk::Popover> {
    let text = ui.text();
    let popover = gtk::Popover::new();
    popover.set_has_arrow(false);
    let items = gtk::Box::new(gtk::Orientation::Vertical, 0);

    let item = |label: &str, run: Box<dyn Fn()>| {
        let button = gtk::Button::builder().label(label).has_frame(false).build();
        if let Some(child) = button.child() {
            child.set_halign(gtk::Align::Start);
        }
        let popover = popover.clone();
        button.connect_clicked(move |_| {
            popover.popdown();
            run();
        });
        items.append(&button);
    };

    let mut anything = false;
    // The link first: it is the smallest thing under the pointer, and
    // what a right-click on it is most likely about.
    if let Some((shown, url)) = what.link.clone() {
        if ui.links_open() {
            let ui = ui.clone();
            let url = url.clone();
            let shown = shown.clone();
            item(
                text.link_menu_open,
                Box::new(move || ui.clone().open_link(&shown, &url)),
            );
        }
        if ui.follows_links() {
            let ui = ui.clone();
            let url = url.clone();
            item(
                text.link_menu_follow,
                Box::new(move || ui.follow_link(&url)),
            );
        }
        if ui.virustotal() {
            // Submitting is offered only once a lookup has found nothing:
            // it makes the link visible to VirusTotal's customers, so it
            // is a second, deliberate step.
            let scan = ui.link_unknown(&url);
            let ui = ui.clone();
            let url = url.clone();
            item(
                if scan {
                    text.link_menu_scan
                } else {
                    text.link_menu_check
                },
                Box::new(move || ui.check_link(&url, scan)),
            );
        }
        // For a moderator who wants the chat warned at once: the answer
        // goes to the chat, naming the link and whose it was.
        if ui.virustotal() && what.message.is_some() && ui.can_tell() {
            let ui = ui.clone();
            let url = url.clone();
            let shown = shown.clone();
            item(
                text.link_menu_check_tell,
                Box::new(move || ui.clone().check_and_tell(&shown, &url)),
            );
        }
        let ui = ui.clone();
        item(text.link_menu_copy, Box::new(move || ui.copy_text(&url)));
        if what.has_selection || what.message.is_some() {
            items.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
        }
        anything = true;
    }
    if what.has_selection {
        let ui = ui.clone();
        item(text.action_copy, Box::new(move || ui.copy_selection()));
        anything = true;
    }
    if what.message.is_some() {
        let ui = ui.clone();
        item(
            text.action_copy_message,
            Box::new(move || ui.copy_message()),
        );
        anything = true;
    }

    if let Some(user) = what.message.filter(|_| what.one_message) {
        if anything {
            items.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
        }
        // The heading names the author: the menu acts on one message, and
        // a pointer is a poor way of saying which.
        items.append(
            &gtk::Label::builder()
                .label(fill(text.menu_for_message, &[("user", user.as_str())]))
                .xalign(0.0)
                .css_classes(["dim-label"])
                .margin_start(12)
                .margin_top(6)
                .margin_bottom(3)
                .build(),
        );

        let reply = ui.clone();
        item(
            text.action_reply,
            Box::new(move || {
                if let Some((message, user)) = reply.clicked() {
                    reply.start_reply(message, user);
                }
            }),
        );
        let delete = ui.clone();
        item(
            text.action_delete,
            Box::new(move || {
                if let (Some((message, _)), Some((channel, _))) =
                    (delete.clicked(), delete.channel())
                {
                    delete.send(Command::Input {
                        channel,
                        text: format!("/delete {message}"),
                        reply_to: None,
                    });
                }
            }),
        );
        // Not a moderator's action, but one about this person: it hides
        // them on this screen only, and the list of the hidden keeps what
        // they say.
        let ignore = ui.clone();
        let who = user.clone();
        item(
            &fill(text.action_ignore, &[("user", who.as_str())]),
            Box::new(move || ignore.clone().ignore_user(who.clone())),
        );
        let card = ui.clone();
        item(
            text.action_card,
            Box::new(move || {
                if let (Some((_, user)), Some((channel, _))) = (card.clicked(), card.channel()) {
                    card.send(Command::ShowUser { channel, user });
                }
            }),
        );
        let pin = ui.clone();
        item(
            text.action_pin,
            Box::new(move || {
                if let (Some((message, _)), Some((channel, _))) = (pin.clicked(), pin.channel()) {
                    pin.send(Command::Input {
                        channel,
                        text: format!("/pin {message}"),
                        reply_to: None,
                    });
                }
            }),
        );
        let timeout = ui.clone();
        item(
            text.action_timeout,
            Box::new(move || {
                if let (Some((_, user)), Some((channel, _))) =
                    (timeout.clicked(), timeout.channel())
                {
                    timeout.send(Command::Input {
                        channel,
                        text: format!("/timeout {user} 10m"),
                        reply_to: None,
                    });
                }
            }),
        );
        let ban = ui.clone();
        let window = window.clone();
        item(
            text.action_ban,
            Box::new(move || {
                if let Some((_, user)) = ban.clicked() {
                    confirm_ban(&window, ban.clone(), user);
                }
            }),
        );
        anything = true;
    }

    if !anything {
        return None;
    }
    popover.set_child(Some(&items));
    Some(popover)
}

/// Asks before banning, and says who from where.
fn confirm_ban<W: Moderate + 'static>(window: &gtk::ApplicationWindow, ui: Rc<W>, user: Login) {
    let text = ui.text();
    let Some((channel, login)) = ui.channel() else {
        return;
    };
    let dialog = gtk::AlertDialog::builder()
        .message(fill(
            text.confirm_ban,
            &[("user", user.as_str()), ("channel", &format!("#{login}"))],
        ))
        .buttons([text.cancel, text.confirm])
        .cancel_button(0)
        .default_button(0)
        .build();
    dialog.choose(Some(window), gtk::gio::Cancellable::NONE, move |answer| {
        if answer == Ok(1) {
            ui.send(Command::Input {
                channel,
                text: format!("/ban {user}"),
                reply_to: None,
            });
        }
    });
}

/// The menu of room restrictions, for the channel on screen.
pub fn room_menu(text: &'static Strings, on_now: bool) -> gio::Menu {
    let menu = gio::Menu::new();
    let modes = gio::Menu::new();
    for (label, action) in [
        (text.mode_emote_only, "room.emoteonly"),
        (text.mode_subscribers, "room.subscribers"),
        (text.mode_followers, "room.followers"),
        (text.mode_slow, "room.slow"),
        (text.mode_unique, "room.unique"),
    ] {
        modes.append(Some(label), Some(action));
    }
    menu.append_section(None, &modes);
    let rest = gio::Menu::new();
    // The broadcaster's own, not a moderator's, but this is the menu for
    // "things done to the room" and a poll is one of them.
    rest.append(Some(text.poll_open), Some("room.poll"));
    rest.append(Some(text.pred_open), Some("room.prediction"));
    rest.append(Some(text.reward_open), Some("room.rewards"));
    rest.append(Some(text.clip_make), Some("room.clip"));
    rest.append(Some(text.channel_open), Some("room.channel"));
    rest.append(Some(text.schedule_open), Some("room.schedule"));
    rest.append(Some(text.blocked_open), Some("room.blockedterms"));
    rest.append(Some(text.followers_open), Some("room.followerlist"));
    rest.append(Some(text.subscribers_open), Some("room.sublist"));
    rest.append(Some(text.action_unpin), Some("room.unpin"));
    rest.append(Some(text.clear_chat), Some("room.clear"));
    menu.append_section(None, &rest);
    // One item rather than two: the engine asks Twitch whether Shield
    // Mode is on when joining and keeps up with the events, so the menu
    // can say which way it would go instead of offering both.
    let shield = gio::Menu::new();
    shield.append(
        Some(match on_now {
            true => text.shield_off,
            false => text.shield_on,
        }),
        Some("room.shield"),
    );
    menu.append_section(None, &shield);
    menu
}

/// The commands behind the room menu. Each entry is a toggle: the engine
/// reports the new state, and the window shows it.
pub fn room_actions<W: Moderate + 'static>(
    ui: Rc<W>,
    modes: Rc<dyn Fn() -> liveside_proto::model::RoomModes>,
    shield: Rc<dyn Fn() -> bool>,
) -> gio::SimpleActionGroup {
    let actions = gio::SimpleActionGroup::new();
    let add = |name: &'static str, run: Box<dyn Fn()>| {
        let action = gio::SimpleAction::new(name, None);
        action.connect_activate(move |_, _| run());
        actions.add_action(&action);
    };
    let toggle = |on: bool, yes: &str, no: &str| -> String {
        if on { no.to_owned() } else { yes.to_owned() }
    };

    add("emoteonly", {
        let (ui, modes) = (ui.clone(), modes.clone());
        Box::new(move || {
            let text = toggle(modes().emote_only, "/emoteonly", "/emoteonlyoff");
            run(&ui, text);
        })
    });
    add("subscribers", {
        let (ui, modes) = (ui.clone(), modes.clone());
        Box::new(move || {
            let text = toggle(modes().subscribers_only, "/subscribers", "/subscribersoff");
            run(&ui, text);
        })
    });
    add("followers", {
        let (ui, modes) = (ui.clone(), modes.clone());
        Box::new(move || {
            let text = toggle(
                modes().followers_only.is_some(),
                "/followers",
                "/followersoff",
            );
            run(&ui, text);
        })
    });
    add("slow", {
        let (ui, modes) = (ui.clone(), modes.clone());
        Box::new(move || {
            let text = toggle(modes().slow.is_some(), "/slow 30", "/slowoff");
            run(&ui, text);
        })
    });
    add("unique", {
        let (ui, modes) = (ui.clone(), modes.clone());
        Box::new(move || {
            let text = toggle(modes().unique_chat, "/uniquechat", "/uniquechatoff");
            run(&ui, text);
        })
    });
    add("blockedterms", {
        let ui = ui.clone();
        Box::new(move || run(&ui, "/blockedterms".to_owned()))
    });
    add("followerlist", {
        let ui = ui.clone();
        Box::new(move || run(&ui, "/followerlist".to_owned()))
    });
    add("sublist", {
        let ui = ui.clone();
        Box::new(move || run(&ui, "/sublist".to_owned()))
    });
    add("unpin", {
        let ui = ui.clone();
        Box::new(move || run(&ui, "/unpin".to_owned()))
    });
    add("shield", {
        let (ui, shield) = (ui.clone(), shield.clone());
        Box::new(move || {
            let text = match shield() {
                true => "/shieldoff",
                false => "/shield",
            };
            run(&ui, text.to_owned());
        })
    });
    add("clear", {
        let ui = ui.clone();
        Box::new(move || run(&ui, "/clear".to_owned()))
    });
    actions
}

fn run<W: Moderate>(ui: &Rc<W>, text: String) {
    if let Some((channel, _)) = ui.channel() {
        ui.send(Command::Input {
            channel,
            text,
            reply_to: None,
        });
    }
}
