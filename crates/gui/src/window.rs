//! The window: channels on the left, chat in the middle, a line to type in.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::rc::Rc;

use gtk::glib;
use gtk::glib::object::ObjectExt;
use gtk::prelude::*;
use liveside_client::Config;
use liveside_client::config::LinkMode;
use liveside_client::input::{Completion, History, Step};
use liveside_client::lang::{Language, Strings, fill};
use liveside_proto::model::{Line, LineContent, LineFlags, NoticeLevel};
use liveside_proto::{Command, Event, Login, MessageId, ServerMessage, Timestamp, UserId};

use crate::chat::ChatView;
use crate::emotes::Emotes;
use crate::engine::{Engine, FromEngine};

/// Everything the window keeps while it runs.
/// The stack's page for what belongs to no channel.
const STATUS_PAGE: &str = "status";
/// The page shown for a channel that is in a window of its own.
const POPPED_PAGE: &str = "popped";

struct Ui {
    engine: Engine,
    /// The settings, shared with the window that edits them.
    settings: Rc<RefCell<liveside_client::settings::Settings>>,
    /// The language in use; a change from the settings window lands here.
    language: Cell<Language>,
    /// The colours in use, which the settings window can turn round.
    palette: Cell<&'static crate::theme::Palette>,
    emotes: Rc<Emotes>,
    text: &'static Strings,
    channels: gtk::ListBox,
    stack: gtk::Stack,
    entry: gtk::Entry,
    status: gtk::Label,
    /// The wheel beside the status, turning while an answer is owed.
    working: gtk::Spinner,
    /// Which backend the engine runs, so a connection notice does not
    /// claim Twitch when the demo is talking.
    backend: RefCell<String>,
    /// The name of the status page in the channel list, marked while
    /// something has landed there unseen.
    status_label: gtk::Label,
    views: RefCell<HashMap<UserId, ChatView>>,
    /// Channel shown right now, for sending and for the placeholder text.
    active: RefCell<Option<(UserId, Login)>>,
    /// What AutoMod is holding, oldest first: the channel it was caught
    /// in, the message, and who wrote it — so a decision can name them,
    /// and so /allow and /deny can find the oldest without an id.
    held: RefCell<Vec<(UserId, MessageId, String)>>,
    /// People asking to be let back in, oldest first, the same way.
    unban_requests: RefCell<Vec<(UserId, String, String)>>,
    /// Each channel's row label, and whether it is streaming, so the
    /// list can say so beside the name.
    channel_labels: RefCell<HashMap<UserId, (gtk::Label, Login)>>,
    live: RefCell<HashMap<UserId, bool>>,
    /// How many watch each streaming channel and since when, and where
    /// the one on screen is said.
    stats: RefCell<HashMap<UserId, liveside_proto::model::StreamStats>>,
    stream_label: gtk::Label,
    /// How many watched each channel's stream over time, and the graph
    /// of one while it is open.
    viewers: RefCell<HashMap<UserId, liveside_proto::model::ViewerHistory>>,
    graph_window: RefCell<Option<Rc<crate::graph::GraphWindow>>>,
    /// The account's login, for custom commands that name it.
    me: RefCell<Option<String>>,
    /// Starts Streamlink for the channel on screen; there while one is.
    watch_button: gtk::Button,
    /// The channels taken out of the stack: into a window of their own,
    /// or docked beside the chat on screen.
    popouts: RefCell<HashMap<UserId, Detached>>,
    /// The main chat and the channels docked beside it, sharing the width.
    chats: gtk::Box,
    /// The docked channels, left to right as on screen.
    docked: RefCell<Vec<(UserId, Login)>>,
    /// Which channels were docked and how wide, kept across restarts.
    layout: RefCell<liveside_client::layout::Layout>,
    layout_path: PathBuf,
    /// Stream Chat windows, by channel.
    stream_chats: RefCell<HashMap<UserId, Rc<crate::stream_chat::StreamChat>>>,
    /// What the main window says in place of a popped-out channel, and
    /// the button that brings its window up (a docked one has none).
    popped_label: gtk::Label,
    popped_show: gtk::Button,
    /// What is waiting to be decided, and the moderation log.
    review: RefCell<crate::review::Review>,
    /// The people active in the channel on screen.
    users: RefCell<crate::users::UserList>,
    /// Room restrictions per channel, for the moderation menu.
    modes: RefCell<HashMap<UserId, liveside_proto::model::RoomModes>>,
    /// And whether Shield Mode is on in each, so its menu item can say
    /// which way it would go.
    shield: RefCell<HashMap<UserId, bool>>,
    /// The button that menu hangs on, for rebuilding it when the answer
    /// changes.
    moderate_menu: RefCell<Option<gtk::MenuButton>>,
    /// Channels asked for and not joined yet: a row each, so the window
    /// says what it is doing instead of standing empty while four
    /// channels are looked up, subscribed to and asked about one after
    /// another.
    joining: RefCell<HashMap<Login, (gtk::ListBoxRow, gtk::Label)>>,
    /// A channel joined from a notification, to be opened once it is in.
    open_when_joined: RefCell<Option<Login>>,
    /// Channels the window still shows but the engine may not have: the
    /// daemon went away, and they are greyed until it says otherwise.
    stale: RefCell<HashSet<UserId>>,
    /// Channels joined again after that. Their log is already on
    /// screen, so the history a join brings back is not drawn twice.
    rejoined: RefCell<HashSet<UserId>>,
    /// What the engine says it does with the reader's data, for the
    /// report the Settings window opens.
    data_flows: RefCell<Vec<liveside_proto::DataFlow>>,
    /// The poll running in each channel, and the bar that shows the one
    /// in the channel on screen.
    polls: RefCell<HashMap<UserId, liveside_proto::model::Poll>>,
    poll_bar: Rc<crate::poll::PollBar>,
    /// And the prediction, which stays on screen after betting closes:
    /// that is when it needs a decision.
    predictions: RefCell<HashMap<UserId, liveside_proto::model::Prediction>>,
    prediction_bar: Rc<crate::prediction::PredictionBar>,
    /// The window listing the channel's rewards, while one is open: the
    /// answer to asking for them is drawn there rather than said.
    reward_window: RefCell<Option<Rc<crate::rewards::RewardWindow>>>,
    /// The same for the form that edits what the channel says about
    /// itself.
    channel_window: RefCell<Option<Rc<crate::channel::ChannelWindow>>>,
    /// And for the schedule.
    schedule_window: RefCell<Option<Rc<crate::schedule::ScheduleWindow>>>,
    /// Whisper threads: a page each, keyed by the other person's login,
    /// and which one is on screen. A whisper is a channel without the
    /// moderation, so it reuses the chat view and nothing else.
    whispers: RefCell<HashMap<Login, ChatView>>,
    active_whisper: RefCell<Option<Login>>,
    /// Logging in, or out again once there is an account.
    login_button: gtk::Button,
    logged_in: Cell<bool>,
    /// The page for everything that belongs to no channel: logging in,
    /// what the engine reports at startup, why a command was refused.
    /// The terminal frontend has always had this buffer; without it a
    /// window has nowhere to put a sentence before a channel is joined.
    status_view: RefCell<ChatView>,
    /// The window itself, for dialogs and menus.
    window: glib::WeakRef<gtk::ApplicationWindow>,
    /// The message the pointer was last over, for the menu.
    clicked: RefCell<Option<(MessageId, Login)>>,
    /// The message being answered, shown above the input line.
    replying_to: RefCell<Option<(MessageId, Login)>>,
    reply_label: gtk::Label,
    scrollback: usize,
    /// Lines sent from this window, for Up and Down in the input line.
    history: RefCell<History>,
    /// A Tab completion in progress, ended by any other key.
    completion: RefCell<Completion>,
    /// What is known about links, shared with every chat view.
    link_reports: Rc<RefCell<crate::chat::LinkReports>>,
    /// Searching the chat on screen.
    search: crate::search::SearchBar,
    /// The table of a channel's blocked terms, while one is open: the
    /// list coming again redraws it rather than opening another.
    blocked_window: RefCell<Option<(UserId, Rc<crate::blocked::BlockedWindow>)>>,
    /// The followed channels, while that window is open: Refresh and the
    /// button redraw it rather than opening another.
    followed_window: RefCell<Option<Rc<crate::followed::FollowedWindow>>>,
    /// The user's own bots as the engine last reported them, and their
    /// window while it is open.
    bots: RefCell<Vec<liveside_proto::model::BotStatus>>,
    bots_window: RefCell<Option<Rc<crate::bots::BotsWindow>>>,
    /// What lit up and what was hidden, in every channel, and the window
    /// that lists it while it is open.
    collected: RefCell<liveside_client::collected::Collected>,
    collected_window: RefCell<Option<Rc<crate::collected::CollectedWindow>>>,
    /// Desktop notifications and sounds.
    notifications: crate::notify::Notifications,
}

impl crate::moderation::Moderate for Ui {
    fn text(&self) -> &'static Strings {
        self.language.get().strings()
    }

    fn send(&self, command: Command) {
        self.engine.send(command);
    }

    fn channel(&self) -> Option<(UserId, Login)> {
        self.active.borrow().clone()
    }

    fn clicked(&self) -> Option<(MessageId, Login)> {
        self.clicked.borrow().clone()
    }

    fn copy_selection(&self) {
        let Some(view) = self.visible_view() else {
            return;
        };
        let buffer = view.buffer();
        let Some((start, end)) = buffer.selection_bounds() else {
            return;
        };
        // Taken through the chat view, so an emote comes out as its code
        // rather than as the placeholder character that stands for a
        // widget.
        self.to_clipboard(&crate::chat::text_between(&start, &end));
    }

    fn open_link(self: Rc<Self>, text: &str, url: &str) {
        follow_link(&self, text, url);
    }

    fn links_open(&self) -> bool {
        self.link_mode() != LinkMode::Off
    }

    fn virustotal(&self) -> bool {
        self.settings.borrow().config().security.virustotal
    }

    fn link_unknown(&self, url: &str) -> bool {
        matches!(
            self.link_reports.borrow().get(&(
                url.to_owned(),
                liveside_proto::model::LinkSource::VirusTotal
            )),
            Some(liveside_proto::model::LinkState::Unknown)
        )
    }

    fn follows_links(&self) -> bool {
        self.settings.borrow().config().security.resolver != liveside_client::config::Resolver::Off
    }

    fn follow_link(&self, url: &str) {
        self.engine.send(Command::FollowLink {
            url: url.to_owned(),
        });
    }

    fn ignore_user(self: Rc<Self>, user: Login) {
        let active = self.active.borrow().as_ref().map(|(id, _)| id.clone());
        ignore_command(&self, active.as_ref(), true, user.as_str());
    }

    fn check_link(&self, url: &str, scan: bool) {
        self.engine.send(Command::CheckLink {
            url: url.to_owned(),
            scan,
        });
    }

    fn copy_text(&self, text: &str) {
        self.to_clipboard(text);
    }

    fn copy_message(&self) {
        let Some((id, _)) = self.clicked() else {
            return;
        };
        let text = self.active.borrow().as_ref().and_then(|(channel, _)| {
            self.views
                .borrow()
                .get(channel)
                .and_then(|chat| chat.message_text(&id))
        });
        if let Some(text) = text {
            self.to_clipboard(&text);
        }
    }

    fn start_reply(&self, message: MessageId, user: Login) {
        self.reply_label
            .set_text(&fill(self.text().replying_to, &[("user", user.as_str())]));
        self.reply_label.set_visible(true);
        *self.replying_to.borrow_mut() = Some((message, user));
        self.entry.grab_focus();
        self.entry.set_position(-1);
    }
}

impl Ui {
    /// What clicking a link does, as the settings say now.
    fn link_mode(&self) -> LinkMode {
        self.settings.borrow().config().security.links
    }

    /// Drops a reply being written, so the next line is said plainly.
    /// Returns whether there was one.
    fn stop_reply(&self) -> bool {
        self.reply_label.set_visible(false);
        self.replying_to.borrow_mut().take().is_some()
    }
}

pub fn build(
    application: &gtk::Application,
    config: Config,
    config_path: PathBuf,
    socket: Option<PathBuf>,
    demo: bool,
    language: Language,
) {
    let settings_config = config.clone();
    // Before any widget is built, so the window opens the right way round.
    let palette = crate::theme::apply(config.ui.theme);
    let text = language.strings();
    set_direction(text);
    let scrollback = config.chat.scrollback;
    let animated = config.ui.animated_emotes;
    let mut config = config;
    if demo {
        // Invented chat has no business in the local log, as in the other
        // frontend.
        config.chat.history = false;
    }
    let engine = Engine::start(config, socket, demo);
    let layout_path = liveside_client::layout::default_path(demo);

    let window = gtk::ApplicationWindow::builder()
        .application(application)
        .title(text.window_title)
        .default_width(1000)
        .default_height(700)
        .build();

    // Channels on the left.
    let channels = gtk::ListBox::builder()
        .selection_mode(gtk::SelectionMode::Single)
        .build();
    let channel_scroll = gtk::ScrolledWindow::builder()
        .child(&channels)
        .hscrollbar_policy(gtk::PolicyType::Never)
        .width_request(180)
        // The list takes the height; the buttons below keep only what they need.
        .vexpand(true)
        .build();
    let side = gtk::Box::new(gtk::Orientation::Vertical, 6);
    let heading = gtk::Label::builder()
        .label(text.channels_heading)
        .xalign(0.0)
        .css_classes(["heading"])
        .build();
    side.append(&heading);
    side.append(&channel_scroll);
    // A button's words, cut short rather than widening what holds them:
    // in a language with longer words (Spanish) the header used to run
    // past the window's right edge and take the last button with it.
    let words = |label: &str| {
        gtk::Label::builder()
            .label(label)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .build()
    };
    // The name is asked for in a window of its own (`join.rs`): a field
    // here had room for neither a name nor the button's word.
    let join_button = gtk::Button::builder()
        .child(&words(text.join_open))
        .tooltip_text(text.join_open)
        .build();
    // What there is to join besides what one remembers: the channels
    // this account moderates, and the ones it follows.
    // Their words are cut short rather than widening the panel, which
    // otherwise pushes the channel list out of the window's left edge.
    let side_button = |label: &str| {
        let words = gtk::Label::builder()
            .label(label)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .build();
        let button = gtk::Button::builder()
            .child(&words)
            .css_classes(["flat"])
            .tooltip_text(label)
            .build();
        side.append(&button);
        button
    };
    let moderated_button = side_button(text.moderated_open);
    let followed_button = side_button(text.followed_open);
    let collected_button = side_button(text.collected_open);
    side.append(&join_button);
    side.set_margin_start(6);
    side.set_margin_end(6);
    side.set_margin_top(6);
    side.set_margin_bottom(6);

    // Chat in the middle: one page per channel.
    let stack = gtk::Stack::new();
    // Each page asks for its own width: the buttons of a channel popped
    // out to its own window, never on screen while chatting, kept the
    // chat from being narrower than they are.
    stack.set_hhomogeneous(false);
    stack.set_hexpand(true);
    stack.set_vexpand(true);

    let entry = gtk::Entry::builder()
        .placeholder_text(fill(text.send_placeholder, &[("channel", "…")]))
        .build();
    let status = gtk::Label::builder()
        .xalign(0.0)
        .css_classes(["dim-label"])
        // A long line must never push the buttons off the window.
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .max_width_chars(40)
        .build();
    let login_button = gtk::Button::builder()
        .child(&words(text.login_button))
        .tooltip_text(text.login_button)
        .build();
    let settings_button = gtk::Button::builder()
        .child(&words(text.settings_button))
        .tooltip_text(text.settings_button)
        .build();
    let bots_button = gtk::Button::builder()
        .child(&words(text.bots_button))
        .tooltip_text(text.bots_button)
        .build();
    let moderate_button = gtk::MenuButton::builder()
        .child(&words(text.moderate_button))
        .tooltip_text(text.moderate_button)
        .always_show_arrow(true)
        .build();
    let review_button = gtk::ToggleButton::builder()
        .child(&words(text.review_button))
        .tooltip_text(text.review_button)
        .active(false)
        .build();
    let users_button = gtk::ToggleButton::builder()
        .child(&words(text.users_button))
        .tooltip_text(text.users_button)
        .active(true)
        .build();
    // Turns while the engine owes an answer: opening a user card or a
    // list of people is a question to Twitch, and the window used to
    // look asleep until it came back.
    let working = gtk::Spinner::builder()
        .width_request(16)
        .height_request(16)
        .tooltip_text(text.working)
        .visible(false)
        .build();
    // How the stream on screen stands: viewers and how long it has run.
    // A click shows how many watched over time.
    let stream_label = gtk::Label::builder()
        .css_classes(["dim-label"])
        // Cut short before the buttons are, when the header is narrow:
        // its longer text gives way first.
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .visible(false)
        .tooltip_text(text.graph_open)
        .cursor(&gtk::gdk::Cursor::from_name("pointer", None).expect("a pointer cursor"))
        .build();
    // Watching the channel on screen, here, with Streamlink.
    let watch_button = gtk::Button::builder()
        .icon_name("media-playback-start-symbolic")
        .tooltip_text(text.streamlink_watch)
        .visible(false)
        .build();
    let status_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    status.set_hexpand(true);
    status_row.append(&status);
    status_row.append(&stream_label);
    status_row.append(&watch_button);
    status_row.append(&working);
    status_row.append(&review_button);
    status_row.append(&users_button);
    status_row.append(&moderate_button);
    status_row.append(&bots_button);
    status_row.append(&settings_button);
    status_row.append(&login_button);

    // Shown only while a poll is running in the channel on screen. The
    // buttons need the engine, which does not exist yet, so they go
    // through the same late-bound handle the review panel uses.
    let poll_end: Rc<RefCell<Option<Rc<Ui>>>> = Rc::new(RefCell::new(None));
    let poll_bar = crate::poll::PollBar::new(text, {
        let poll_end = poll_end.clone();
        Rc::new(move |archive: bool| {
            let Some(ui) = poll_end.borrow().clone() else {
                return;
            };
            let Some((channel, _)) = ui.active.borrow().clone() else {
                return;
            };
            ui.engine.send(Command::EndPoll { channel, archive });
        })
    });

    let prediction_bar = crate::prediction::PredictionBar::new(text, {
        let late = poll_end.clone();
        Rc::new(move |ask: crate::prediction::Ask| {
            let Some(ui) = late.borrow().clone() else {
                return;
            };
            let Some((channel, _)) = ui.active.borrow().clone() else {
                return;
            };
            let how = match ask {
                crate::prediction::Ask::Lock => liveside_proto::PredictionEnd::Lock,
                crate::prediction::Ask::Cancel => liveside_proto::PredictionEnd::Cancel,
                crate::prediction::Ask::Pay(outcome) => {
                    liveside_proto::PredictionEnd::Resolve { outcome }
                }
            };
            ui.engine.send(Command::EndPrediction { channel, how });
        })
    });

    // Shown only while a reply is being written.
    let reply_label = gtk::Label::builder()
        .xalign(0.0)
        .css_classes(["dim-label"])
        .visible(false)
        .build();

    // Pictures — emotes and badges — are fetched once for the window and
    // shared by the chat and the user list.
    let emotes = Rc::new(Emotes::start(animated));

    // What waits for a decision, and what the moderators have done. The
    // panel answers through the engine, which it knows nothing about, so
    // it is handed a closure that does.
    let engine_for_review: Rc<RefCell<Option<Rc<Ui>>>> = Rc::new(RefCell::new(None));
    let review = crate::review::Review::new(text, {
        let ui = engine_for_review.clone();
        Rc::new(
            move |channel: &UserId, waiting: &crate::review::Waiting, allow: bool| {
                let Some(ui) = ui.borrow().clone() else {
                    return;
                };
                let command = match waiting {
                    crate::review::Waiting::Held { id, .. } => Command::ResolveHeld {
                        channel: channel.clone(),
                        message: id.clone(),
                        allow,
                    },
                    crate::review::Waiting::Unban { id, .. } => Command::ResolveUnban {
                        channel: channel.clone(),
                        request: id.clone(),
                        allow,
                    },
                    // "Allow" here means the thing was done; the other
                    // button gives the points back.
                    crate::review::Waiting::Redeemed { id, reward_id, .. } => {
                        Command::AnswerRedemption {
                            channel: channel.clone(),
                            reward: reward_id.clone(),
                            redemption: id.clone(),
                            fulfil: allow,
                        }
                    }
                };
                ui.engine.send(command);
            },
        )
    });
    review.set_visible(false);

    // Clicking a name in the user list starts writing to that person.
    let users = crate::users::UserList::new(
        palette,
        {
            let entry = entry.clone();
            Rc::new(move |name: &str| {
                let mut text = entry.text().to_string();
                if !text.is_empty() && !text.ends_with(' ') {
                    text.push(' ');
                }
                text.push('@');
                text.push_str(name);
                text.push(' ');
                entry.set_text(&text);
                // Taking the focus selects the whole line, so the cursor is
                // put back at the end afterwards: the next thing typed has to
                // follow the name, not replace it.
                entry.grab_focus();
                entry.set_position(-1);
            })
        },
        emotes.clone(),
    );
    // The chat keeps the room it is given; the list takes what is left,
    // and gives it all back when it is hidden.
    // The chat on screen, and beside it any channels docked there; see
    // `arrange_docked`.
    let chats = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    chats.append(&stack);
    let with_review = gtk::Paned::builder()
        .orientation(gtk::Orientation::Horizontal)
        .start_child(&chats)
        .end_child(review.root())
        .resize_start_child(true)
        .resize_end_child(false)
        .shrink_end_child(false)
        .build();
    let with_users = gtk::Paned::builder()
        .orientation(gtk::Orientation::Horizontal)
        .start_child(&with_review)
        .end_child(users.root())
        .resize_start_child(true)
        .resize_end_child(false)
        .shrink_end_child(false)
        .position(620)
        .build();

    let search = crate::search::SearchBar::new(text);
    let middle = gtk::Box::new(gtk::Orientation::Vertical, 6);
    middle.append(&status_row);
    middle.append(&search.root);
    middle.append(&with_users);
    middle.append(poll_bar.root());
    middle.append(prediction_bar.root());
    middle.append(&reply_label);
    middle.append(&entry);
    middle.set_margin_start(6);
    middle.set_margin_end(6);
    middle.set_margin_top(6);
    middle.set_margin_bottom(6);

    // The channel list keeps its width and the chat takes what the window
    // gains or loses, as the panels on the right do. Shrunk with the
    // window, the list went below its own width and was cut off at the
    // left edge: Cosmic opens a window at two thirds of the screen,
    // whatever size it asks for.
    let split = gtk::Paned::builder()
        .orientation(gtk::Orientation::Horizontal)
        .start_child(&side)
        .end_child(&middle)
        .resize_start_child(false)
        .shrink_start_child(false)
        .position(200)
        .build();
    window.set_child(Some(&split));

    // The status page: always there, and what the window opens on.
    let status_view = ChatView::new(scrollback, emotes.clone(), palette);
    stack.add_named(status_view.root(), Some(STATUS_PAGE));
    // What stands in for a channel that is in a window of its own.
    let popped_label = gtk::Label::builder()
        .css_classes(["title-3"])
        .wrap(true)
        .build();
    let popped_back = gtk::Button::with_label(text.popout_back);
    let popped_show = gtk::Button::with_label(text.popout_show);
    let popped_buttons = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    popped_buttons.set_halign(gtk::Align::Center);
    popped_buttons.append(&popped_show);
    popped_buttons.append(&popped_back);
    let popped_page = gtk::Box::new(gtk::Orientation::Vertical, 12);
    popped_page.set_valign(gtk::Align::Center);
    popped_page.append(&popped_label);
    popped_page.append(&popped_buttons);
    stack.add_named(&popped_page, Some(POPPED_PAGE));
    let status_row = gtk::ListBoxRow::new();
    let status_label = gtk::Label::builder()
        .use_markup(true)
        .label(text.status_buffer)
        .xalign(0.0)
        .margin_start(6)
        .margin_end(6)
        .margin_top(4)
        .margin_bottom(4)
        .build();
    status_row.set_child(Some(&status_label));
    channels.append(&status_row);
    channels.select_row(Some(&status_row));

    let settings = Rc::new(RefCell::new(liveside_client::settings::Settings::new(
        settings_config,
        config_path,
        language,
        liveside_client::settings::Frontend::Window,
    )));
    // Why desktop notifications cannot be shown arrives from their own
    // thread, and is said in the status view once there is one.
    let (notify_failed, notify_failed_in) = async_channel::bounded::<String>(1);
    // Where a clicked notification leads, with the token that lets the
    // window come to the front, from the thread that heard the click.
    let (notify_clicked, notify_clicked_in) =
        async_channel::unbounded::<(liveside_client::notify::Place, Option<String>)>();
    let notifications = crate::notify::Notifications::new(
        settings.borrow().config().notify.clone(),
        move |error| {
            let _ = notify_failed.send_blocking(error);
        },
        std::sync::Arc::new(move |place, token| {
            let _ = notify_clicked.send_blocking((place, token));
        }),
        language.strings(),
    );
    let ui = Rc::new(Ui {
        notifications,
        engine,
        settings: settings.clone(),
        language: Cell::new(language),
        palette: Cell::new(palette),
        emotes: emotes.clone(),
        text,
        channels: channels.clone(),
        working: working.clone(),
        backend: RefCell::new(String::new()),
        status_label: status_label.clone(),
        stack: stack.clone(),
        entry: entry.clone(),
        status: status.clone(),
        views: RefCell::new(HashMap::new()),
        active: RefCell::new(None),
        held: RefCell::new(Vec::new()),
        unban_requests: RefCell::new(Vec::new()),
        channel_labels: RefCell::new(HashMap::new()),
        live: RefCell::new(HashMap::new()),
        stats: RefCell::new(HashMap::new()),
        stream_label: stream_label.clone(),
        viewers: RefCell::new(HashMap::new()),
        graph_window: RefCell::new(None),
        me: RefCell::new(None),
        watch_button: watch_button.clone(),
        popouts: RefCell::new(HashMap::new()),
        chats: chats.clone(),
        docked: RefCell::new(Vec::new()),
        layout: RefCell::new(liveside_client::layout::Layout::load(&layout_path)),
        layout_path,
        stream_chats: RefCell::new(HashMap::new()),
        popped_label: popped_label.clone(),
        popped_show: popped_show.clone(),
        review: RefCell::new(review),
        users: RefCell::new(users),
        modes: RefCell::new(HashMap::new()),
        shield: RefCell::new(HashMap::new()),
        moderate_menu: RefCell::new(None),
        joining: RefCell::new(HashMap::new()),
        open_when_joined: RefCell::new(None),
        stale: RefCell::new(HashSet::new()),
        rejoined: RefCell::new(HashSet::new()),
        data_flows: RefCell::new(Vec::new()),
        polls: RefCell::new(HashMap::new()),
        poll_bar: poll_bar.clone(),
        predictions: RefCell::new(HashMap::new()),
        prediction_bar: prediction_bar.clone(),
        reward_window: RefCell::new(None),
        channel_window: RefCell::new(None),
        schedule_window: RefCell::new(None),
        whispers: RefCell::new(HashMap::new()),
        active_whisper: RefCell::new(None),
        login_button: login_button.clone(),
        logged_in: Cell::new(false),
        status_view: RefCell::new(status_view),
        window: window.downgrade(),
        clicked: RefCell::new(None),
        replying_to: RefCell::new(None),
        reply_label: reply_label.clone(),
        scrollback,
        history: RefCell::new(History::default()),
        completion: RefCell::new(Completion::default()),
        link_reports: Rc::default(),
        search,
        blocked_window: RefCell::new(None),
        followed_window: RefCell::new(None),
        bots: RefCell::new(Vec::new()),
        bots_window: RefCell::new(None),
        collected: RefCell::new(Default::default()),
        collected_window: RefCell::new(None),
    });

    *engine_for_review.borrow_mut() = Some(ui.clone());
    // Liveside's own notices name an address now and then: where to log in.
    watch_links(&ui, &mut ui.status_view.borrow_mut());
    // Closing the main window closes the program, the channels' windows
    // with it; they stay in the layout, at the size they are now, and
    // open again at the next start. Destroyed, not closed: closing would
    // bring the channel back and forget it.
    window.connect_close_request({
        let ui = ui.clone();
        move |_| {
            let windows: Vec<(UserId, gtk::Window)> = ui
                .popouts
                .borrow()
                .iter()
                .filter_map(|(id, detached)| match detached {
                    Detached::Window(window) => Some((id.clone(), window.clone())),
                    Detached::Docked { .. } => None,
                })
                .collect();
            let mut changed = false;
            for (id, window) in windows {
                let login = ui.channel_labels.borrow().get(&id).map(|(_, l)| l.clone());
                let (width, height) = window.default_size();
                if let Some(login) = login
                    && width > 0
                    && height > 0
                {
                    changed |= ui
                        .layout
                        .borrow_mut()
                        .set_window_size(login.as_str(), (width as u32, height as u32));
                }
                ui.popouts.borrow_mut().remove(&id);
                window.destroy();
            }
            if changed {
                write_layout(&ui);
            }
            glib::Propagation::Proceed
        }
    });
    // Ctrl-Q quits, as in the terminal, and on a Mac Cmd-Q too, which
    // every program there answers. `<Primary>` alone did not do that:
    // GTK 4.24 on macOS reads it as Ctrl (tried 2026-10-29). It closes the
    // main window, so what is kept on closing is kept here too.
    let quit = gtk::gio::SimpleAction::new("quit", None);
    quit.connect_activate({
        let window = window.downgrade();
        move |_, _| {
            if let Some(window) = window.upgrade() {
                window.close();
            }
        }
    });
    application.add_action(&quit);
    let quit_keys: &[&str] = if cfg!(target_os = "macos") {
        &["<Primary>q", "<Meta>q"]
    } else {
        &["<Primary>q"]
    };
    application.set_accels_for_action("app.quit", quit_keys);
    *poll_end.borrow_mut() = Some(ui.clone());

    ui.notifications.set_say({
        let ui = Rc::downgrade(&ui);
        Box::new(move |said| {
            if let Some(ui) = ui.upgrade() {
                say_status(&ui, NoticeLevel::Warning, said);
            }
        })
    });
    glib::spawn_future_local({
        let ui = Rc::downgrade(&ui);
        async move {
            while let Ok((place, token)) = notify_clicked_in.recv().await {
                let Some(ui) = ui.upgrade() else {
                    return;
                };
                go_to(&ui, place, token);
            }
        }
    });
    glib::spawn_future_local({
        let ui = Rc::downgrade(&ui);
        async move {
            if let Ok(error) = notify_failed_in.recv().await
                && let Some(ui) = ui.upgrade()
            {
                let strings = ui.language.get().strings();
                say_status(
                    &ui,
                    NoticeLevel::Warning,
                    fill(strings.notify_desktop_failed, &[("error", &error)]),
                );
            }
        }
    });

    // Typing a line sends it to the channel on screen.
    entry.connect_activate({
        let ui = ui.clone();
        move |entry| {
            let text = entry.text().to_string();
            if text.trim().is_empty() {
                return;
            }
            ui.history.borrow_mut().remember(&text);
            ui.completion.borrow_mut().stop();
            entry.set_text("");
            submit_line(&ui, &Place::main(&ui), text);
        }
    });

    // The input line keeps what was sent (Up, Down) and completes names,
    // emotes and commands (Tab, Shift-Tab), as the terminal does.
    let keys = gtk::EventControllerKey::new();
    keys.set_propagation_phase(gtk::PropagationPhase::Capture);
    keys.connect_key_pressed({
        let ui = ui.clone();
        move |_, key, _, modifiers| {
            use gtk::gdk::{Key, ModifierType};
            let plain = !(primary(modifiers) || modifiers.contains(ModifierType::ALT_MASK));
            match key {
                Key::Tab | Key::ISO_Left_Tab | Key::KP_Tab if plain => {
                    let back =
                        key == Key::ISO_Left_Tab || modifiers.contains(ModifierType::SHIFT_MASK);
                    complete(&ui, if back { -1 } else { 1 })
                }
                Key::Up | Key::Down | Key::KP_Up | Key::KP_Down if plain => {
                    ui.completion.borrow_mut().stop();
                    let typed = ui.entry.text().to_string();
                    let line = if matches!(key, Key::Up | Key::KP_Up) {
                        ui.history.borrow_mut().earlier(&typed)
                    } else {
                        ui.history.borrow_mut().later()
                    };
                    if let Some(line) = line {
                        ui.entry.set_text(&line);
                        ui.entry.set_position(-1);
                    }
                    // Up and Down would otherwise move the focus out of
                    // the line, which nobody typing wants.
                    glib::Propagation::Stop
                }
                // Shift alone changes nothing yet; Shift-Tab is coming.
                Key::Shift_L | Key::Shift_R => glib::Propagation::Proceed,
                _ => {
                    ui.completion.borrow_mut().stop();
                    glib::Propagation::Proceed
                }
            }
        }
    });
    entry.add_controller(keys);

    // The user's own hotkeys, wherever the focus is in the window. They
    // come before the window's own keys, so one can take a key over;
    // Ctrl-C and Ctrl-Q stay as they are, as in the terminal.
    let hotkeys = gtk::EventControllerKey::new();
    hotkeys.set_propagation_phase(gtk::PropagationPhase::Capture);
    hotkeys.connect_key_pressed({
        let ui = ui.clone();
        move |_, key, _, modifiers| match run_hotkey(&ui, key, modifiers) {
            true => glib::Propagation::Stop,
            false => glib::Propagation::Proceed,
        }
    });
    window.add_controller(hotkeys);

    // A channel's own menu, on a right click in the list: a window of
    // its own, as Chatty's tabs have.
    let channel_menu = gtk::GestureClick::new();
    channel_menu.set_button(gtk::gdk::BUTTON_SECONDARY);
    channel_menu.connect_pressed({
        let ui = ui.clone();
        let channels = channels.clone();
        move |_, _, x, y| {
            let Some(row) = channels.row_at_y(y as i32) else {
                return;
            };
            let Some(name) = (unsafe { row.data::<(UserId, Login)>("channel") }) else {
                return;
            };
            let (id, login) = unsafe { name.as_ref() }.clone();
            let strings = ui.language.get().strings();
            let items = gtk::Box::new(gtk::Orientation::Vertical, 0);
            let popover = gtk::Popover::builder()
                .has_arrow(false)
                .child(&items)
                .build();
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
            item(strings.popout_open, {
                let (ui, id, login) = (ui.clone(), id.clone(), login.clone());
                Box::new(move || pop_out(&ui, id.clone(), login.clone()))
            });
            item(strings.dock_open, {
                let (ui, id, login) = (ui.clone(), id.clone(), login.clone());
                Box::new(move || dock(&ui, id.clone(), login.clone()))
            });
            item(strings.stream_chat_open, {
                let (ui, id, login) = (ui.clone(), id.clone(), login.clone());
                Box::new(move || open_stream_chat(&ui, id.clone(), login.clone()))
            });
            // As `clear` in a terminal: the reader's view only. Twitch's
            // /clear would empty it for every viewer.
            item(strings.channel_clear_here, {
                let (ui, id) = (ui.clone(), id.clone());
                Box::new(move || {
                    if let Some(view) = ui.views.borrow_mut().get_mut(&id) {
                        view.clear();
                    }
                })
            });
            // How a channel is closed, which nothing else in the window
            // said: /part was the only way.
            item(strings.channel_leave, {
                let (ui, id) = (ui.clone(), id.clone());
                Box::new(move || {
                    ui.engine.send(Command::Part {
                        channel: id.clone(),
                    })
                })
            });
            popover.set_parent(&channels);
            popover.set_pointing_to(Some(&gtk::gdk::Rectangle::new(x as i32, y as i32, 1, 1)));
            popover.connect_closed(|popover| popover.unparent());
            popover.popup();
        }
    });
    channels.add_controller(channel_menu);
    popped_show.connect_clicked({
        let ui = ui.clone();
        move |_| {
            let active = ui.active.borrow().as_ref().map(|(id, _)| id.clone());
            let detached = active.and_then(|id| ui.popouts.borrow().get(&id).cloned());
            if let Some(detached) = detached {
                detached.show();
            }
        }
    });
    popped_back.connect_clicked({
        let ui = ui.clone();
        move |_| {
            let active = ui.active.borrow().as_ref().map(|(id, _)| id.clone());
            if let Some(id) = active {
                bring_back(&ui, &id);
            }
        }
    });
    watch_button.connect_clicked({
        let ui = ui.clone();
        move |_| start_streamlink(&ui)
    });
    moderated_button.connect_clicked({
        let ui = ui.clone();
        move |_| ui.engine.send(Command::ShowModeratedChannels)
    });
    followed_button.connect_clicked({
        let ui = ui.clone();
        move |_| ui.engine.send(Command::ShowFollowedChannels)
    });
    let graph_click = gtk::GestureClick::new();
    graph_click.connect_released({
        let ui = ui.clone();
        move |_, _, _, _| open_graph(&ui, ui.active.borrow().clone())
    });
    stream_label.add_controller(graph_click);
    collected_button.connect_clicked({
        let ui = ui.clone();
        move |_| open_collected(&ui, crate::collected::Kind::Highlighted)
    });
    join_button.connect_clicked({
        let ui = ui.clone();
        move |_| {
            let Some(parent) = ui.window.upgrade() else {
                return;
            };
            let me = ui.clone();
            crate::join::ask(&parent, ui.text, move |name| {
                me.engine.send(Command::Join {
                    channel: name.into(),
                });
            });
        }
    });
    login_button.connect_clicked({
        let ui = ui.clone();
        move |_| {
            // One button, because there is only ever one of the two to do.
            if ui.logged_in.get() {
                ui.engine.send(Command::Logout);
            } else {
                ui.engine.send(Command::Login);
            }
        }
    });
    bots_button.connect_clicked({
        let ui = ui.clone();
        move |_| open_bots(&ui)
    });
    settings_button.connect_clicked({
        let ui = ui.clone();
        let window = window.clone();
        move |_| {
            let ui_for_change = ui.clone();
            crate::settings::open(
                &window,
                ui.settings.clone(),
                ui.language.get().strings(),
                move |language, config: &liveside_client::Config| {
                    let (theme, animated) = (config.ui.theme, config.ui.animated_emotes);
                    ui_for_change
                        .notifications
                        .set_config(config.notify.clone());
                    // The engine takes the rules as saved, so they apply at
                    // once, to what arrives from now on.
                    ui_for_change
                        .engine
                        .send(liveside_client::settings::rules_command(config));
                    let links = config.security.links != LinkMode::Off;
                    // Text is looked up when drawn, so new lines are in the
                    // new language at once; older ones keep their wording.
                    ui_for_change.language.set(language);
                    set_direction(language.strings());
                    let palette = crate::theme::apply(theme);
                    ui_for_change.palette.set(palette);
                    for view in ui_for_change.views.borrow_mut().values_mut() {
                        view.set_palette(palette);
                        view.set_links(links);
                    }
                    for view in ui_for_change.whispers.borrow_mut().values_mut() {
                        view.set_links(links);
                    }
                    {
                        let mut status_view = ui_for_change.status_view.borrow_mut();
                        status_view.set_palette(palette);
                        status_view.set_links(links);
                    }
                    if let Some(window) = ui_for_change.collected_window.borrow().as_ref() {
                        window.set_palette(palette);
                    }
                    let channels: Vec<UserId> = ui_for_change
                        .channel_labels
                        .borrow()
                        .keys()
                        .cloned()
                        .collect();
                    for channel in channels {
                        show_live(&ui_for_change, &channel);
                    }
                    ui_for_change
                        .users
                        .borrow_mut()
                        .set_palette(palette, language.strings());
                    // The pictures already on screen start or stop with it.
                    ui_for_change.emotes.set_animated(animated);
                },
                {
                    let ui = ui.clone();
                    // Above the settings it is asked from, which hold all
                    // input while they are open.
                    move |settings: &gtk::Window| {
                        crate::privacy::open(
                            settings,
                            &ui.data_flows.borrow(),
                            ui.language.get().strings(),
                        );
                    }
                },
            );
        }
    });

    channels.connect_row_selected({
        let ui = ui.clone();
        move |_, row| {
            let Some(row) = row else { return };
            let strings = ui.language.get().strings();
            // A reply belongs to the channel it was started in; carried
            // over, it would point Twitch at a message from somewhere else.
            ui.stop_reply();
            // A search belongs to the chat it was made in; the one opened
            // now is searched for the same, once it is the one on screen.
            if ui.search.is_open() {
                with_active_view(&ui, ChatView::end_search);
                let ui = ui.clone();
                glib::idle_add_local_once(move || search_now(&ui));
            }
            // A whisper thread is a row like any other, and selecting it
            // points the message line at that person instead of at a
            // channel.
            if let Some(login) = unsafe { row.data::<Login>("whisper") } {
                let login = unsafe { login.as_ref() }.clone();
                ui.stack.set_visible_child_name(&format!("w:{login}"));
                ui.review
                    .borrow_mut()
                    .show_channel(None, ui.language.get().strings());
                *ui.active.borrow_mut() = None;
                *ui.active_whisper.borrow_mut() = Some(login);
                refresh_placeholder(&ui);
                ui.poll_bar.show(None, strings);
                ui.prediction_bar.show(None, strings);
                refresh_users(&ui);
                return;
            }
            *ui.active_whisper.borrow_mut() = None;
            match unsafe { row.data::<(UserId, Login)>("channel") } {
                Some(name) => {
                    let (id, login) = unsafe { name.as_ref() }.clone();
                    // A channel in a window of its own is shown there: the
                    // main window says so, and brings that window up.
                    let popped = ui.popouts.borrow().get(&id).cloned();
                    match popped {
                        Some(detached) => {
                            ui.popped_show
                                .set_visible(matches!(detached, Detached::Window(_)));
                            ui.popped_label.set_text(&fill(
                                detached.here(strings),
                                &[("channel", login.as_str())],
                            ));
                            ui.stack.set_visible_child_name(POPPED_PAGE);
                            detached.show();
                        }
                        None => ui.stack.set_visible_child_name(id.as_str()),
                    }
                    ui.review
                        .borrow_mut()
                        .show_channel(Some(&id), ui.language.get().strings());
                    *ui.active.borrow_mut() = Some((id.clone(), login));
                    refresh_placeholder(&ui);
                    // Whatever is running in the channel just opened.
                    show_poll(&ui, &id);
                    refresh_room_menu(&ui);
                }
                // The status page, which belongs to no channel. The
                // placeholder says what can be done from here.
                None => {
                    ui.review
                        .borrow_mut()
                        .show_channel(None, ui.language.get().strings());
                    ui.stack.set_visible_child_name(STATUS_PAGE);
                    // Read now, so nothing is waiting there any more.
                    ui.status_label.set_markup(strings.status_buffer);
                    *ui.active.borrow_mut() = None;
                    refresh_placeholder(&ui);
                    ui.poll_bar.show(None, strings);
                    ui.prediction_bar.show(None, strings);
                }
            }
            refresh_users(&ui);
        }
    });

    review_button.connect_toggled({
        let ui = ui.clone();
        move |button| {
            ui.review.borrow().set_visible(button.is_active());
        }
    });
    users_button.connect_toggled({
        let ui = ui.clone();
        move |button| {
            ui.users.borrow().set_visible(button.is_active());
            refresh_users(&ui);
        }
    });
    // Who is active changes with every message, but redrawing the list
    // that often would be work for nothing — and it would move the names
    // under the pointer. It is looked at once a second instead, and drawn
    // again only when it has actually changed.
    glib::timeout_add_local(std::time::Duration::from_secs(1), {
        let ui = Rc::downgrade(&ui);
        move || match ui.upgrade() {
            Some(ui) => {
                refresh_users(&ui);
                // The time since the stream started moves on by itself.
                show_stats(&ui);
                glib::ControlFlow::Continue
            }
            // The window is gone; so is the reason to keep looking.
            None => glib::ControlFlow::Break,
        }
    });

    // The wheel beside the status: asking happens all over the window,
    // so rather than every caller remembering to say so, what is owed is
    // looked at a few times a second.
    glib::timeout_add_local(std::time::Duration::from_millis(150), {
        let ui = Rc::downgrade(&ui);
        move || match ui.upgrade() {
            Some(ui) => {
                show_working(&ui);
                glib::ControlFlow::Continue
            }
            None => glib::ControlFlow::Break,
        }
    });

    // Esc goes back to the newest line, as it does in the terminal.
    let keys = gtk::EventControllerKey::new();
    keys.set_propagation_phase(gtk::PropagationPhase::Capture);
    keys.connect_key_pressed({
        let ui = ui.clone();
        move |_, key, _, modifiers| {
            // The same key as in the terminal: clipping is the one thing
            // that cannot wait for a menu to be found.
            if key == gtk::gdk::Key::F4 {
                if let Some((channel, _)) = ui.active.borrow().clone() {
                    ui.engine.send(Command::CreateClip { channel });
                    return glib::Propagation::Stop;
                }
                return glib::Propagation::Proceed;
            }
            // Ctrl-R answers the newest message, as in the terminal. The
            // window has no keyboard selection to walk with, so pressing
            // it again moves the reply one message further back.
            if matches!(key, gtk::gdk::Key::r | gtk::gdk::Key::R) && primary(modifiers) {
                reply_by_key(&ui);
                return glib::Propagation::Stop;
            }
            let ctrl = primary(modifiers);
            let shift = modifiers.contains(gtk::gdk::ModifierType::SHIFT_MASK);
            if ctrl && matches!(key, gtk::gdk::Key::f | gtk::gdk::Key::F) {
                ui.search.open();
                search_now(&ui);
                return glib::Propagation::Stop;
            }
            if key == gtk::gdk::Key::F3 {
                if !ui.search.is_open() {
                    ui.search.open();
                }
                step_search(&ui, if shift { 1 } else { -1 });
                return glib::Propagation::Stop;
            }
            if key != gtk::gdk::Key::Escape {
                return glib::Propagation::Proceed;
            }
            // Esc closes the search first, wherever the focus is.
            if ui.search.is_open() {
                ui.search.close();
                return glib::Propagation::Stop;
            }
            // Esc backs out of the innermost thing first: a reply, then
            // reading back.
            if ui.stop_reply() {
                return glib::Propagation::Stop;
            }
            let active = ui.active.borrow().as_ref().map(|(id, _)| id.clone());
            let views = ui.views.borrow();
            match active.and_then(|id| views.get(&id)) {
                // Only when there is something to come back from, so Esc
                // is free to do whatever else it would.
                Some(view) if view.reading_back() => {
                    view.jump_to_bottom();
                    glib::Propagation::Stop
                }
                _ => glib::Propagation::Proceed,
            }
        }
    });
    window.add_controller(keys);

    // The search bar: typing searches, Enter goes further back, Shift+Enter
    // forward again; closing it takes the marks away.
    ui.search.entry.connect_search_changed({
        let ui = ui.clone();
        move |_| search_now(&ui)
    });
    ui.search.entry.connect_activate({
        let ui = ui.clone();
        move |_| step_search(&ui, -1)
    });
    let shift_enter = gtk::EventControllerKey::new();
    shift_enter.set_propagation_phase(gtk::PropagationPhase::Capture);
    shift_enter.connect_key_pressed({
        let ui = ui.clone();
        move |_, key, _, modifiers| {
            let enter = matches!(key, gtk::gdk::Key::Return | gtk::gdk::Key::KP_Enter);
            if enter && modifiers.contains(gtk::gdk::ModifierType::SHIFT_MASK) {
                step_search(&ui, 1);
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        }
    });
    ui.search.entry.add_controller(shift_enter);
    ui.search.older.connect_clicked({
        let ui = ui.clone();
        move |_| step_search(&ui, -1)
    });
    ui.search.newer.connect_clicked({
        let ui = ui.clone();
        move |_| step_search(&ui, 1)
    });
    ui.search.root.connect_search_mode_enabled_notify({
        let ui = ui.clone();
        move |bar| {
            if !bar.is_search_mode() {
                with_active_view(&ui, ChatView::end_search);
                ui.search.show(None, ui.language.get().strings());
                ui.entry.grab_focus();
            }
        }
    });

    // Moderating the room on screen.
    let modes_now = {
        let ui = ui.clone();
        Rc::new(move || {
            ui.active
                .borrow()
                .as_ref()
                .and_then(|(id, _)| ui.modes.borrow().get(id).cloned())
                .unwrap_or_default()
        }) as Rc<dyn Fn() -> liveside_proto::model::RoomModes>
    };
    let shield_now = {
        let ui = ui.clone();
        Rc::new(move || {
            let active = ui.active.borrow().as_ref().map(|(id, _)| id.clone());
            active
                .and_then(|id| ui.shield.borrow().get(&id).copied())
                .unwrap_or(false)
        }) as Rc<dyn Fn() -> bool>
    };
    moderate_button.set_menu_model(Some(&crate::moderation::room_menu(text, false)));
    // Rebuilt when the state changes or the channel does, rather than as
    // it opens: setting a menu model on a popover that is opening closes
    // it again.
    *ui.moderate_menu.borrow_mut() = Some(moderate_button.clone());
    let room_actions = crate::moderation::room_actions(ui.clone(), modes_now, shield_now.clone());
    {
        // Starting a poll asks for a question first, so it opens a dialog
        // rather than sending a command the way the rest of the menu does.
        let action = gtk::gio::SimpleAction::new("poll", None);
        let ui = ui.clone();
        let window = window.clone();
        action.connect_activate(move |_, _| {
            let Some((channel, _)) = ui.active.borrow().clone() else {
                return;
            };
            let ui = ui.clone();
            crate::poll::ask(
                &window,
                ui.language.get().strings(),
                move |title, choices, seconds| {
                    ui.engine.send(Command::CreatePoll {
                        channel: channel.clone(),
                        title,
                        choices,
                        seconds,
                    });
                },
            );
        });
        room_actions.add_action(&action);
    }
    {
        let action = gtk::gio::SimpleAction::new("prediction", None);
        let ui = ui.clone();
        let window = window.clone();
        action.connect_activate(move |_, _| {
            let Some((channel, _)) = ui.active.borrow().clone() else {
                return;
            };
            let ui = ui.clone();
            crate::prediction::ask(
                &window,
                ui.language.get().strings(),
                move |title, outcomes, seconds| {
                    ui.engine.send(Command::CreatePrediction {
                        channel: channel.clone(),
                        title,
                        outcomes,
                        seconds,
                    });
                },
            );
        });
        room_actions.add_action(&action);
    }
    {
        let action = gtk::gio::SimpleAction::new("rewards", None);
        let ui = ui.clone();
        let window = window.clone();
        action.connect_activate(move |_, _| {
            let Some((channel, _)) = ui.active.borrow().clone() else {
                return;
            };
            let strings = ui.language.get().strings();
            let for_add = ui.clone();
            let add_channel = channel.clone();
            let for_remove = ui.clone();
            let remove_channel = channel.clone();
            let opened = crate::rewards::RewardWindow::open(
                &window,
                strings,
                Rc::new(move |title, cost, prompt| {
                    for_add.engine.send(Command::CreateReward {
                        channel: add_channel.clone(),
                        title,
                        cost,
                        prompt,
                    });
                }),
                Rc::new(move |reward: &str| {
                    for_remove.engine.send(Command::DeleteReward {
                        channel: remove_channel.clone(),
                        reward: reward.to_owned(),
                    });
                }),
                {
                    let ui = ui.clone();
                    let channel = channel.clone();
                    Rc::new(move |reward: &str, change: crate::rewards::Change| {
                        ui.engine.send(Command::UpdateReward {
                            channel: channel.clone(),
                            reward: reward.to_owned(),
                            title: change.title,
                            cost: change.cost,
                            prompt: change.prompt,
                            enabled: None,
                            paused: change.paused,
                        });
                    })
                },
            );
            *ui.reward_window.borrow_mut() = Some(opened);
            // Drawn from the engine's answer, which also keeps the list
            // right after adding or removing one.
            ui.engine.send(Command::ShowRewards { channel });
        });
        room_actions.add_action(&action);
    }
    {
        let action = gtk::gio::SimpleAction::new("channel", None);
        let ui = ui.clone();
        let window = window.clone();
        action.connect_activate(move |_, _| {
            let Some((channel, _)) = ui.active.borrow().clone() else {
                return;
            };
            let strings = ui.language.get().strings();
            let for_save = ui.clone();
            let save_channel = channel.clone();
            let opened = crate::channel::ChannelWindow::open(
                &window,
                strings,
                Rc::new(move |now: liveside_proto::model::ChannelInfo| {
                    // Only what was edited is sent: Twitch leaves the
                    // rest alone, and a form that always sent everything
                    // would overwrite what somebody changed meanwhile.
                    let Some(form) = for_save.channel_window.borrow().clone() else {
                        return;
                    };
                    let changed = form.changes(&now);
                    for_save.engine.send(Command::SetChannelInfo {
                        channel: save_channel.clone(),
                        title: changed.title,
                        game: changed.game,
                        language: changed.language,
                        tags: changed.tags,
                        delay: changed.delay,
                        labels: changed.labels,
                    });
                }),
            );
            *ui.channel_window.borrow_mut() = Some(opened);
            ui.engine.send(Command::ShowChannelInfo { channel });
        });
        room_actions.add_action(&action);
    }
    {
        let action = gtk::gio::SimpleAction::new("schedule", None);
        let ui = ui.clone();
        let window = window.clone();
        action.connect_activate(move |_, _| {
            let Some((channel, _)) = ui.active.borrow().clone() else {
                return;
            };
            let strings = ui.language.get().strings();
            let for_act = ui.clone();
            let act_channel = channel.clone();
            let opened = crate::schedule::ScheduleWindow::open(
                &window,
                strings,
                Rc::new(move |ask: crate::schedule::Ask| {
                    use crate::schedule::Ask;
                    let channel = act_channel.clone();
                    let command = match ask {
                        Ask::Cancel { segment, canceled } => Command::CancelSegment {
                            channel,
                            segment,
                            canceled,
                        },
                        Ask::Delete { segment } => Command::DeleteSegment { channel, segment },
                        Ask::Add {
                            start,
                            minutes,
                            title,
                            weekly,
                        } => Command::AddSegment {
                            channel,
                            start,
                            minutes,
                            title,
                            weekly,
                        },
                        Ask::Vacation(away) => Command::SetVacation { channel, away },
                    };
                    for_act.engine.send(command);
                }),
            );
            *ui.schedule_window.borrow_mut() = Some(opened);
            ui.engine.send(Command::ShowSchedule { channel });
        });
        room_actions.add_action(&action);
    }
    {
        let action = gtk::gio::SimpleAction::new("clip", None);
        let ui = ui.clone();
        action.connect_activate(move |_, _| {
            let Some((channel, _)) = ui.active.borrow().clone() else {
                return;
            };
            ui.engine.send(Command::CreateClip { channel });
        });
        room_actions.add_action(&action);
    }
    moderate_button.insert_action_group("room", Some(&room_actions));

    // Messages from the engine arrive on the GTK thread.
    glib::spawn_future_local({
        let ui = ui.clone();
        async move {
            while let Ok(from) = ui.engine.messages.recv().await {
                match from {
                    FromEngine::Message(message) => apply(&ui, message),
                    FromEngine::Lost => lose_engine(&ui),
                }
            }
            // The engine is gone for good: one in this process, or a
            // daemon that was never there or would not have us. Said where the reader is looking, rather than a
            // window that looks alive and does nothing, which is what a
            // tester on WSL2 saw. The terminal says the same and quits; a
            // window stays, so what was read can still be read.
            let text = ui.language.get().strings().connection_closed.to_owned();
            say(&ui, NoticeLevel::Error, text.clone());
            if ui.active.borrow().is_some() {
                say_status(&ui, NoticeLevel::Error, text);
            }
        }
    });

    // The demo backend logs in instantly, and the terminal frontend asks
    // it to; without this the window ran the demo as nobody, and anything
    // that needs an account — whispering, sending a message — was refused
    // for a reason that is not true of the demo.
    if demo {
        ui.engine.send(Command::Login);
    }

    window.present();
}

/// How a poll stands, as one line: every choice with its count.
fn standing(poll: &liveside_proto::model::Poll) -> String {
    poll.choices
        .iter()
        .map(|choice| format!("{} {}", choice.title, choice.votes))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Draws the poll of the channel on screen, or hides the bar. Called when
/// a poll changes and when the channel being read does.
fn show_poll(ui: &Rc<Ui>, channel: &UserId) {
    let active = ui.active.borrow().as_ref().map(|(id, _)| id.clone());
    if active.as_ref() != Some(channel) {
        return;
    }
    let strings = ui.language.get().strings();
    ui.poll_bar.show(ui.polls.borrow().get(channel), strings);
    ui.prediction_bar
        .show(ui.predictions.borrow().get(channel), strings);
}

/// How a prediction stands, as one line: each outcome with who has bet
/// what on it.
fn prediction_standing(prediction: &liveside_proto::model::Prediction) -> String {
    prediction
        .outcomes
        .iter()
        .map(|outcome| format!("{} ({})", outcome.title, outcome.points))
        .collect::<Vec<_>>()
        .join(", ")
}

/// What the reader has in front of them, for deciding whether something
/// is news to them.
fn seen(ui: &Rc<Ui>) -> liveside_client::notify::Seen {
    use liveside_client::notify::Place;
    // A channel in a window of its own is on screen while that window is
    // the one in front.
    let popped = ui
        .popouts
        .borrow()
        .iter()
        .find(|(_, detached)| matches!(detached, Detached::Window(w) if w.is_active()))
        .map(|(id, _)| id.clone());
    if let Some(id) = popped {
        return liveside_client::notify::Seen {
            focused: true,
            place: Some(Place::Channel(id)),
            beside: Vec::new(),
        };
    }
    let focused = ui.window.upgrade().is_some_and(|window| window.is_active());
    let place = match (&*ui.active.borrow(), &*ui.active_whisper.borrow()) {
        (Some((channel, _)), _) => Some(Place::Channel(channel.clone())),
        (None, Some(login)) => Some(Place::Whisper(login.clone())),
        (None, None) => None,
    };
    // Docked channels are on screen with the main window.
    let beside = ui
        .popouts
        .borrow()
        .iter()
        .filter(|(_, detached)| matches!(detached, Detached::Docked { .. }))
        .map(|(id, _)| Place::Channel(id.clone()))
        .collect();
    liveside_client::notify::Seen {
        focused,
        place,
        beside,
    }
}

fn apply(ui: &Rc<Ui>, message: ServerMessage) {
    match message {
        ServerMessage::Welcome { server, .. } => {
            ui.status.set_text(&server);
            if ui.engine.regain() {
                let text = ui.language.get().strings().engine_back.to_owned();
                say(ui, NoticeLevel::Info, text.clone());
                if ui.active.borrow().is_some() {
                    say_status(ui, NoticeLevel::Info, text);
                }
            }
        }
        ServerMessage::Snapshot { snapshot, .. } => {
            // Before the lines, so they are written with what is known.
            ui.link_reports.borrow_mut().extend(
                snapshot
                    .link_reports
                    .into_iter()
                    .map(|report| ((report.url, report.source), report.state)),
            );
            for channel in snapshot.channels {
                ensure_channel(ui, &channel.channel);
                let id = channel.channel.id.clone();
                ui.stale.borrow_mut().remove(&id);
                {
                    let mut views = ui.views.borrow_mut();
                    let Some(view) = views.get_mut(&id) else {
                        continue;
                    };
                    // A snapshot replaces what was known, and a channel
                    // already on screen got its lines once before.
                    view.clear();
                    for line in &channel.lines {
                        view.push(line, ui.text);
                    }
                    view.set_present(channel.chatters);
                }
                ui.live.borrow_mut().insert(id.clone(), channel.live);
                ui.notifications
                    .know_channel(&channel.channel, channel.live);
                if let Some(stats) = channel.stats {
                    ui.stats.borrow_mut().insert(id.clone(), stats);
                }
                ui.viewers.borrow_mut().insert(id.clone(), channel.viewers);
                if let Some(poll) = channel.poll {
                    ui.polls.borrow_mut().insert(id.clone(), poll);
                }
                if let Some(prediction) = channel.prediction {
                    ui.predictions.borrow_mut().insert(id.clone(), prediction);
                }
                show_live(ui, &id);
                show_poll(ui, &id);
            }
            // Below the joined ones, in the order they were asked for, as
            // they would have come had the window been there to see it.
            for login in &snapshot.joining {
                show_joining(ui, login);
            }
            *ui.collected.borrow_mut() = liveside_client::collected::Collected::from_snapshot(
                snapshot.highlighted,
                snapshot.ignored,
            );
            *ui.backend.borrow_mut() = snapshot.backend.clone();
            *ui.data_flows.borrow_mut() = snapshot.data_flows;
            show_bots(ui, snapshot.bots);
            show_account(ui, snapshot.account.as_ref());
            rejoin(ui);
        }
        ServerMessage::Event { event, .. } => {
            ui.notifications
                .consider(&event, &seen(ui), ui.language.get().strings());
            apply_event(ui, event);
        }
        ServerMessage::Response { id, error } => {
            ui.engine.answered(id);
            show_working(ui);
            if let Some(error) = error {
                let text = ui.language.get().strings();
                let said = if error.code.is_empty() {
                    // Twitch's own wording, which is not ours to translate;
                    // the sentence around it says what happened.
                    fill(text.refused_by_twitch, &[("detail", &error.message)])
                } else {
                    liveside_client::lang::from_engine(
                        text,
                        &error.code,
                        &error.args,
                        &error.message,
                    )
                };
                say(ui, NoticeLevel::Error, said);
            }
        }
        ServerMessage::Rejected { reason } => say_status(ui, NoticeLevel::Error, reason),
    }
}

fn apply_event(ui: &Rc<Ui>, event: Event) {
    match event {
        Event::Bots { bots } => show_bots(ui, bots),
        Event::BotState { name, state } => {
            if let Some(bot) = ui
                .bots
                .borrow_mut()
                .iter_mut()
                .find(|bot| bot.config.name == name)
            {
                bot.state = state.clone();
            }
            if let Some(window) = bots_window(ui) {
                window.state(&name, &state);
            }
            // A bot that falls over is said in the main window too, on
            // the status page with what else the engine says: its own
            // window may well be closed.
            if liveside_client::bots::went_wrong(&state) {
                let text = ui.language.get().strings();
                let said = liveside_client::bots::state_said(text, &state);
                say_to(
                    ui,
                    None,
                    NoticeLevel::Warning,
                    fill(text.bots_said, &[("bot", &name), ("state", &said)]),
                );
            }
        }
        Event::BotLog { name, line } => {
            if let Some(bot) = ui
                .bots
                .borrow_mut()
                .iter_mut()
                .find(|bot| bot.config.name == name)
            {
                bot.log.push(line.clone());
            }
            if let Some(window) = bots_window(ui) {
                window.log_line(&name, &line);
            }
        }
        Event::ChannelJoined { channel, modes } => {
            ui.modes.borrow_mut().insert(channel.id.clone(), modes);
            if ui.stale.borrow_mut().remove(&channel.id) {
                ui.rejoined.borrow_mut().insert(channel.id.clone());
                show_live(ui, &channel.id);
            }
            ensure_channel(ui, &channel);
        }
        // Struck through where they are, as the terminal does: what a
        // moderator removed must not read as still there.
        Event::MessageDeleted { channel, message } => {
            if let Some(view) = ui.views.borrow_mut().get_mut(&channel) {
                view.delete_message(&message);
            }
            if let Some(chat) = stream_chat(ui, &channel) {
                chat.delete_message(&message);
            }
        }
        Event::UserCleared { channel, user, .. } => {
            if let Some(view) = ui.views.borrow_mut().get_mut(&channel) {
                view.delete_user(&user);
            }
            if let Some(chat) = stream_chat(ui, &channel) {
                chat.delete_user(&user);
            }
        }
        Event::ChatCleared { channel } => {
            if let Some(view) = ui.views.borrow_mut().get_mut(&channel) {
                view.clear();
            }
            if let Some(chat) = stream_chat(ui, &channel) {
                chat.clear();
            }
        }
        Event::ChannelParted { channel } => {
            ui.review
                .borrow_mut()
                .forget(&channel, ui.language.get().strings());
            remove_channel(ui, &channel);
        }
        Event::RoomModesChanged { channel, modes } => {
            ui.modes.borrow_mut().insert(channel, modes);
        }
        Event::Line { channel, line } => {
            if let Some(chat) = stream_chat(ui, &channel) {
                chat.push(&line);
            }
            // A new title or category marks the viewer graph.
            if let LineContent::Notice { code, args, .. } = &line.content
                && code == "event.channel_update"
                && !line.flags.historical
            {
                let arg = |name: &str| {
                    args.iter()
                        .find(|(k, _)| k == name)
                        .map(|(_, v)| v.clone())
                        .unwrap_or_default()
                };
                if let Some(history) = ui.viewers.borrow_mut().get_mut(&channel) {
                    history.mark(line.time, &arg("title"), &arg("game"));
                }
                refresh_graph(ui, &channel);
            }
            // A moderator's doing goes to the panel as well, where it
            // stays put while the chat runs on.
            if let LineContent::Notice {
                code, text, args, ..
            } = &line.content
                && code.starts_with("mod.")
            {
                let said = liveside_client::lang::from_engine(
                    ui.language.get().strings(),
                    code,
                    args,
                    text,
                );
                ui.review.borrow_mut().add_log(&channel, said);
            }
            collect(ui, crate::collected::Kind::Highlighted, &channel, &line);
            let mut views = ui.views.borrow_mut();
            if let Some(view) = views.get_mut(&channel) {
                view.push(&line, ui.text);
            }
        }
        // Hidden from the channel, kept in the list of what was.
        Event::Ignored { channel, line } => {
            collect(ui, crate::collected::Kind::Ignored, &channel, &line);
        }
        Event::History { channel, lines } => {
            if ui.rejoined.borrow_mut().remove(&channel) {
                return;
            }
            let mut views = ui.views.borrow_mut();
            if let Some(view) = views.get_mut(&channel) {
                for line in &lines {
                    view.push(line, ui.text);
                }
            }
        }
        // Someone asking to be let back in, with the answer beside it.
        Event::UnbanRequested { channel, request } => {
            let strings = ui.language.get().strings();
            {
                let mut queue = ui.unban_requests.borrow_mut();
                if !queue.iter().any(|(_, id, _)| id == &request.id) {
                    queue.push((
                        channel.clone(),
                        request.id.clone(),
                        request.user.display_name.clone(),
                    ));
                }
            }
            ui.review.borrow_mut().add_waiting(
                &channel,
                crate::review::Waiting::Unban {
                    id: request.id.clone(),
                    user: request.user.display_name.clone(),
                    text: request.text.clone(),
                },
                strings,
            );
            let mut views = ui.views.borrow_mut();
            if let Some(view) = views.get_mut(&channel) {
                let said = fill(
                    strings.unban_request,
                    &[
                        ("user", &request.user.display_name),
                        ("text", &request.text),
                    ],
                );
                let ui = ui.clone();
                let id = request.id.clone();
                let channel = channel.clone();
                view.push_decision(said, strings, move |allow| {
                    ui.engine.send(Command::ResolveUnban {
                        channel: channel.clone(),
                        request: id.clone(),
                        allow,
                    });
                });
            }
        }
        Event::UnbanResolved {
            channel,
            request,
            status,
            ..
        } => {
            let strings = ui.language.get().strings();
            let user = {
                let mut queue = ui.unban_requests.borrow_mut();
                let at = queue.iter().position(|(_, id, _)| id == &request);
                at.map(|at| queue.remove(at).2)
            };
            ui.review.borrow_mut().resolve(&channel, &request, strings);
            let Some(user) = user else { return };
            let pattern = if status == "approved" {
                strings.unban_approved
            } else {
                strings.unban_denied
            };
            let said = fill(pattern, &[("user", &user)]);
            let mut views = ui.views.borrow_mut();
            if let Some(view) = views.get_mut(&channel) {
                view.push(&notice(NoticeLevel::Info, said), strings);
            }
        }
        // How the poll stands. Kept per channel, and drawn when it is the
        // channel being read.
        Event::PollUpdate { channel, poll } => {
            use liveside_proto::model::PollStatus;
            let strings = ui.language.get().strings();
            let said = match poll.status {
                // A count on the way is shown in the bar, not said: a line
                // per vote would bury the chat the poll is running in.
                PollStatus::Active => None,
                PollStatus::Completed => {
                    Some(fill(strings.poll_result, &[("choices", &standing(&poll))]))
                }
                PollStatus::Terminated => {
                    Some(fill(strings.poll_stopped, &[("choices", &standing(&poll))]))
                }
                PollStatus::Archived => Some(strings.poll_hidden.to_owned()),
            };
            let running = poll.status == PollStatus::Active;
            let starting = running && !ui.polls.borrow().contains_key(&channel);
            {
                let mut polls = ui.polls.borrow_mut();
                match running {
                    true => polls.insert(channel.clone(), poll.clone()),
                    false => polls.remove(&channel),
                };
            }
            if starting {
                say_in(
                    ui,
                    &channel,
                    fill(strings.poll_started, &[("title", &poll.title)]),
                );
            }
            if let Some(said) = said {
                say_in(ui, &channel, said);
            }
            show_poll(ui, &channel);
        }
        // How a prediction stands. Betting closing is worth saying: from
        // there somebody has to decide, and Twitch gives them a day.
        Event::PredictionUpdate {
            channel,
            prediction,
        } => {
            use liveside_proto::model::PredictionStatus;
            let strings = ui.language.get().strings();
            let was = ui.predictions.borrow().get(&channel).map(|p| p.status);
            let said = match prediction.status {
                PredictionStatus::Active if was.is_none() => {
                    Some(fill(strings.pred_started, &[("title", &prediction.title)]))
                }
                PredictionStatus::Active => None,
                PredictionStatus::Locked if was != Some(PredictionStatus::Locked) => Some(fill(
                    strings.pred_locked,
                    &[("outcomes", &prediction_standing(&prediction))],
                )),
                PredictionStatus::Locked => None,
                PredictionStatus::Resolved => {
                    let won = prediction
                        .winner
                        .as_ref()
                        .and_then(|id| prediction.outcomes.iter().find(|o| &o.id == id))
                        .map(|outcome| outcome.title.clone())
                        .unwrap_or_default();
                    Some(fill(strings.pred_paid, &[("outcome", &won)]))
                }
                PredictionStatus::Canceled => Some(strings.pred_canceled.to_owned()),
            };
            {
                let mut predictions = ui.predictions.borrow_mut();
                match prediction.status {
                    PredictionStatus::Active | PredictionStatus::Locked => {
                        predictions.insert(channel.clone(), prediction)
                    }
                    _ => predictions.remove(&channel),
                };
            }
            if let Some(said) = said {
                say_in(ui, &channel, said);
            }
            show_poll(ui, &channel);
        }
        // What was spent on a reward made from here: into the queue, and
        // out of it once it has been answered anywhere.
        Event::RedemptionUpdate {
            channel,
            redemption,
        } => {
            use liveside_proto::model::RedemptionStatus;
            let strings = ui.language.get().strings();
            match redemption.status {
                RedemptionStatus::Unfulfilled => ui.review.borrow_mut().add_waiting(
                    &channel,
                    crate::review::Waiting::Redeemed {
                        id: redemption.id,
                        reward_id: redemption.reward_id,
                        reward: redemption.reward_title,
                        user: redemption.user.display_name,
                        cost: redemption.cost,
                        text: redemption.text,
                    },
                    strings,
                ),
                _ => {
                    let said = fill(
                        match redemption.status {
                            RedemptionStatus::Fulfilled => strings.reward_done,
                            _ => strings.reward_refunded,
                        },
                        &[
                            ("reward", &redemption.reward_title),
                            ("user", &redemption.user.display_name),
                        ],
                    );
                    let mut review = ui.review.borrow_mut();
                    review.resolve(&channel, &redemption.id, strings);
                    review.add_log(&channel, said);
                }
            }
        }
        // Marks the message rather than adding a line of its own.
        Event::SuspiciousMessage {
            channel,
            message,
            suspicion,
            ..
        } => {
            if let Some(view) = ui.views.borrow_mut().get_mut(&channel) {
                view.mark_suspicious(&message, *suspicion, ui.language.get().strings());
            }
        }
        // Asked for and not here yet: a row at once, with a wheel.
        Event::ChannelJoining { login } => show_joining(ui, &login),
        // ...and it will not arrive, so the row goes.
        Event::ChannelJoinFailed { login } => drop_joining(ui, &login),
        // Shield Mode, kept so its menu item can say which way it goes.
        Event::ShieldMode { channel, on } => {
            ui.shield.borrow_mut().insert(channel, on);
            refresh_room_menu(ui);
        }
        // What is known about a link, written after it wherever it is.
        Event::LinkReport { report } => {
            let strings = ui.language.get().strings();
            ui.link_reports
                .borrow_mut()
                .insert((report.url.clone(), report.source), report.state);
            for view in ui.views.borrow().values() {
                view.show_report(&report.url, strings);
            }
            for view in ui.whispers.borrow().values() {
                view.show_report(&report.url, strings);
            }
        }
        // A whisper: into the thread with that person, which is made
        // when the first one goes either way.
        Event::Whisper { user, line } => {
            ensure_whisper(ui, &user);
            if let Some(view) = ui.whispers.borrow_mut().get_mut(&user.login) {
                view.push(&line, ui.language.get().strings());
            }
        }
        // When the stream is next: into the window when it is open, and
        // said in the channel otherwise.
        Event::Schedule { channel, schedule } => {
            let strings = ui.language.get().strings();
            let shown = ui.schedule_window.borrow().clone();
            if let Some(window) = shown {
                if window.is_open() {
                    window.show(&schedule, strings);
                    return;
                }
                *ui.schedule_window.borrow_mut() = None;
            }
            let said = match schedule.segments.first() {
                Some(next) => fill(
                    strings.schedule_line,
                    &[
                        ("start", &next.start.replace('T', " ")),
                        (
                            "title",
                            &match next.title.is_empty() {
                                true => next.category.clone(),
                                false => next.title.clone(),
                            },
                        ),
                    ],
                ),
                None => strings.schedule_none.to_owned(),
            };
            say_in(ui, &channel, said);
        }
        // What the channel says about itself: into the form when it is
        // open, and said in the channel otherwise.
        Event::ChannelInfo { channel, info } => {
            let strings = ui.language.get().strings();
            let form = ui.channel_window.borrow().clone();
            if let Some(form) = form {
                if form.is_open() {
                    form.show(&info);
                    return;
                }
                *ui.channel_window.borrow_mut() = None;
            }
            let said = match info.title.is_empty() && info.game.is_empty() {
                true => strings.channel_said_nothing.to_owned(),
                false => fill(
                    strings.channel_said,
                    &[("title", &info.title), ("game", &info.game)],
                ),
            };
            say_in(ui, &channel, said);
        }
        // The rewards a channel offers, said in the channel it was asked
        // in: a list is read once, not watched.
        Event::Rewards { channel, rewards } => {
            let strings = ui.language.get().strings();
            // With the window open the list belongs in it; a line in the
            // chat as well would say the same thing twice.
            let shown = ui.reward_window.borrow().clone();
            if let Some(window) = shown {
                if window.is_open() {
                    window.show(&rewards, strings);
                    return;
                }
                *ui.reward_window.borrow_mut() = None;
            }
            let said = if rewards.is_empty() {
                strings.reward_none.to_owned()
            } else {
                let list = rewards
                    .iter()
                    .map(|reward| {
                        let mark = match reward.ours {
                            true => String::new(),
                            false => format!(" {}", strings.reward_theirs),
                        };
                        format!("{} ({}){mark}", reward.title, reward.cost)
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                fill(
                    strings.reward_list,
                    &[("count", &rewards.len().to_string()), ("rewards", &list)],
                )
            };
            say_in(ui, &channel, said);
        }
        // What the engine does with the reader's data; kept until the
        // Settings window asks for it.
        Event::DataFlows { flows } => *ui.data_flows.borrow_mut() = flows,
        // A list asked for with /mods, /vips or /banned.
        Event::People {
            channel,
            list,
            people,
            total,
            points,
        } => {
            let strings = ui.language.get().strings();
            let Some(window) = ui.window.upgrade() else {
                return;
            };
            let for_card = ui.clone();
            let for_unban = ui.clone();
            let card_channel = channel.clone();
            crate::people::open(
                &window,
                list,
                &people,
                total,
                points,
                strings,
                Rc::new(move |name: &str| {
                    for_card.engine.send(Command::ShowUser {
                        channel: card_channel.clone(),
                        user: name.to_lowercase().into(),
                    });
                }),
                Rc::new(move |name: &str| {
                    for_unban.engine.send(Command::Input {
                        channel: channel.clone(),
                        text: format!("/unban {name}"),
                        reply_to: None,
                    });
                }),
            );
        }
        // The channels this account moderates, each one a button to join.
        Event::ModeratedChannels { channels } => {
            let Some(parent) = ui.window.upgrade() else {
                return;
            };
            // By login: a channel is the same one under its name wherever
            // the list came from.
            let joined: Vec<Login> = ui
                .channel_labels
                .borrow()
                .values()
                .map(|(_, login)| login.clone())
                .collect();
            let for_join = ui.clone();
            crate::moderated::open(
                &parent,
                &channels,
                ui.language.get().strings(),
                |channel| joined.contains(&channel.login),
                Rc::new(move |login: &str| {
                    for_join.engine.send(Command::Join {
                        channel: login.into(),
                    });
                }),
            );
        }
        // The followed channels: into the window if it is open, a new
        // window otherwise.
        Event::FollowedChannels { channels } => {
            let open = ui
                .followed_window
                .borrow()
                .as_ref()
                .filter(|window| window.is_open())
                .cloned();
            let window = match open {
                Some(window) => {
                    window.present();
                    window
                }
                None => {
                    let Some(parent) = ui.window.upgrade() else {
                        return;
                    };
                    // Asked when drawn, not when opened: Refresh after a
                    // join marks it.
                    let for_joined = ui.clone();
                    let for_join = ui.clone();
                    let for_refresh = ui.clone();
                    let window = crate::followed::FollowedWindow::open(
                        &parent,
                        ui.language.get().strings(),
                        Rc::new(move |channel: &liveside_proto::model::Channel| {
                            // By login: a channel is the same one under its
                            // name wherever the list came from.
                            for_joined
                                .channel_labels
                                .borrow()
                                .values()
                                .any(|(_, login)| login == &channel.login)
                        }),
                        Rc::new(move |login: &str| {
                            for_join.engine.send(Command::Join {
                                channel: login.into(),
                            });
                        }),
                        move || for_refresh.engine.send(Command::ShowFollowedChannels),
                    );
                    *ui.followed_window.borrow_mut() = Some(window.clone());
                    window
                }
            };
            window.show(&channels);
        }
        // Who of the followed streams, heard every minute: into the
        // window if it is open, and a line in the status view for each
        // one that began.
        Event::FollowedStreams { live, started } => {
            let open = ui
                .followed_window
                .borrow()
                .as_ref()
                .filter(|window| window.is_open())
                .cloned();
            if let Some(window) = open {
                window.show_live(&live);
            }
            let text = ui.language.get().strings();
            for followed in live.iter().filter(|f| started.contains(&f.channel.id)) {
                let about = followed
                    .live
                    .as_ref()
                    .map(|stream| liveside_client::followed::about(stream, text))
                    .unwrap_or_default();
                say_status(
                    ui,
                    NoticeLevel::Info,
                    fill(
                        text.followed_started,
                        &[
                            ("channel", &followed.channel.display_name),
                            ("about", &about),
                            ("login", followed.channel.login.as_str()),
                        ],
                    ),
                );
            }
        }
        // What the channel blocks: into the table if it is open for this
        // channel, a new table otherwise.
        Event::BlockedTerms { channel, terms } => {
            let open = ui
                .blocked_window
                .borrow()
                .as_ref()
                .filter(|(shown, window)| shown == &channel && window.is_open())
                .map(|(_, window)| window.clone());
            let window = match open {
                // Asked for again from the menu: brought to the front.
                Some(window) => {
                    window.present();
                    window
                }
                None => {
                    let Some(parent) = ui.window.upgrade() else {
                        return;
                    };
                    let name = ui
                        .channel_labels
                        .borrow()
                        .get(&channel)
                        .map(|(_, login)| login.to_string())
                        .unwrap_or_default();
                    let for_act = ui.clone();
                    let act_channel = channel.clone();
                    let window = crate::blocked::BlockedWindow::open(
                        &parent,
                        &name,
                        ui.language.get().strings(),
                        Rc::new(move |act: crate::blocked::Act| {
                            let text = match act {
                                crate::blocked::Act::Block(term) => format!("/blockterm {term}"),
                                crate::blocked::Act::Unblock(term) => {
                                    format!("/unblockterm {term}")
                                }
                            };
                            for command in [text, "/blockedterms".to_owned()] {
                                for_act.engine.send(Command::Input {
                                    channel: act_channel.clone(),
                                    text: command,
                                    reply_to: None,
                                });
                            }
                        }),
                    );
                    *ui.blocked_window.borrow_mut() = Some((channel.clone(), window.clone()));
                    window
                }
            };
            window.show(&terms);
        }
        // Somebody's card, asked for from their message's menu.
        Event::UserCard { channel, card } => {
            let strings = ui.language.get().strings();
            let Some(window) = ui.window.upgrade() else {
                return;
            };
            let messages = ui
                .views
                .borrow()
                .get(&channel)
                .map(|view| view.messages_from(&card.user.login, 20))
                .unwrap_or_default();
            let ui_for_action = ui.clone();
            crate::card::open(
                &window,
                &card,
                &messages,
                strings,
                Rc::new(move |command: &str| {
                    ui_for_action.engine.send(Command::Input {
                        channel: channel.clone(),
                        text: command.to_owned(),
                        reply_to: None,
                    });
                }),
            );
        }
        // What AutoMod caught, with the two decisions beside it.
        Event::MessageHeld { channel, held } => {
            let strings = ui.language.get().strings();
            ui.review.borrow_mut().add_waiting(
                &channel,
                crate::review::Waiting::Held {
                    id: held.id.clone(),
                    user: held.sender.display_name.clone(),
                    text: held.text.clone(),
                    detail: held.detail.clone(),
                },
                strings,
            );
            let mut views = ui.views.borrow_mut();
            {
                let mut queue = ui.held.borrow_mut();
                if !queue.iter().any(|(_, id, _)| id == &held.id) {
                    queue.push((
                        channel.clone(),
                        held.id.clone(),
                        held.sender.display_name.clone(),
                    ));
                }
            }
            if let Some(view) = views.get_mut(&channel) {
                let ui = ui.clone();
                let id = held.id.clone();
                let channel = channel.clone();
                view.push_held(&held, strings, move |allow| {
                    ui.engine.send(Command::ResolveHeld {
                        channel: channel.clone(),
                        message: id.clone(),
                        allow,
                    });
                });
            }
        }
        Event::HeldResolved {
            channel,
            status,
            message,
            ..
        } => {
            let strings = ui.language.get().strings();
            let user = {
                let mut queue = ui.held.borrow_mut();
                let at = queue.iter().position(|(_, id, _)| id == &message);
                at.map(|at| queue.remove(at).2)
            };
            ui.review
                .borrow_mut()
                .resolve(&channel, message.as_str(), strings);
            let Some(user) = user else { return };
            let pattern = match status.as_str() {
                "approved" => strings.held_approved,
                "denied" => strings.held_denied,
                _ => strings.held_expired,
            };
            let said = fill(pattern, &[("user", &user)]);
            let mut views = ui.views.borrow_mut();
            if let Some(view) = views.get_mut(&channel) {
                view.push(&notice(NoticeLevel::Info, said), strings);
            }
        }
        Event::Stream { channel, live } => {
            ui.live.borrow_mut().insert(channel.clone(), live);
            if !live {
                ui.stats.borrow_mut().remove(&channel);
            }
            show_live(ui, &channel);
            show_stats(ui);
        }
        Event::StreamStats { channel, stats } => {
            ui.stats.borrow_mut().insert(channel.clone(), stats);
            ui.viewers
                .borrow_mut()
                .entry(channel.clone())
                .or_default()
                .record(Timestamp::now(), &stats);
            show_stats(ui);
            refresh_graph(ui, &channel);
        }
        Event::Chatters { channel, logins } => {
            if let Some(view) = ui.views.borrow_mut().get_mut(&channel) {
                view.set_present(logins);
            }
        }
        Event::ChatterJoined { channel, login } => {
            if let Some(view) = ui.views.borrow_mut().get_mut(&channel) {
                view.set_here(login, true);
            }
        }
        Event::ChatterParted { channel, login } => {
            if let Some(view) = ui.views.borrow_mut().get_mut(&channel) {
                view.set_here(login, false);
            }
        }
        // The corner of the window changing is not enough: a command
        // typed into the chat is answered in the chat.
        Event::Backend { name } => *ui.backend.borrow_mut() = name,
        // The window used to say nothing at all about the connection:
        // the chat simply stopped. The terminal has said it all along.
        Event::Connection { state } => {
            let strings = ui.language.get().strings();
            let (text, level) = match &state {
                liveside_proto::ConnectionState::Disconnected => {
                    (strings.conn_lost.to_owned(), NoticeLevel::Warning)
                }
                liveside_proto::ConnectionState::Connecting => {
                    (strings.conn_trying.to_owned(), NoticeLevel::Info)
                }
                liveside_proto::ConnectionState::Connected => (
                    fill(strings.conn_backend, &[("backend", &ui.backend.borrow())]),
                    NoticeLevel::Info,
                ),
                liveside_proto::ConnectionState::Reconnecting { attempt, reason } => (
                    fill(
                        strings.conn_retry,
                        &[("attempt", &attempt.to_string()), ("reason", reason)],
                    ),
                    NoticeLevel::Warning,
                ),
            };
            say_status(ui, level, text);
        }
        Event::LoggedIn { account } => {
            let text = ui.language.get().strings();
            let said = fill(text.logged_in_as, &[("user", &account.user.display_name)]);
            show_account(ui, Some(&account));
            say_status(ui, NoticeLevel::Info, said);
        }
        Event::LoggedOut => {
            let text = ui.language.get().strings();
            show_account(ui, None);
            say_status(ui, NoticeLevel::Info, text.logged_out.to_owned());
        }
        Event::LoginFailed { reason } => {
            let text = ui.language.get().strings();
            let said = fill(text.login_failed, &[("reason", &reason)]);
            say_status(ui, NoticeLevel::Error, said);
        }
        Event::LoginPending { uri, code, .. } => {
            let text = ui.language.get().strings();
            let said = fill(text.login_pending, &[("uri", &uri), ("code", &code)]);
            say_status(ui, NoticeLevel::Warning, said);
            // Clicked, it opens as any link in chat does, after the
            // dialog that shows where it goes.
            ui.status_view.borrow().link_in_last_line(&uri);
        }
        Event::Log {
            level,
            text,
            code,
            args,
        } => {
            let said = liveside_client::lang::from_engine(
                ui.language.get().strings(),
                &code,
                &args,
                &text,
            );
            say(ui, level, said);
        }
    }
}

impl Ui {
    /// The text view of the channel on screen.
    fn visible_view(&self) -> Option<gtk::TextView> {
        if let Some(login) = self.active_whisper.borrow().as_ref() {
            return self
                .whispers
                .borrow()
                .get(login)
                .map(|chat| chat.view().clone());
        }
        let active = self.active.borrow().as_ref().map(|(id, _)| id.clone())?;
        self.views
            .borrow()
            .get(&active)
            .map(|chat| chat.view().clone())
    }

    fn to_clipboard(&self, text: &str) {
        if let Some(window) = self.window.upgrade() {
            window.clipboard().set_text(text);
        }
    }
}

/// Draws the people active in the channel on screen, if the list is out.
fn refresh_users(ui: &Rc<Ui>) {
    let mut users = ui.users.borrow_mut();
    if !users.visible() {
        return;
    }
    let active = ui.active.borrow().as_ref().map(|(id, _)| id.clone());
    let chatters = active
        .and_then(|id| ui.views.borrow().get(&id).map(ChatView::chatters))
        .unwrap_or_default();
    users.show(chatters, ui.language.get().strings());
}

/// Puts a line into the channel on screen, the way the terminal frontend
/// does: with a time, in colour, wrapped, and scrolling away with the rest.
/// The status line then keeps to the account and the connection.
/// Handles `/allow` and `/deny` in a channel: the queue is the window's,
/// not the engine's command language. Answers whether it took the line.
fn decide_held(ui: &Rc<Ui>, channel: &UserId, text: &str) -> bool {
    let (name, args) = match text.trim().strip_prefix('/') {
        Some(rest) => match rest.split_once(char::is_whitespace) {
            Some((name, args)) => (name.to_ascii_lowercase(), args.trim()),
            None => (rest.to_ascii_lowercase(), ""),
        },
        None => return false,
    };
    let (allow, unban) = match name.as_str() {
        "allow" => (true, false),
        "deny" => (false, false),
        "approve" => (true, true),
        "refuse" => (false, true),
        _ => return false,
    };
    let strings = ui.language.get().strings();
    let wanted = args.trim_start_matches('@').to_lowercase();
    if unban {
        let found = ui
            .unban_requests
            .borrow()
            .iter()
            .find(|(where_, _, user)| {
                where_ == channel && (wanted.is_empty() || user.to_lowercase() == wanted)
            })
            .map(|(_, id, _)| id.clone());
        match found {
            Some(request) => ui.engine.send(Command::ResolveUnban {
                channel: channel.clone(),
                request,
                allow,
            }),
            None => say_to(
                ui,
                Some(channel),
                NoticeLevel::Error,
                strings.nothing_requested.to_owned(),
            ),
        }
        return true;
    }
    let found = ui
        .held
        .borrow()
        .iter()
        .find(|(where_, _, user)| {
            where_ == channel && (wanted.is_empty() || user.to_lowercase() == wanted)
        })
        .map(|(_, id, _)| id.clone());
    match found {
        Some(message) => ui.engine.send(Command::ResolveHeld {
            channel: channel.clone(),
            message,
            allow,
        }),
        None => say_to(
            ui,
            Some(channel),
            NoticeLevel::Error,
            strings.nothing_held.to_owned(),
        ),
    }
    true
}

/// A line the frontend wrote itself, already in the reader's language.
fn notice(level: NoticeLevel, text: String) -> Line {
    Line {
        time: Timestamp::now(),
        content: LineContent::Notice {
            level,
            text,
            code: String::new(),
            args: Vec::new(),
        },
        flags: LineFlags::default(),
    }
}

/// Says something in one named channel, wherever the reader is looking.
/// A poll's result belongs in the channel it ran in, not in whatever is
/// on screen when it finishes.
fn say_in(ui: &Rc<Ui>, channel: &UserId, text: String) {
    let line = Line {
        time: Timestamp::now(),
        content: LineContent::Notice {
            level: NoticeLevel::Info,
            text,
            code: String::new(),
            args: Vec::new(),
        },
        flags: LineFlags::default(),
    };
    if let Some(view) = ui.views.borrow_mut().get_mut(channel) {
        view.push(&line, ui.language.get().strings());
    }
}

/// What the message line invites you to do: write to the channel on
/// screen, whisper to the person whose thread is open, or log in first.
/// It used to invite writing even when nothing could be sent, and the
/// refusal came only once the line had been typed and sent.
fn refresh_placeholder(ui: &Rc<Ui>) {
    let strings = ui.language.get().strings();
    if !ui.logged_in.get() {
        ui.entry.set_placeholder_text(Some(strings.send_logged_out));
        return;
    }
    let whisper = ui.active_whisper.borrow().clone();
    if let Some(login) = whisper {
        ui.entry.set_placeholder_text(Some(&fill(
            strings.whisper_send,
            &[("user", login.as_str())],
        )));
        return;
    }
    let active = ui.active.borrow().clone();
    match active {
        Some((_, login)) => ui.entry.set_placeholder_text(Some(&fill(
            strings.send_placeholder,
            &[("channel", &format!("#{login}"))],
        ))),
        None => ui
            .entry
            .set_placeholder_text(Some(strings.not_in_channel_logged_in)),
    }
}

/// A notice written here rather than by the engine, so already in the
/// reader's language.
fn notice_line(level: NoticeLevel, text: String) -> Line {
    Line {
        time: Timestamp::now(),
        content: LineContent::Notice {
            level,
            text,
            code: String::new(),
            args: Vec::new(),
        },
        flags: LineFlags::default(),
    }
}

/// Says something on the status page, whatever is on screen. The
/// connection and the login belong to the client rather than to a
/// channel, and said into a busy chat they were gone in seconds: the
/// line about the socket being replaced, and worse, the code to type
/// into Twitch to log in.
fn say_status(ui: &Rc<Ui>, level: NoticeLevel, text: String) {
    let line = notice_line(level, text);
    ui.status_view
        .borrow_mut()
        .push(&line, ui.language.get().strings());
    // Moving these off the channel would hide them altogether if the
    // list did not say that something had arrived.
    if ui.stack.visible_child_name().as_deref() != Some(STATUS_PAGE) {
        let strings = ui.language.get().strings();
        ui.status_label.set_markup(&format!(
            "<b>{}</b> •",
            glib::markup_escape_text(strings.status_buffer)
        ));
    }
}

/// The bots' window, while it is open.
fn bots_window(ui: &Ui) -> Option<Rc<crate::bots::BotsWindow>> {
    ui.bots_window
        .borrow()
        .as_ref()
        .filter(|window| window.is_open())
        .cloned()
}

/// The whole list of bots, as the engine reports it.
fn show_bots(ui: &Rc<Ui>, bots: Vec<liveside_proto::model::BotStatus>) {
    *ui.bots.borrow_mut() = bots.clone();
    if let Some(window) = bots_window(ui) {
        window.show(bots);
    }
}

/// Opens the bots' window, or brings it up. Starting and stopping go to
/// the engine; a changed list is saved with the settings first, and goes
/// to the engine only once it is.
fn open_bots(ui: &Rc<Ui>) {
    if let Some(window) = bots_window(ui) {
        window.present();
        return;
    }
    let Some(main) = ui.window.upgrade() else {
        return;
    };
    let act: crate::bots::Acts = Rc::new({
        let ui = Rc::downgrade(ui);
        move |act| {
            let Some(ui) = ui.upgrade() else {
                return Ok(());
            };
            match act {
                crate::bots::Act::Start(name) => ui.engine.send(Command::StartBot { name }),
                crate::bots::Act::Stop(name) => ui.engine.send(Command::StopBot { name }),
                crate::bots::Act::Save(list) => {
                    let command = ui.settings.borrow_mut().save_bots(list)?;
                    ui.engine.send(command);
                }
            }
            Ok(())
        }
    });
    let window = crate::bots::BotsWindow::open(
        &main,
        ui.language.get().strings(),
        act,
        ui.bots.borrow().clone(),
    );
    *ui.bots_window.borrow_mut() = Some(window);
}

fn say(ui: &Rc<Ui>, level: NoticeLevel, text: String) {
    let active = ui.active.borrow().as_ref().map(|(id, _)| id.clone());
    say_to(ui, active.as_ref(), level, text);
}

/// Puts a line into a channel's view, wherever that is shown: the main
/// window or a window of its own. No channel is the status page.
fn say_to(ui: &Rc<Ui>, channel: Option<&UserId>, level: NoticeLevel, text: String) {
    let line = notice_line(level, text);
    let mut views = ui.views.borrow_mut();
    match channel.and_then(|id| views.get_mut(id)) {
        Some(view) => view.push(&line, ui.language.get().strings()),
        // No channel on screen: it belongs on the status page, whole and
        // readable, rather than cut short in the status line.
        None => ui
            .status_view
            .borrow_mut()
            .push(&line, ui.language.get().strings()),
    }
}

/// Whether the modifier that goes with commands is held: Ctrl, and on a
/// Mac also Cmd, which GTK reports as Meta there and which is what a Mac
/// user presses for Find. A hotkey written with `ctrl` takes either.
fn primary(modifiers: gtk::gdk::ModifierType) -> bool {
    use gtk::gdk::ModifierType;
    let mut mask = ModifierType::CONTROL_MASK;
    if cfg!(target_os = "macos") {
        mask |= ModifierType::META_MASK;
    }
    modifiers.intersects(mask)
}

/// Runs the hotkey a key is bound to, if any. Whether it was one.
fn run_hotkey(ui: &Rc<Ui>, key: gtk::gdk::Key, modifiers: gtk::gdk::ModifierType) -> bool {
    use gtk::gdk::{Key, ModifierType};
    use liveside_client::hotkeys::{Key as Hotkey, KeyCode};
    let function = [
        Key::F1,
        Key::F2,
        Key::F3,
        Key::F4,
        Key::F5,
        Key::F6,
        Key::F7,
        Key::F8,
        Key::F9,
        Key::F10,
        Key::F11,
        Key::F12,
    ]
    .iter()
    .position(|f| *f == key);
    let code = match (function, key.to_lower().to_unicode()) {
        (Some(i), _) => KeyCode::F(i as u8 + 1),
        (None, Some(c)) if c.is_ascii_alphanumeric() => KeyCode::Char(c.to_ascii_lowercase()),
        _ => return false,
    };
    let pressed = Hotkey {
        ctrl: primary(modifiers),
        alt: modifiers.contains(ModifierType::ALT_MASK),
        shift: modifiers.contains(ModifierType::SHIFT_MASK),
        code,
    };
    if pressed.ctrl && matches!(code, KeyCode::Char('c' | 'q')) {
        return false;
    }
    let hotkeys = ui.settings.borrow().hotkeys();
    let Some(hotkey) = hotkeys.find(&pressed) else {
        return false;
    };
    // `$1`: whoever wrote the message under the pointer, as the message
    // menu acts on it.
    let selected = ui
        .clicked
        .borrow()
        .as_ref()
        .map(|(_, login)| login.to_string());
    let place = Place::main(ui);
    match hotkey.run(
        selected.as_deref(),
        &command_context(ui, place.channel.as_ref()),
    ) {
        Ok(line) if line.is_empty() => {}
        Ok(line) => submit_line(ui, &place, line),
        Err(problem) => {
            let said = liveside_client::lang::custom_problem(ui.language.get().strings(), &problem);
            say(ui, NoticeLevel::Error, said);
        }
    }
    true
}

/// Where a line is typed: the main window's message line, on whatever it
/// shows, or the line of a channel popped out into a window of its own.
#[derive(Debug, Clone)]
struct Place {
    channel: Option<(UserId, Login)>,
    whisper: Option<Login>,
    /// The main window, whose reply in progress goes with the line.
    main: bool,
}

impl Place {
    fn main(ui: &Ui) -> Place {
        Place {
            channel: ui.active.borrow().clone(),
            whisper: ui.active_whisper.borrow().clone(),
            main: true,
        }
    }

    fn channel_id(&self) -> Option<&UserId> {
        self.channel.as_ref().map(|(id, _)| id)
    }
}

/// Says something where a line was typed: its channel, its whisper
/// thread, or the status page.
fn say_at(ui: &Rc<Ui>, place: &Place, level: NoticeLevel, text: String) {
    if place.channel.is_none()
        && let Some(login) = &place.whisper
        && let Some(view) = ui.whispers.borrow_mut().get_mut(login)
    {
        view.push(&notice_line(level, text), ui.language.get().strings());
        return;
    }
    say_to(ui, place.channel_id(), level, text);
}

/// A line as if typed: what Enter does with the input line, and what a
/// hotkey does with what it makes.
fn submit_line(ui: &Rc<Ui>, place: &Place, text: String) {
    // Nothing would carry it; Up brings it back once the daemon is.
    if ui.engine.is_lost() {
        let said = ui.language.get().strings().engine_lost.to_owned();
        say_at(ui, place, NoticeLevel::Error, said);
        return;
    }
    // A custom command first: what it makes goes on as if typed,
    // and is not expanded again.
    let commands = ui.settings.borrow().custom_commands();
    let text = match liveside_client::custom::expand(
        &commands,
        &text,
        &command_context(ui, place.channel.as_ref()),
        ui.language.get().strings(),
    ) {
        liveside_client::custom::Typed::Unchanged => text,
        liveside_client::custom::Typed::Replaced(line) if line.is_empty() => {
            return;
        }
        liveside_client::custom::Typed::Replaced(line) => line,
        // Cleared as after any line; Up brings it back to finish.
        liveside_client::custom::Typed::Refused(why) => {
            say_at(ui, place, NoticeLevel::Error, why);
            return;
        }
    };
    // A whisper goes to a person rather than to a channel, so it
    // is answered here whatever is on screen.
    if let Some(rest) = whisper_command(&text) {
        match rest {
            Some((who, said)) => ui.engine.send(Command::SendWhisper {
                user: who.into(),
                text: said,
            }),
            None => say_at(
                ui,
                place,
                NoticeLevel::Error,
                ui.language.get().strings().whisper_usage.to_owned(),
            ),
        }
        return;
    }
    // In a thread, what is typed is whispered to that person; a command
    // is still a command, as in the terminal, and `//` sends a line that
    // starts with `/`.
    if let Some(login) = place.whisper.clone() {
        let said = match text.strip_prefix("//") {
            Some(rest) => Some(format!("/{rest}")),
            None if text.starts_with('/') => None,
            None => Some(text.clone()),
        };
        if let Some(said) = said {
            ui.engine.send(Command::SendWhisper {
                user: login,
                text: said,
            });
            return;
        }
    }
    // Answered by the window itself, in or out of a channel, the
    // way the terminal answers it from its own state.
    if text.trim().eq_ignore_ascii_case("/privacy") {
        if let Some(window) = ui.window.upgrade() {
            crate::privacy::open(
                &window,
                &ui.data_flows.borrow(),
                ui.language.get().strings(),
            );
        }
        return;
    }
    // The lists of what lit up and what was hidden, and ignoring
    // somebody: the window's own, wherever it is typed.
    if collected_command(ui, place, &text) {
        return;
    }
    let Some((channel, _)) = place.channel.clone() else {
        // No channel: only what belongs to the account itself can
        // be done from here, as in the terminal frontend.
        account_command(ui, &text);
        return;
    };
    // Someone who sends a message wants to see it, wherever they
    // were reading.
    if let Some(view) = ui.views.borrow().get(&channel) {
        view.jump_to_bottom();
    }
    // What AutoMod caught can be decided by typing, as well as by
    // the buttons in the line: a queue is often worked through
    // from the keyboard.
    if decide_held(ui, &channel, &text) {
        return;
    }
    // A reply is started in the main window and goes with its line only.
    let reply_to = match place.main {
        true => {
            ui.reply_label.set_visible(false);
            ui.replying_to.borrow_mut().take().map(|(id, _)| id)
        }
        false => None,
    };
    ui.engine.send(Command::Input {
        channel,
        text,
        reply_to,
    });
}

/// What a custom command run here may name: the channel on screen and
/// its stream, the others joined, the account.
fn command_context(
    ui: &Rc<Ui>,
    active: Option<&(UserId, Login)>,
) -> liveside_client::commands::Context {
    let active = active.cloned();
    let mut channels: Vec<String> = ui
        .channel_labels
        .borrow()
        .values()
        .map(|(_, login)| login.to_string())
        .collect();
    channels.sort();
    let mut context = liveside_client::commands::Context {
        channel: active.as_ref().map(|(_, login)| login.to_string()),
        channels,
        me: ui.me.borrow().clone(),
        ..Default::default()
    };
    if let Some((id, _)) = &active {
        let live = ui.live.borrow().get(id).copied().unwrap_or(false);
        let stats = ui.stats.borrow().get(id).copied().filter(|_| live);
        liveside_client::custom::stream_context(
            &mut context,
            stats.as_ref(),
            ui.viewers.borrow().get(id),
            Timestamp::now(),
        );
    }
    context
}

/// The channel's open Stream Chat window, if it has one.
fn stream_chat(ui: &Rc<Ui>, channel: &UserId) -> Option<Rc<crate::stream_chat::StreamChat>> {
    ui.stream_chats
        .borrow()
        .get(channel)
        .filter(|chat| chat.is_open())
        .cloned()
}

/// Opens a channel's Stream Chat window, or brings it up. It starts empty:
/// a stream shows what is said from now on.
fn open_stream_chat(ui: &Rc<Ui>, id: UserId, login: Login) {
    if let Some(chat) = stream_chat(ui, &id) {
        chat.present();
        return;
    }
    let Some(parent) = ui.window.upgrade() else {
        return;
    };
    let settings = ui.settings.borrow().config().stream_chat.clone();
    let chat = crate::stream_chat::StreamChat::open(
        &parent,
        login.as_str(),
        &settings,
        ui.emotes.clone(),
        ui.language.get().strings(),
    );
    ui.stream_chats.borrow_mut().insert(id, chat);
}

/// A channel taken out of the main window's stack.
#[derive(Clone)]
enum Detached {
    /// In a window of its own.
    Window(gtk::Window),
    /// Docked beside the chat on screen, in `pane`.
    Docked { pane: gtk::Box, entry: gtk::Entry },
}

impl Detached {
    /// Brings it before the reader: its window up, or its line focused.
    fn show(&self) {
        match self {
            Detached::Window(window) => window.present(),
            Detached::Docked { entry, .. } => {
                entry.grab_focus();
            }
        }
    }

    /// What the main window says where the channel was.
    fn here(&self, strings: &'static Strings) -> &'static str {
        match self {
            Detached::Window(_) => strings.popout_here,
            Detached::Docked { .. } => strings.dock_here,
        }
    }
}

fn pop_out(ui: &Rc<Ui>, id: UserId, login: Login) {
    detach(ui, id, login, false, true);
}

fn dock(ui: &Rc<Ui>, id: UserId, login: Login) {
    detach(ui, id, login, true, true);
}

/// Puts a channel just joined where it was when the program closed:
/// docked, or in a window of its own.
fn restore_detached(ui: &Rc<Ui>, id: &UserId, login: &Login) {
    let (docked, window) = {
        let layout = ui.layout.borrow();
        (
            layout.is_docked(login.as_str()),
            layout.is_window(login.as_str()),
        )
    };
    if docked || window {
        detach(ui, id.clone(), login.clone(), docked, false);
    }
}

/// Moves a channel's chat out of the stack, with a line to type into of
/// its own: into a window, or docked beside the chat on screen. Moved, not
/// copied, so it keeps everything it has and goes on taking lines; it
/// comes back when its window closes or its pane is closed. `focus` says
/// whether its line takes the keyboard: not for a pane put back where it
/// was when the program last closed.
fn detach(ui: &Rc<Ui>, id: UserId, login: Login, docked: bool, focus: bool) {
    if let Some(detached) = ui.popouts.borrow().get(&id) {
        detached.show();
        return;
    }
    let Some(root) = ui.views.borrow().get(&id).map(|view| view.root().clone()) else {
        return;
    };
    let Some(main) = ui.window.upgrade() else {
        return;
    };
    let strings = ui.language.get().strings();
    ui.stack.remove(&root);

    let entry = gtk::Entry::builder()
        .placeholder_text(fill(
            strings.send_placeholder,
            &[("channel", &format!("#{login}"))],
        ))
        .margin_start(6)
        .margin_end(6)
        .margin_top(6)
        .margin_bottom(6)
        .build();
    // The stack stretched it in the main window; here it has to ask.
    root.set_vexpand(true);
    let column = gtk::Box::new(gtk::Orientation::Vertical, 0);
    column.append(&root);
    column.append(&entry);
    let place = Place {
        channel: Some((id.clone(), login.clone())),
        whisper: None,
        main: false,
    };
    entry.connect_activate({
        let ui = ui.clone();
        move |entry| {
            let text = entry.text().to_string();
            if text.trim().is_empty() {
                return;
            }
            ui.history.borrow_mut().remember(&text);
            entry.set_text("");
            // Someone who sends a message wants to see it.
            if let Some(id) = place.channel_id()
                && let Some(view) = ui.views.borrow().get(id)
            {
                view.jump_to_bottom();
            }
            submit_line(&ui, &place, text);
        }
    });

    let detached = if docked {
        // A name over it and a way to put it back; the rest is as in a
        // window.
        let name = gtk::Label::builder()
            .label(format!("#{login}"))
            .xalign(0.0)
            .hexpand(true)
            .css_classes(["heading"])
            .margin_start(8)
            .build();
        let close = gtk::Button::builder()
            .icon_name("window-close-symbolic")
            .tooltip_text(strings.dock_close)
            .css_classes(["flat"])
            .build();
        let header = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        header.append(&name);
        header.append(&close);
        let pane = gtk::Box::new(gtk::Orientation::Vertical, 0);
        pane.append(&header);
        pane.append(&column);
        close.connect_clicked({
            let (ui, id) = (ui.clone(), id.clone());
            move |_| bring_back(&ui, &id)
        });
        // In its place among the others, and remembered for the next
        // start.
        let fresh = ui.layout.borrow_mut().dock(login.as_str());
        let at = {
            let docked = ui.docked.borrow();
            let shown: Vec<&str> = docked.iter().map(|(_, login)| login.as_str()).collect();
            ui.layout.borrow().place(login.as_str(), &shown)
        };
        ui.docked
            .borrow_mut()
            .insert(at, (id.clone(), login.clone()));
        if fresh {
            write_layout(ui);
        }
        Detached::Docked {
            pane,
            entry: entry.clone(),
        }
    } else {
        // A window of the application's own, not one held above the main
        // window: it is for another screen as much as for beside this one.
        // As big as it was when the program last closed, and remembered
        // for the next start.
        let (width, height) = ui
            .layout
            .borrow()
            .window_size(login.as_str())
            .unwrap_or((480, 640));
        if ui.layout.borrow_mut().open_window(login.as_str()) {
            write_layout(ui);
        }
        let window = gtk::ApplicationWindow::builder()
            .title(fill(strings.popout_title, &[("channel", login.as_str())]))
            .default_width(width as i32)
            .default_height(height as i32)
            .child(&column)
            .build();
        if let Some(application) = main.application() {
            window.set_application(Some(&application));
        }
        window.connect_close_request({
            let (ui, id) = (ui.clone(), id.clone());
            move |_| {
                put_back(&ui, &id, false);
                glib::Propagation::Proceed
            }
        });
        Detached::Window(window.upcast())
    };
    ui.popouts.borrow_mut().insert(id.clone(), detached.clone());
    if docked {
        arrange_docked(ui);
    }
    // Moved, it is at the newest line, as a chat one opens is.
    if let Some(view) = ui.views.borrow().get(&id) {
        view.jump_to_bottom();
    }
    if ui
        .active
        .borrow()
        .as_ref()
        .is_some_and(|(active, _)| active == &id)
    {
        ui.popped_show.set_visible(!docked);
        ui.popped_label.set_text(&fill(
            detached.here(strings),
            &[("channel", login.as_str())],
        ));
        ui.stack.set_visible_child_name(POPPED_PAGE);
    }
    show_stats(ui);
    if let Detached::Window(window) = &detached {
        window.present();
    }
    if focus {
        entry.grab_focus();
    }
}

/// Puts a detached channel's chat back in the stack, and closes its
/// window or removes its pane.
fn bring_back(ui: &Rc<Ui>, id: &UserId) {
    put_back(ui, id, true);
}

/// The same, with the window left alone: what its own close request
/// does, since that window is already closing.
fn put_back(ui: &Rc<Ui>, id: &UserId, close_window: bool) {
    let Some(detached) = ui.popouts.borrow_mut().remove(id) else {
        return;
    };
    let Some(root) = ui.views.borrow().get(id).map(|view| view.root().clone()) else {
        return;
    };
    match &detached {
        // Closed from the main window's page, the window goes too, and its
        // close request finds nothing left to bring back.
        Detached::Window(window) => {
            if let Some(column) = root.parent().and_downcast::<gtk::Box>() {
                column.remove(&root);
            }
            if close_window {
                window.close();
            }
            forget_window(ui, id);
        }
        Detached::Docked { .. } => {
            if let Some(column) = root.parent().and_downcast::<gtk::Box>() {
                column.remove(&root);
            }
            undock(ui, id);
        }
    }
    ui.stack.add_named(&root, Some(id.as_str()));
    if let Some(view) = ui.views.borrow().get(id) {
        view.jump_to_bottom();
    }
    // On screen in the main window again, if it is the one there.
    if ui
        .active
        .borrow()
        .as_ref()
        .is_some_and(|(active, _)| active == id)
    {
        ui.stack.set_visible_child_name(id.as_str());
    }
    show_stats(ui);
}

/// Opens the viewer graph of the channel on screen, or brings it up.
fn open_graph(ui: &Rc<Ui>, channel: Option<(UserId, Login)>) {
    let Some((channel, login)) = channel else {
        say(
            ui,
            NoticeLevel::Error,
            ui.language.get().strings().not_in_channel.to_owned(),
        );
        return;
    };
    let open = ui
        .graph_window
        .borrow()
        .as_ref()
        .filter(|window| window.is_open() && window.channel() == &channel)
        .cloned();
    if let Some(window) = open {
        window.present();
        return;
    }
    let Some(parent) = ui.window.upgrade() else {
        return;
    };
    let history = ui
        .viewers
        .borrow()
        .get(&channel)
        .cloned()
        .unwrap_or_default();
    let window = crate::graph::GraphWindow::open(
        &parent,
        channel,
        login.as_str(),
        history,
        ui.palette.get(),
        ui.language.get().strings(),
    );
    *ui.graph_window.borrow_mut() = Some(window);
}

/// Shows a channel's history in its graph, if that is open.
fn refresh_graph(ui: &Rc<Ui>, channel: &UserId) {
    let open = ui
        .graph_window
        .borrow()
        .as_ref()
        .filter(|window| window.is_open() && window.channel() == channel)
        .cloned();
    if let (Some(window), Some(history)) = (open, ui.viewers.borrow().get(channel)) {
        window.update(history);
    }
}

/// Keeps a line in the list of what lit up or was hidden, and shows it in
/// the window if that is open.
fn collect(ui: &Rc<Ui>, kind: crate::collected::Kind, channel: &UserId, line: &Line) {
    let Some(login) = ui
        .channel_labels
        .borrow()
        .get(channel)
        .map(|(_, login)| login.clone())
    else {
        return;
    };
    let channel = liveside_proto::model::Channel {
        id: channel.clone(),
        display_name: login.to_string(),
        login,
    };
    let kept = match kind {
        crate::collected::Kind::Highlighted => ui.collected.borrow_mut().line(&channel, line),
        crate::collected::Kind::Ignored => {
            ui.collected.borrow_mut().ignored(&channel, line);
            true
        }
    };
    if !kept {
        return;
    }
    let open = ui
        .collected_window
        .borrow()
        .as_ref()
        .filter(|window| window.is_open())
        .cloned();
    if let Some(window) = open {
        window.push(
            kind,
            &liveside_proto::model::CollectedLine {
                channel,
                line: line.clone(),
            },
        );
    }
}

/// Opens the window of what lit up and what was hidden at a page, or
/// turns the open one to it.
fn open_collected(ui: &Rc<Ui>, kind: crate::collected::Kind) {
    let open = ui
        .collected_window
        .borrow()
        .as_ref()
        .filter(|window| window.is_open())
        .cloned();
    if let Some(window) = open {
        window.show(kind);
        return;
    }
    let Some(parent) = ui.window.upgrade() else {
        return;
    };
    let window = crate::collected::CollectedWindow::open(
        &parent,
        &ui.collected.borrow(),
        kind,
        ui.emotes.clone(),
        ui.palette.get(),
        ui.language.get().strings(),
    );
    *ui.collected_window.borrow_mut() = Some(window);
}

/// `/highlights`, `/ignored`, `/ignore name`, `/unignore name` and `/ab`,
/// typed anywhere. Answers whether it took the line.
fn collected_command(ui: &Rc<Ui>, place: &Place, text: &str) -> bool {
    let Some(rest) = text.trim().strip_prefix('/') else {
        return false;
    };
    let (name, args) = match rest.split_once(char::is_whitespace) {
        Some((name, args)) => (name.to_ascii_lowercase(), args.trim()),
        None => (rest.to_ascii_lowercase(), ""),
    };
    match name.as_str() {
        "highlights" | "highlighted" => open_collected(ui, crate::collected::Kind::Highlighted),
        "ignored" => open_collected(ui, crate::collected::Kind::Ignored),
        "ignore" => ignore_command(ui, place.channel_id(), true, args),
        "unignore" => ignore_command(ui, place.channel_id(), false, args),
        // How many watched the stream where it is typed, over time.
        "viewers" => open_graph(ui, place.channel.clone()),
        // The channel's messages alone, for a stream to show.
        "streamchat" => match place.channel.clone() {
            Some((id, login)) => open_stream_chat(ui, id, login),
            None => say_to(
                ui,
                None,
                NoticeLevel::Error,
                ui.language.get().strings().not_in_channel.to_owned(),
            ),
        },
        // Popped out into a window of its own.
        "popout" => match place.channel.clone() {
            Some((id, login)) => pop_out(ui, id, login),
            None => say_to(
                ui,
                None,
                NoticeLevel::Error,
                ui.language.get().strings().not_in_channel.to_owned(),
            ),
        },
        // Said here and to nobody else, as Chatty's /echo is.
        "echo" => {
            if !args.is_empty() {
                say_at(ui, place, NoticeLevel::Info, args.to_owned());
            }
        }
        // The address book, which rules and /join [category] use.
        "ab" => {
            let text = ui.language.get().strings();
            let answer = {
                let mut settings = ui.settings.borrow_mut();
                liveside_client::addressbook::command(&mut settings, args, text)
            };
            if answer.changed {
                ui.engine.send(ui.settings.borrow().rules_command());
            }
            say_at(ui, place, answer.level, answer.said);
        }
        _ => return false,
    }
    true
}

/// Ignores somebody, or stops: a rule in the settings, saved at once and
/// handed to the engine. Said where the reader is.
fn ignore_command(ui: &Rc<Ui>, to: Option<&UserId>, ignoring: bool, who: &str) {
    let text = ui.language.get().strings();
    let who = who.trim().trim_start_matches('@').to_lowercase();
    if who.is_empty() || who.contains(char::is_whitespace) {
        let usage = if ignoring {
            text.ignore_usage
        } else {
            text.unignore_usage
        };
        say_to(ui, to, NoticeLevel::Error, usage.to_owned());
        return;
    }
    let done = {
        let mut settings = ui.settings.borrow_mut();
        if ignoring {
            settings.ignore_user(&who)
        } else {
            settings.unignore_user(&who)
        }
    };
    let (level, said) = match (done, ignoring) {
        (Err(e), _) => (
            NoticeLevel::Error,
            fill(text.ignore_failed, &[("error", &e)]),
        ),
        (Ok(true), true) => (
            NoticeLevel::Info,
            fill(text.ignored_user, &[("user", &who)]),
        ),
        (Ok(false), true) => (
            NoticeLevel::Info,
            fill(text.ignored_already, &[("user", &who)]),
        ),
        (Ok(true), false) => (
            NoticeLevel::Info,
            fill(text.unignored_user, &[("user", &who)]),
        ),
        (Ok(false), false) => (
            NoticeLevel::Error,
            fill(text.unignored_none, &[("user", &who)]),
        ),
    };
    if level == NoticeLevel::Info {
        ui.engine.send(ui.settings.borrow().rules_command());
    }
    say_to(ui, to, level, said);
}

/// The line typed on the status page, where there is no channel to send
/// it to. Only the account's own commands make sense here; anything else
/// is answered with what can be done instead.
fn account_command(ui: &Rc<Ui>, text: &str) {
    let strings = ui.language.get().strings();
    let (name, args) = match text.trim().strip_prefix('/') {
        Some(rest) => match rest.split_once(char::is_whitespace) {
            Some((name, args)) => (name.to_ascii_lowercase(), args.trim()),
            None => (rest.to_ascii_lowercase(), ""),
        },
        None => (String::new(), ""),
    };
    match name.as_str() {
        "login" => ui.engine.send(Command::Login),
        "logout" => ui.engine.send(Command::Logout),
        // `[category #]` stays whole; the engine expands it.
        "join" if !args.is_empty() => {
            for channel in liveside_proto::join_targets(args) {
                ui.engine.send(Command::Join {
                    channel: channel.as_str().into(),
                });
            }
        }
        _ => say(ui, NoticeLevel::Error, strings.not_in_channel.to_owned()),
    }
}

/// A channel's name with a mark for whether it is streaming: filled when
/// it is, hollow when it is not, so the two are told apart by shape as
/// well as by colour.
fn channel_label(login: &Login, live: bool, palette: &crate::theme::Palette) -> String {
    let (mark, colour) = if live {
        ("●", palette.badge)
    } else {
        ("○", palette.faded)
    };
    format!(
        "<span foreground=\"{colour}\">{mark}</span> #{}",
        glib::markup_escape_text(login.as_str())
    )
}

/// Redraws one channel's row from what is known about it.
fn show_live(ui: &Rc<Ui>, channel: &UserId) {
    let live = ui.live.borrow().get(channel).copied().unwrap_or(false);
    let Some((label, login)) = ui.channel_labels.borrow().get(channel).cloned() else {
        return;
    };
    let palette = ui.palette.get();
    if ui.stale.borrow().contains(channel) {
        label.set_markup(&stale_label(&login, palette));
    } else {
        label.set_markup(&channel_label(&login, live, palette));
    }
}

/// A channel nothing is known about while the daemon is away: all of
/// it faded, the mark too, as no colour would be true.
fn stale_label(login: &Login, palette: &crate::theme::Palette) -> String {
    format!(
        "<span foreground=\"{}\">○ #{}</span>",
        palette.faded,
        glib::markup_escape_text(login.as_str())
    )
}

/// The daemon went away. The window keeps what was read, greys every
/// channel and says the engine thread is trying to reach it again,
/// rather than a chat that has stopped and still looks alive, as a tester
/// on a Mac saw it.
fn lose_engine(ui: &Rc<Ui>) {
    ui.engine.lose();
    let channels: Vec<UserId> = ui.channel_labels.borrow().keys().cloned().collect();
    {
        let mut live = ui.live.borrow_mut();
        let mut stale = ui.stale.borrow_mut();
        for channel in &channels {
            live.remove(channel);
            stale.insert(channel.clone());
        }
    }
    for channel in &channels {
        show_live(ui, channel);
    }
    let text = ui.language.get().strings().engine_lost.to_owned();
    say(ui, NoticeLevel::Error, text.clone());
    if ui.active.borrow().is_some() {
        say_status(ui, NoticeLevel::Error, text);
    }
}

/// Joins again what the window still shows and a daemon back from the
/// dead does not have: it starts with no channels, and the reader had
/// not left them.
fn rejoin(ui: &Rc<Ui>) {
    let logins: Vec<Login> = {
        let labels = ui.channel_labels.borrow();
        ui.stale
            .borrow()
            .iter()
            .filter_map(|id| labels.get(id).map(|(_, login)| login.clone()))
            .collect()
    };
    for login in logins {
        ui.engine.send(Command::Join {
            channel: login.as_str().into(),
        });
    }
}

fn show_account(ui: &Rc<Ui>, account: Option<&liveside_proto::Account>) {
    *ui.me.borrow_mut() = account.map(|a| a.user.login.to_string());
    let strings = ui.language.get().strings();
    let text = match account {
        Some(account) => account.user.display_name.clone(),
        None => strings.not_logged_in.to_owned(),
    };
    ui.status.set_text(&text);
    ui.logged_in.set(account.is_some());
    refresh_placeholder(ui);
    let said = if account.is_some() {
        strings.logout_button
    } else {
        strings.login_button
    };
    // Its words are a label of their own, which set_label would replace
    // with one that is not cut short.
    if let Some(words) = ui.login_button.child().and_downcast::<gtk::Label>() {
        words.set_text(said);
    }
    ui.login_button.set_tooltip_text(Some(said));
}

/// Whether what is selected lies inside a single message, which is what
/// the moderation items act on. No selection at all counts as inside.
fn selection_is_within_one_message(ui: &Rc<Ui>, channel: &UserId, view: &gtk::TextView) -> bool {
    let buffer = view.buffer();
    let Some((start, end)) = buffer.selection_bounds() else {
        return true;
    };
    let views = ui.views.borrow();
    let Some(chat) = views.get(channel) else {
        return false;
    };
    match (chat.message_at(&start), chat.message_at(&end)) {
        (Some((first, _)), Some((last, _))) => first == last,
        _ => false,
    }
}

/// Adds a channel to the list and the stack, if it is not there yet.
/// Puts the moderation menu back together for the channel on screen,
/// which is where the Shield Mode item learns which way it goes.
fn refresh_room_menu(ui: &Rc<Ui>) {
    let Some(button) = ui.moderate_menu.borrow().clone() else {
        return;
    };
    let on = ui
        .active
        .borrow()
        .as_ref()
        .and_then(|(id, _)| ui.shield.borrow().get(id).copied())
        .unwrap_or(false);
    let text = ui.language.get().strings();
    button.set_menu_model(Some(&crate::moderation::room_menu(text, on)));
}

/// `/w name message`, as a command the window answers itself. `None`
/// means it was not that command at all; `Some(None)` that it was, but
/// without somebody to whisper or something to say.
#[allow(clippy::type_complexity)]
fn whisper_command(text: &str) -> Option<Option<(String, String)>> {
    let rest = text.trim().strip_prefix('/')?;
    let (name, args) = match rest.split_once(char::is_whitespace) {
        Some((name, args)) => (name.to_ascii_lowercase(), args.trim()),
        None => (rest.to_ascii_lowercase(), ""),
    };
    if name != "w" && name != "whisper" {
        return None;
    }
    let Some((who, said)) = args.split_once(char::is_whitespace) else {
        return Some(None);
    };
    let who = who.trim_start_matches('@').to_ascii_lowercase();
    if who.is_empty() || said.trim().is_empty() {
        return Some(None);
    }
    Some(Some((who, said.trim().to_owned())))
}

/// The page and the row for a whisper thread, made once.
fn ensure_whisper(ui: &Rc<Ui>, user: &liveside_proto::model::User) {
    if ui.whispers.borrow().contains_key(&user.login) {
        return;
    }
    let mut view = ChatView::new(ui.scrollback, ui.emotes.clone(), ui.palette.get());
    watch_links(ui, &mut view);
    // A whisper is not moderated, so its menu has what a link and a
    // selection offer and nothing else — and never the view's own Cut,
    // Paste and Undo, which can do nothing in text that cannot be edited.
    let click = gtk::GestureClick::new();
    click.set_button(gtk::gdk::BUTTON_SECONDARY);
    click.set_propagation_phase(gtk::PropagationPhase::Capture);
    click.connect_pressed({
        let ui = ui.clone();
        let text_view = view.view().clone();
        move |gesture, _, x, y| {
            gesture.set_state(gtk::EventSequenceState::Claimed);
            let (bx, by) =
                text_view.window_to_buffer_coords(gtk::TextWindowType::Widget, x as i32, y as i32);
            let link = text_view
                .iter_at_location(bx, by)
                .and_then(|iter| crate::chat::link_at(&iter));
            *ui.clicked.borrow_mut() = None;
            let Some(window) = ui.window.upgrade() else {
                return;
            };
            let menu = crate::moderation::message_menu(
                &window,
                ui.clone(),
                crate::moderation::MenuFor {
                    has_selection: text_view.buffer().selection_bounds().is_some(),
                    message: None,
                    one_message: false,
                    link,
                },
            );
            let Some(popover) = menu else {
                return;
            };
            popover.set_parent(&text_view);
            popover.set_pointing_to(Some(&gtk::gdk::Rectangle::new(x as i32, y as i32, 1, 1)));
            popover.connect_closed(|popover| popover.unparent());
            popover.popup();
        }
    });
    view.view().add_controller(click);
    ui.stack
        .add_named(view.root(), Some(&format!("w:{}", user.login)));

    let row = gtk::ListBoxRow::new();
    row.set_child(Some(
        &gtk::Label::builder()
            .label(format!("@{}", user.login))
            .xalign(0.0)
            .margin_start(6)
            .margin_end(6)
            .margin_top(4)
            .margin_bottom(4)
            .build(),
    ));
    unsafe {
        row.set_data("whisper", user.login.clone());
    }
    ui.channels.append(&row);
    ui.whispers.borrow_mut().insert(user.login.clone(), view);
}

/// A row for a channel that has been asked for. It carries the name at
/// once, and a turning wheel where the live mark will be, so that waiting
/// is told apart from a channel that is simply off the air.
fn show_joining(ui: &Rc<Ui>, login: &Login) {
    if ui.joining.borrow().contains_key(login) {
        return;
    }
    // Joined again after the daemon came back: its row is there already,
    // greyed until the join is done.
    if ui
        .channel_labels
        .borrow()
        .values()
        .any(|(_, known)| known == login)
    {
        return;
    }
    let palette = ui.palette.get();
    let label = gtk::Label::builder()
        .use_markup(true)
        .label(joining_label(login, palette))
        .xalign(0.0)
        .build();
    let wheel = gtk::Spinner::builder()
        .width_request(12)
        .height_request(12)
        .build();
    wheel.start();
    let line = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(6)
        .margin_start(6)
        .margin_end(6)
        .margin_top(4)
        .margin_bottom(4)
        .build();
    line.append(&wheel);
    line.append(&label);
    let row = gtk::ListBoxRow::new();
    row.set_child(Some(&line));
    // Not selectable: there is nothing behind it to show yet.
    row.set_selectable(false);
    ui.channels.append(&row);
    ui.joining.borrow_mut().insert(login.clone(), (row, label));
}

/// Drops the row for a channel that will not be joined after all.
fn drop_joining(ui: &Rc<Ui>, login: &Login) {
    if let Some((row, _)) = ui.joining.borrow_mut().remove(login) {
        ui.channels.remove(&row);
    }
    // One fewer channel to wait for before the list can be opened.
    select_first_ready(ui);
}

/// The name alone, faded: the wheel beside it carries the state.
fn joining_label(login: &Login, palette: &crate::theme::Palette) -> String {
    format!(
        "<span foreground=\"{}\">#{}</span>",
        palette.faded,
        glib::markup_escape_text(login.as_str())
    )
}

fn ensure_channel(ui: &Rc<Ui>, channel: &liveside_proto::model::Channel) {
    if ui.views.borrow().contains_key(&channel.id) {
        return;
    }
    let mut view = ChatView::new(ui.scrollback, ui.emotes.clone(), ui.palette.get());
    watch_links(ui, &mut view);
    // The menu acts on the message under the pointer, so the view notes
    // which one that is before GTK opens its own menu.
    let click = gtk::GestureClick::new();
    click.set_button(gtk::gdk::BUTTON_SECONDARY);
    click.set_propagation_phase(gtk::PropagationPhase::Capture);
    click.connect_pressed({
        let ui = ui.clone();
        let id = channel.id.clone();
        let text_view = view.view().clone();
        move |gesture, _, x, y| {
            // Taking the click here stops the view from opening its own
            // menu, whose Cut, Paste, Undo and Redo can never do anything
            // in a chat that cannot be edited.
            gesture.set_state(gtk::EventSequenceState::Claimed);
            let (bx, by) =
                text_view.window_to_buffer_coords(gtk::TextWindowType::Widget, x as i32, y as i32);
            let found = text_view.iter_at_location(bx, by).and_then(|iter| {
                ui.views
                    .borrow()
                    .get(&id)
                    .and_then(|chat| chat.message_at(&iter))
            });
            // Moderating acts on one message. With several selected, the
            // menu would act on something other than what is selected, so
            // those items are not offered at all.
            let one_message =
                found.is_some() && selection_is_within_one_message(&ui, &id, &text_view);
            let has_selection = text_view.buffer().selection_bounds().is_some();
            *ui.clicked.borrow_mut() = found.clone();
            let link = text_view
                .iter_at_location(bx, by)
                .and_then(|iter| crate::chat::link_at(&iter));

            let Some(window) = ui.window.upgrade() else {
                return;
            };
            let menu = crate::moderation::message_menu(
                &window,
                ui.clone(),
                crate::moderation::MenuFor {
                    has_selection,
                    message: found.as_ref().map(|(_, user)| user),
                    one_message,
                    link,
                },
            );
            let Some(popover) = menu else {
                return;
            };
            popover.set_parent(&text_view);
            popover.set_pointing_to(Some(&gtk::gdk::Rectangle::new(x as i32, y as i32, 1, 1)));
            // The popover belongs to the view only while it is open.
            popover.connect_closed(|popover| popover.unparent());
            popover.popup();
        }
    });
    view.view().add_controller(click);
    // A plain click marks the message too, for a hotkey's `$1`; it is not
    // taken, so selecting text with the mouse works as before.
    let pick = gtk::GestureClick::new();
    pick.set_button(gtk::gdk::BUTTON_PRIMARY);
    pick.connect_pressed({
        let ui = ui.clone();
        let id = channel.id.clone();
        let text_view = view.view().clone();
        move |_, _, x, y| {
            let (bx, by) =
                text_view.window_to_buffer_coords(gtk::TextWindowType::Widget, x as i32, y as i32);
            let found = text_view.iter_at_location(bx, by).and_then(|iter| {
                ui.views
                    .borrow()
                    .get(&id)
                    .and_then(|chat| chat.message_at(&iter))
            });
            *ui.clicked.borrow_mut() = found;
        }
    });
    view.view().add_controller(pick);

    ui.stack.add_named(view.root(), Some(channel.id.as_str()));

    // The row that was waiting for this channel becomes it, so the list
    // does not jump about as four channels arrive one by one.
    if let Some((row, label)) = ui.joining.borrow_mut().remove(&channel.login) {
        label.set_markup(&channel_label(&channel.login, false, ui.palette.get()));
        label.set_margin_start(6);
        label.set_margin_end(6);
        label.set_margin_top(4);
        label.set_margin_bottom(4);
        // The wheel has done its work; the row becomes an ordinary one.
        // The label has to leave the box before it can be the row's child.
        if let Some(line) = label.parent().and_downcast::<gtk::Box>() {
            line.remove(&label);
        }
        row.set_child(Some(&label));
        row.set_selectable(true);
        ui.channel_labels
            .borrow_mut()
            .insert(channel.id.clone(), (label, channel.login.clone()));
        unsafe {
            row.set_data("channel", (channel.id.clone(), channel.login.clone()));
        }
        ui.views.borrow_mut().insert(channel.id.clone(), view);
        select_first_ready(ui);
        restore_detached(ui, &channel.id, &channel.login);
        open_if_asked(ui, &channel.login);
        return;
    }

    let row = gtk::ListBoxRow::new();
    let label = gtk::Label::builder()
        .use_markup(true)
        .label(channel_label(&channel.login, false, ui.palette.get()))
        .xalign(0.0)
        .margin_start(6)
        .margin_end(6)
        .margin_top(4)
        .margin_bottom(4)
        .build();
    ui.channel_labels
        .borrow_mut()
        .insert(channel.id.clone(), (label.clone(), channel.login.clone()));
    row.set_child(Some(&label));
    unsafe {
        row.set_data("channel", (channel.id.clone(), channel.login.clone()));
    }
    ui.channels.append(&row);
    ui.views.borrow_mut().insert(channel.id.clone(), view);
    select_first_ready(ui);
    restore_detached(ui, &channel.id, &channel.login);
    open_if_asked(ui, &channel.login);
}

/// Opens a channel just joined if a click on a notification asked for it.
fn open_if_asked(ui: &Rc<Ui>, login: &Login) {
    let asked = ui.open_when_joined.borrow().as_ref() == Some(login);
    if asked {
        *ui.open_when_joined.borrow_mut() = None;
        select_listed(ui, |channel, _| channel.is_some_and(|(_, l)| l == login));
    }
}

/// Selects the first row of the channel list that `wanted` picks, by the
/// channel or the whisper partner it belongs to. Whether one was found.
fn select_listed(
    ui: &Rc<Ui>,
    wanted: impl Fn(Option<&(UserId, Login)>, Option<&Login>) -> bool,
) -> bool {
    let mut row = ui.channels.first_child();
    while let Some(widget) = row {
        row = widget.next_sibling();
        let Ok(listed) = widget.downcast::<gtk::ListBoxRow>() else {
            continue;
        };
        if !listed.is_selectable() {
            continue;
        }
        // Set on every row when it is made, with these very types.
        let channel = unsafe { listed.data::<(UserId, Login)>("channel") }
            .map(|data| unsafe { data.as_ref() });
        let whisper =
            unsafe { listed.data::<Login>("whisper") }.map(|data| unsafe { data.as_ref() });
        if wanted(channel, whisper) {
            ui.channels.select_row(Some(&listed));
            return true;
        }
    }
    false
}

/// Lays the window out from the right for a language written right to
/// left (Arabic), and from the left otherwise. Every widget that has not
/// chosen a direction of its own follows at once; chat lines take theirs
/// from their own text, so a Latin nickname in Arabic chat still reads
/// left to right.
fn set_direction(text: &liveside_client::lang::Strings) {
    gtk::Widget::set_default_direction(if text.rtl {
        gtk::TextDirection::Rtl
    } else {
        gtk::TextDirection::Ltr
    });
}

/// Goes where a clicked notification happened: the channel or the
/// whisper thread, a followed channel joined first, or the list of
/// followed channels for several of them. The window comes to the front,
/// as far as the desktop lets it: the click's activation token, when the
/// desktop gave one, is what lets it past a guard against focus stealing.
fn go_to(ui: &Rc<Ui>, place: liveside_client::notify::Place, token: Option<String>) {
    use liveside_client::notify::Place;
    // A mention or a whisper is clicked to answer it: the keys go to the
    // message line. The list of followed channels has nothing to write.
    let to_write = !matches!(place, Place::Followed(None));
    match place {
        Place::Channel(id) => {
            select_listed(ui, |channel, _| channel.is_some_and(|(i, _)| i == &id));
        }
        Place::Whisper(login) => {
            select_listed(ui, |_, whisper| whisper == Some(&login));
        }
        Place::Followed(Some(login)) => {
            let joined = select_listed(ui, |channel, _| channel.is_some_and(|(_, l)| l == &login));
            if !joined {
                *ui.open_when_joined.borrow_mut() = Some(login.clone());
                ui.engine.send(Command::Join { channel: login });
            }
        }
        Place::Followed(None) => ui.engine.send(Command::ShowFollowedChannels),
    }
    if let Some(window) = ui.window.upgrade() {
        if let Some(token) = token.or_else(|| x11_now(&window)) {
            window.set_startup_id(&token);
        }
        window.present();
    }
    if to_write {
        // Taking the focus selects the line; a draft is kept, not replaced.
        ui.entry.grab_focus();
        ui.entry.set_position(-1);
    }
}

/// On X11, a startup ID that only carries the X server's present time,
/// which GTK takes as the moment of the user's click and presents the
/// window with. Without it GTK presents with the time this window was
/// last used, older than the window in front, and xfwm4 raised the window
/// but left the keyboard where it was (seen on Xfce 4.20). Notification
/// services send no activation token on X11. `None` elsewhere.
fn x11_now(window: &gtk::ApplicationWindow) -> Option<String> {
    #[cfg(target_os = "linux")]
    {
        use gtk::prelude::Cast;
        let surface = window.surface()?.downcast::<gdk_x11::X11Surface>().ok()?;
        Some(format!("_TIME{}", gdk_x11::x11_get_server_time(&surface)))
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = window;
        None
    }
}

/// Shows or hides the wheel beside the status, according to whether the
/// engine still owes an answer. Called on every answer and from a timer,
/// since asking is what starts the waiting and that happens all over the
/// window.
fn show_working(ui: &Rc<Ui>) {
    let waiting = ui.engine.is_waiting();
    if waiting == ui.working.is_visible() {
        return;
    }
    ui.working.set_visible(waiting);
    if waiting {
        ui.working.start();
    } else {
        ui.working.stop();
    }
}

/// Takes a channel that is no longer in a window of its own out of the
/// layout kept for the next start.
fn forget_window(ui: &Ui, id: &UserId) {
    let login = ui
        .channel_labels
        .borrow()
        .get(id)
        .map(|(_, login)| login.clone());
    if let Some(login) = login
        && ui.layout.borrow_mut().close_window(login.as_str())
    {
        write_layout(ui);
    }
}

/// Takes a docked channel's pane off the screen, and out of the layout
/// kept for the next start.
fn undock(ui: &Rc<Ui>, id: &UserId) {
    let login = {
        let mut docked = ui.docked.borrow_mut();
        let at = docked.iter().position(|(docked, _)| docked == id);
        at.map(|at| docked.remove(at).1)
    };
    arrange_docked(ui);
    if let Some(login) = login
        && ui.layout.borrow_mut().undock(login.as_str())
    {
        write_layout(ui);
    }
}

/// Lays out the chat on screen and the channels docked beside it: each
/// pane is the end of a divider whose start is everything on its left, so
/// a drag moves only the pane right of the divider, and a wider window
/// widens the chat rather than the panes. Built anew whenever a pane comes
/// or goes, which is rare enough not to keep the dividers in step one by
/// one.
fn arrange_docked(ui: &Rc<Ui>) {
    // Everything out of the old dividers first: a widget has one parent.
    let mut next = ui.chats.first_child();
    if let Some(top) = &next {
        ui.chats.remove(top);
    }
    while let Some(paned) = next.and_downcast::<gtk::Paned>() {
        next = paned.start_child();
        paned.set_start_child(None::<&gtk::Widget>);
        paned.set_end_child(None::<&gtk::Widget>);
    }
    let docked = ui.docked.borrow().clone();
    let shown = docked.len();
    let mut left: gtk::Widget = ui.stack.clone().upcast();
    for (id, login) in docked {
        let pane = match ui.popouts.borrow().get(&id) {
            Some(Detached::Docked { pane, .. }) => pane.clone(),
            _ => continue,
        };
        let paned = gtk::Paned::builder()
            .orientation(gtk::Orientation::Horizontal)
            .start_child(&left)
            .end_child(&pane)
            .resize_end_child(false)
            .shrink_start_child(false)
            .shrink_end_child(false)
            .build();
        place_docked(ui, &paned, login, shown);
        left = paned.upcast();
    }
    // The box gives room only to a child that asks for it.
    left.set_hexpand(true);
    ui.chats.append(&left);
}

/// Gives a docked pane its width once its divider has a width to share
/// out: the one it was left at, or as much as the chat and each of the
/// `shown` panes get. A window too narrow for it squeezes it, and it
/// grows back as room comes. A width the reader drags it to is kept for
/// the next start; one it is squeezed to is not.
fn place_docked(ui: &Rc<Ui>, paned: &gtk::Paned, login: Login, shown: usize) {
    // The pane's width with its handle: what is right of the divider.
    let width = |paned: &gtk::Paned| paned.width() - paned.position();
    // None until the divider first has a width.
    let wanted: Rc<Cell<Option<i32>>> = Rc::default();
    let placed = Rc::new(Cell::new(false));
    // GTK notes a new largest position whenever the divider's width
    // changes, the first allocation included. Moving the divider inside
    // an allocation would only queue another, so it waits for the
    // allocation to end.
    paned.connect_notify_local(Some("max-position"), {
        let (ui, login, wanted, placed) =
            (ui.clone(), login.clone(), wanted.clone(), placed.clone());
        move |paned, _| {
            if paned.width() <= 0 {
                return;
            }
            let goal = wanted.get().unwrap_or_else(|| {
                ui.layout
                    .borrow()
                    .width(login.as_str())
                    .map(|width| width as i32)
                    .unwrap_or_else(|| ui.chats.width() / (shown as i32 + 1))
            });
            wanted.set(Some(goal));
            if placed.get() && width(paned) == goal {
                return;
            }
            let (paned, placed) = (paned.clone(), placed.clone());
            glib::idle_add_local_once(move || {
                // As near as the chat's narrowest lets it.
                paned.set_position(paned.width() - goal);
                placed.set(true);
            });
        }
    });
    // A drag is a press and a release with the divider moved in between;
    // the pointer's state cannot say so on Wayland, so the events are
    // watched on their way to the divider, and none of them is taken.
    let pressed_at: Rc<Cell<Option<i32>>> = Rc::default();
    let watch = gtk::EventControllerLegacy::new();
    watch.set_propagation_phase(gtk::PropagationPhase::Capture);
    watch.connect_event({
        let (ui, paned) = (ui.clone(), paned.downgrade());
        move |_, event| {
            let Some(paned) = paned.upgrade() else {
                return glib::Propagation::Proceed;
            };
            match event.event_type() {
                gtk::gdk::EventType::ButtonPress | gtk::gdk::EventType::TouchBegin => {
                    pressed_at.set(Some(width(&paned)));
                }
                gtk::gdk::EventType::ButtonRelease | gtk::gdk::EventType::TouchEnd => {
                    let now = width(&paned);
                    if placed.get()
                        && pressed_at.take().is_some_and(|before| before != now)
                        && now > 0
                    {
                        wanted.set(Some(now));
                        if ui.layout.borrow_mut().set_width(login.as_str(), now as u32) {
                            write_layout(&ui);
                        }
                    }
                }
                _ => {}
            }
            glib::Propagation::Proceed
        }
    });
    paned.add_controller(watch);
}

/// Writes the layout for the next start.
fn write_layout(ui: &Ui) {
    if let Err(e) = ui.layout.borrow().save(&ui.layout_path) {
        eprintln!("liveside-gui: {}: {e}", ui.layout_path.display());
    }
}

/// Opens the first channel of the list once it is in. Channels are
/// joined side by side and arrive in no fixed order, so opening
/// whichever came back first would open a different channel at every
/// start; this waits for the ones above it to be settled instead.
/// Takes a channel that was left out of the window: its row, its chat
/// (from a window of its own too) and what was known about it. The status
/// page comes up if it was on screen.
fn remove_channel(ui: &Rc<Ui>, id: &UserId) {
    let Some(view) = ui.views.borrow_mut().remove(id) else {
        return;
    };
    // Destroyed, not closed: closing would bring the chat back.
    match ui.popouts.borrow_mut().remove(id) {
        Some(Detached::Window(window)) => {
            window.destroy();
            forget_window(ui, id);
        }
        Some(Detached::Docked { .. }) => undock(ui, id),
        None => ui.stack.remove(view.root()),
    }
    if let Some((label, _)) = ui.channel_labels.borrow_mut().remove(id)
        && let Some(row) = label.parent()
    {
        ui.channels.remove(&row);
    }
    if let Some(chat) = ui.stream_chats.borrow_mut().remove(id) {
        chat.close();
    }
    ui.live.borrow_mut().remove(id);
    ui.stale.borrow_mut().remove(id);
    ui.rejoined.borrow_mut().remove(id);
    ui.stats.borrow_mut().remove(id);
    ui.viewers.borrow_mut().remove(id);
    ui.modes.borrow_mut().remove(id);
    ui.shield.borrow_mut().remove(id);
    ui.polls.borrow_mut().remove(id);
    ui.predictions.borrow_mut().remove(id);
    let was_active = ui
        .active
        .borrow()
        .as_ref()
        .is_some_and(|(active, _)| active == id);
    if was_active {
        *ui.active.borrow_mut() = None;
        if let Some(status) = ui.channels.row_at_index(0) {
            ui.channels.select_row(Some(&status));
        }
    }
}

fn select_first_ready(ui: &Rc<Ui>) {
    if ui.active.borrow().is_some() {
        return;
    }
    let mut row = ui.channels.first_child();
    while let Some(widget) = row {
        row = widget.next_sibling();
        let Ok(listed) = widget.downcast::<gtk::ListBoxRow>() else {
            continue;
        };
        // The status buffer and the whisper rows are not channels.
        if !listed.is_selectable() {
            // A channel still being joined: whether it or one below it
            // is opened is not decided yet.
            return;
        }
        let is_channel = unsafe { listed.data::<(UserId, Login)>("channel").is_some() };
        if is_channel {
            ui.channels.select_row(Some(&listed));
            return;
        }
    }
}

/// One step of Tab completion in the input line. With nothing typed
/// before the cursor, Tab keeps its usual meaning and moves the focus.
/// Commands the window answers itself, for completion; keep in step with
/// `submit_line` and `collected_command`.
const OWN_COMMANDS: &[&str] = &[
    "popout",
    "streamchat",
    "w",
    "whisper",
    "privacy",
    "highlights",
    "ignored",
    "ignore",
    "unignore",
    "viewers",
    "echo",
    "ab",
    "allow",
    "deny",
    "approve",
    "refuse",
];

fn complete(ui: &Ui, direction: isize) -> glib::Propagation {
    let names =
        liveside_client::command_names(OWN_COMMANDS, &ui.settings.borrow().custom_commands());
    let names: Vec<&str> = names.iter().map(String::as_str).collect();
    let text = ui.entry.text().to_string();
    // GTK counts the cursor in characters, the completion in bytes.
    let chars = ui.entry.position().max(0) as usize;
    let cursor = text
        .char_indices()
        .nth(chars)
        .map_or(text.len(), |(i, _)| i);
    let step = {
        let channel = ui.active.borrow().as_ref().map(|(id, _)| id.clone());
        let whisper = ui.active_whisper.borrow().clone();
        let views = ui.views.borrow();
        let whispers = ui.whispers.borrow();
        let status = ui.status_view.borrow();
        let view = match (&whisper, &channel) {
            (Some(login), _) => whispers.get(login),
            (None, Some(id)) => views.get(id),
            (None, None) => None,
        }
        .unwrap_or(&status);
        ui.completion
            .borrow_mut()
            .step(&text, cursor, direction, &names, |prefix| {
                view.completions(prefix)
            })
    };
    match step {
        Step::Changed(done) => {
            ui.entry.set_text(&done.text);
            ui.entry
                .set_position(done.text[..done.cursor].chars().count() as i32);
            glib::Propagation::Stop
        }
        // A word that matches nothing is left alone, and the focus stays.
        Step::NoMatch => glib::Propagation::Stop,
        Step::Nothing => glib::Propagation::Proceed,
    }
}

/// Ctrl-R: starts a reply to the newest message in the channel on screen,
/// or moves a reply already started one message back.
fn reply_by_key(ui: &Rc<Ui>) {
    let strings = ui.language.get().strings();
    let Some((channel, _)) = ui.active.borrow().clone() else {
        say(
            ui,
            NoticeLevel::Error,
            strings.reply_needs_channel.to_owned(),
        );
        return;
    };
    let current = ui.replying_to.borrow().as_ref().map(|(id, _)| id.clone());
    let target = ui
        .views
        .borrow()
        .get(&channel)
        .and_then(|view| view.reply_target(current.as_ref()));
    match target {
        Some((message, user)) => {
            crate::moderation::Moderate::start_reply(ui.as_ref(), message, user)
        }
        None => say(
            ui,
            NoticeLevel::Error,
            strings.nothing_to_reply_to.to_owned(),
        ),
    }
}

/// Makes the links in a chat view answer a click, as the settings say,
/// and shows a hand over them while they can be clicked.
fn watch_links(ui: &Rc<Ui>, view: &mut ChatView) {
    view.set_links(ui.link_mode() != LinkMode::Off);
    view.set_reports(ui.link_reports.clone());
    let text_view = view.view().clone();
    let at = {
        let text_view = text_view.clone();
        move |x: f64, y: f64| {
            let (bx, by) =
                text_view.window_to_buffer_coords(gtk::TextWindowType::Widget, x as i32, y as i32);
            text_view
                .iter_at_location(bx, by)
                .and_then(|iter| crate::chat::link_at(&iter))
        }
    };

    let click = gtk::GestureClick::new();
    click.set_button(gtk::gdk::BUTTON_PRIMARY);
    click.connect_released({
        let ui = ui.clone();
        let at = at.clone();
        let text_view = text_view.clone();
        move |_, presses, x, y| {
            // A drag that selected text, or a double click that picked a
            // word, is reading, not opening.
            if presses != 1 || text_view.buffer().has_selection() {
                return;
            }
            if let Some((text, url)) = at(x, y) {
                follow_link(&ui, &text, &url);
            }
        }
    });
    text_view.add_controller(click);

    let motion = gtk::EventControllerMotion::new();
    motion.connect_motion({
        let ui = ui.clone();
        let text_view = text_view.clone();
        move |_, x, y| {
            let hand = ui.link_mode() != LinkMode::Off && at(x, y).is_some();
            text_view.set_cursor_from_name(Some(if hand { "pointer" } else { "text" }));
        }
    });
    text_view.add_controller(motion);
}

/// Opens a link from chat the way the settings say: after asking, at once,
/// or not at all.
fn follow_link(ui: &Rc<Ui>, text: &str, url: &str) {
    let Some(window) = ui.window.upgrade() else {
        return;
    };
    let strings = ui.language.get().strings();
    let launch = {
        let ui = ui.clone();
        let window = window.clone();
        let url = url.to_owned();
        move || {
            let ui = ui.clone();
            crate::link::launch(&window, &url, move |error| {
                let strings = ui.language.get().strings();
                let said = match error {
                    crate::link::LaunchError::NoBrowser => strings.link_no_browser.to_owned(),
                    crate::link::LaunchError::Other(error) => {
                        fill(strings.link_open_failed, &[("error", &error)])
                    }
                };
                say(&ui, NoticeLevel::Error, said);
            });
        }
    };
    match ui.link_mode() {
        LinkMode::Off => {}
        LinkMode::Open => launch(),
        LinkMode::Ask => crate::link::confirm(&window, text, url, strings, launch),
    }
}

/// Runs `act` on the chat on screen: a whisper thread, a channel, or the
/// status page.
fn with_active_view<R>(ui: &Rc<Ui>, act: impl FnOnce(&mut ChatView) -> R) -> R {
    if let Some(login) = ui.active_whisper.borrow().clone()
        && let Some(view) = ui.whispers.borrow_mut().get_mut(&login)
    {
        return act(view);
    }
    if let Some((id, _)) = ui.active.borrow().clone()
        && let Some(view) = ui.views.borrow_mut().get_mut(&id)
    {
        return act(view);
    }
    act(&mut ui.status_view.borrow_mut())
}

/// Searches the chat on screen for what the bar says, and says how it went.
fn search_now(ui: &Rc<Ui>) {
    let query = ui.search.entry.text().to_string();
    let found = with_active_view(ui, |view| view.search(&query));
    ui.search.show(found, ui.language.get().strings());
}

/// Goes to an older (`-1`) or newer (`1`) match.
fn step_search(ui: &Rc<Ui>, direction: isize) {
    let found = with_active_view(ui, |view| view.step_search(direction));
    ui.search.show(found, ui.language.get().strings());
}

/// Says how the stream on screen stands, or nothing when it is not
/// streaming or the page is not a channel.
fn show_stats(ui: &Rc<Ui>) {
    let active = ui.active.borrow().as_ref().map(|(id, _)| id.clone());
    // Streamlink can be started for any channel; for one that is off it
    // says so itself.
    ui.watch_button.set_visible(active.is_some());
    let said = active.and_then(|id| {
        let live = ui.live.borrow().get(&id).copied().unwrap_or(false);
        let stats = ui.stats.borrow().get(&id).copied();
        stats.filter(|_| live).map(|stats| {
            liveside_client::lang::stream_stats(
                ui.language.get().strings(),
                &stats,
                Timestamp::now(),
            )
        })
    });
    match said {
        Some(said) => {
            if ui.stream_label.text() != said {
                ui.stream_label.set_text(&said);
            }
            ui.stream_label.set_visible(true);
        }
        None => ui.stream_label.set_visible(false),
    }
}

/// Starts Streamlink for the channel on screen, and says in that channel
/// how it went: that it is starting, or why it stopped.
fn start_streamlink(ui: &Rc<Ui>) {
    let Some((channel, login)) = ui.active.borrow().clone() else {
        return;
    };
    let strings = ui.language.get().strings();
    let config = ui.settings.borrow().config().streamlink.clone();
    // What is said about it goes at the bottom, where the eye goes next.
    if let Some(view) = ui.views.borrow().get(&channel) {
        view.jump_to_bottom();
    }
    let (tx, rx) = async_channel::bounded(1);
    let started =
        liveside_client::streamlink::start(&config, login.as_str(), None, move |outcome| {
            let _ = tx.send_blocking(outcome);
        });
    match started {
        Ok(()) => {
            say_in(
                ui,
                &channel,
                fill(
                    strings.streamlink_started,
                    &[("channel", login.as_str()), ("quality", &config.quality)],
                ),
            );
            // It may play for hours; only an ending with an error is news.
            let ui = ui.clone();
            glib::spawn_future_local(async move {
                if let Ok(outcome) = rx.recv().await
                    && let Some(said) = liveside_client::lang::streamlink_ended(
                        ui.language.get().strings(),
                        login.as_str(),
                        &outcome,
                    )
                {
                    say_in(&ui, &channel, said);
                }
            });
        }
        Err(e) => say_in(
            ui,
            &channel,
            fill(
                strings.streamlink_missing,
                &[("command", &config.command), ("error", &e.to_string())],
            ),
        ),
    }
}
