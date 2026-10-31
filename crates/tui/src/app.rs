//! Frontend state: what the terminal shows, and what keys do.
//!
//! Pure logic, no I/O and no terminal: [`App::apply`] folds server messages
//! into the view, [`App::on_key`] turns a key into an optional command to
//! send. That keeps it testable and keeps rendering a function of state.

use std::collections::VecDeque;

use liveside_client::input::{Completion, History, Step};
use liveside_proto::event::{DataFlow, DataFlowKind};
use liveside_proto::model::{
    Fragment, Line, LineContent, LineFlags, NoticeLevel, PollStatus, PredictionStatus, RoomModes,
};
use liveside_proto::{
    Account, Command, ConnectionState, Event, ServerMessage, Snapshot, Timestamp, UserId,
};
use liveside_proto::{Login, MessageId};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use unicode_width::UnicodeWidthStr;

/// Lines kept per buffer. The engine keeps its own, larger scrollback.
const BUFFER_LINES: usize = 1000;

/// One view: the status buffer, a joined channel, or a whisper thread.
pub struct Buffer {
    /// `None` for the status buffer and for a whisper thread.
    pub channel: Option<UserId>,
    /// Who this thread is with, when it is one. A whisper buffer is a
    /// channel without the moderation: what is typed into it is sent to
    /// that person rather than to a channel.
    pub whisper: Option<Login>,
    pub title: String,
    /// Chat restrictions of this channel, shown in the header.
    pub modes: RoomModes,
    /// Whether Shield Mode is on here, as far as the engine knows.
    pub shield: bool,
    /// Set while this is a channel that has been asked for and has not
    /// arrived: the tab says the name and that something is happening.
    pub joining: Option<Login>,
    /// Suspicions that arrived before the message they are about.
    pub pending_suspicion: Vec<(MessageId, liveside_proto::model::Suspicion)>,
    /// The poll running in this channel, if one is: shown on /poll and
    /// counted as the votes come in.
    pub poll: Option<liveside_proto::model::Poll>,
    /// The prediction, which stays after betting closes: that is when
    /// somebody has to decide who was right.
    pub prediction: Option<liveside_proto::model::Prediction>,
    /// Redemptions waiting to be answered, oldest first: /fulfil and
    /// /refund take the number of one of these.
    pub redemptions: Vec<liveside_proto::model::Redemption>,
    pub lines: VecDeque<Line>,
    /// How far the user scrolled back, counted in lines (not in rendered
    /// rows, which depend on the window width). 0 is the bottom.
    pub scroll: usize,
    /// Who is in the channel, when the engine tracks membership over IRC.
    /// Empty when it does not, and the list then shows who has talked.
    pub chatters: std::collections::BTreeSet<Login>,
    /// The message Ctrl-Up/Ctrl-Down walk over, and that Ctrl-R answers.
    /// Held by id, so it survives lines arriving and old ones falling off.
    pub selected: Option<MessageId>,
    /// The message being answered, once Ctrl-R was pressed.
    pub replying_to: Option<ReplyTarget>,
    /// Whether the channel is streaming, as far as the engine knows.
    pub live: bool,
    /// How many watch and since when, while it streams.
    pub stats: Option<liveside_proto::model::StreamStats>,
    /// Messages AutoMod is holding here, oldest first.
    pub held: Vec<liveside_proto::event::HeldMessage>,
    /// People asking to be let back in, oldest first.
    pub unban_requests: Vec<liveside_proto::event::UnbanRequest>,
    /// Set when this lists what lit up or what was hidden, in every
    /// channel, rather than being a channel itself.
    pub collected: Option<CollectedKind>,
    /// How many watched the stream over time, for /viewers.
    pub viewers: liveside_proto::model::ViewerHistory,
}

/// Which list a collected buffer shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CollectedKind {
    Highlighted,
    Ignored,
}

impl Buffer {
    fn new(channel: Option<UserId>, title: String) -> Self {
        Self {
            channel,
            whisper: None,
            title,
            modes: RoomModes::default(),
            shield: false,
            joining: None,
            pending_suspicion: Vec::new(),
            poll: None,
            prediction: None,
            redemptions: Vec::new(),
            lines: VecDeque::new(),
            chatters: std::collections::BTreeSet::new(),
            live: false,
            stats: None,
            held: Vec::new(),
            unban_requests: Vec::new(),
            scroll: 0,
            selected: None,
            replying_to: None,
            collected: None,
            viewers: Default::default(),
        }
    }

    pub(crate) fn push(&mut self, line: Line) {
        // A suspicion that arrived before its message waits here: Twitch
        // sends the two separately and in no fixed order.
        let mut line = line;
        if let LineContent::Message(message) = &line.content
            && let Some(i) = self
                .pending_suspicion
                .iter()
                .position(|(id, _)| id == &message.id)
        {
            let (_, suspicion) = self.pending_suspicion.remove(i);
            line.flags.suspicion = Some(Box::new(suspicion));
        }
        self.lines.push_back(line);
        while self.lines.len() > BUFFER_LINES {
            self.lines.pop_front();
        }
        // Reading back: stay on the same line while new ones arrive below.
        if self.scroll > 0 {
            self.scroll += 1;
        }
        // The buffer is capped, so the oldest lines fall off the other end;
        // one line is the floor, the main loop clamps to the real height.
        self.clamp_scroll(1);
    }

    /// Keeps the view inside the buffer. Without this, scrolling up in a
    /// fast channel runs past the oldest line into an empty screen: every
    /// new message pushes the view one line further back, while the oldest
    /// lines are dropped at the other end.
    ///
    /// `visible` is how many lines the screen shows, so the oldest position
    /// still fills it.
    pub fn clamp_scroll(&mut self, visible: usize) {
        self.scroll = self.scroll.min(self.lines.len().saturating_sub(visible));
    }
}

/// A line this frontend wrote itself, already in the reader's language.
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

/// Someone active in a channel, for the user list. Both frontends build
/// the list the same way, so it lives with the shared client code.
pub use liveside_client::chatters::Chatter;

/// Who is being answered, kept for the prompt and the command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplyTarget {
    pub id: MessageId,
    pub name: String,
}

/// The text being typed, with a cursor position in bytes.
#[derive(Default)]
pub struct Input {
    pub text: String,
    pub cursor: usize,
    /// Lines sent from this frontend, for Up and Down.
    history: History,
    /// State of a running Tab completion, dropped by any other edit.
    completion: Completion,
}

impl Input {
    fn insert(&mut self, c: char) {
        self.text.insert(self.cursor, c);
        self.cursor += c.len_utf8();
    }

    /// Walks back through what was sent.
    fn earlier(&mut self) {
        if let Some(line) = self.history.earlier(&self.text) {
            self.text = line;
            self.cursor = self.text.len();
        }
    }

    /// ... and forward again, ending at the line that was being typed.
    fn later(&mut self) {
        if let Some(line) = self.history.later() {
            self.text = line;
            self.cursor = self.text.len();
        }
    }

    /// Empties the line (Ctrl-U), keeping what was sent before.
    fn clear(&mut self) {
        self.text.clear();
        self.cursor = 0;
        self.history.reset();
    }

    fn backspace(&mut self) {
        if let Some(prev) = self.text[..self.cursor].chars().next_back() {
            self.cursor -= prev.len_utf8();
            self.text.remove(self.cursor);
        }
    }

    fn delete(&mut self) {
        if self.cursor < self.text.len() {
            self.text.remove(self.cursor);
        }
    }

    fn left(&mut self) {
        if let Some(prev) = self.text[..self.cursor].chars().next_back() {
            self.cursor -= prev.len_utf8();
        }
    }

    fn right(&mut self) {
        if let Some(next) = self.text[self.cursor..].chars().next() {
            self.cursor += next.len_utf8();
        }
    }

    /// Deletes the word before the cursor (Ctrl-W).
    fn delete_word(&mut self) {
        let before = &self.text[..self.cursor];
        let trimmed = before.trim_end_matches(' ');
        let start = trimmed.rfind(' ').map_or(0, |i| i + 1);
        self.text.replace_range(start..self.cursor, "");
        self.cursor = start;
    }

    fn take(&mut self) -> String {
        self.cursor = 0;
        self.completion.stop();
        std::mem::take(&mut self.text)
    }
}

/// The frames of the little wheel beside a channel that is being joined.
/// Braille dots, because a terminal has them wherever it has anything.
pub const SPINNER: [char; 8] = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧'];

/// A search in the buffer on screen (Ctrl-F). The input line becomes the
/// query; what matches is marked, and the view goes to it.
#[derive(Debug, Default)]
pub struct Search {
    pub query: String,
    /// The line it is at, by its time and text: the index moves as lines
    /// arrive and old ones are dropped.
    current: Option<(Timestamp, String)>,
    /// Where it is, newest counted first, and how many lines match.
    pub at: Option<(usize, usize)>,
}

pub struct App {
    /// Buffer 0 is the status buffer; the rest are channels in join order.
    pub buffers: Vec<Buffer>,
    pub active: usize,
    pub input: Input,
    /// A search in progress, which takes the keys while it lasts.
    pub search: Option<Search>,
    /// Where a Streamlink that ended badly says why, with the buffer it
    /// was started from: it may be hours later, from another thread.
    pub streamlink_ended:
        Option<tokio::sync::mpsc::UnboundedSender<(String, liveside_client::streamlink::Outcome)>>,
    pub account: Option<Account>,
    /// What the engine says it does with the user's data, shown by
    /// `/privacy`.
    pub data_flows: Vec<DataFlow>,
    /// What is known about links, written after them wherever they are.
    pub link_reports: std::collections::HashMap<
        (String, liveside_proto::model::LinkSource),
        liveside_proto::model::LinkState,
    >,
    /// The backend the engine runs. Anything but `twitch` is marked in the
    /// header: invented chat is indistinguishable from real chat otherwise.
    pub backend: String,
    pub connection: ConnectionState,
    /// Set when the user asked to quit, or the engine refused us.
    pub quit: bool,
    /// Where the engine runs, for the status line.
    pub server: String,
    /// The language this interface speaks.
    pub language: liveside_client::lang::Language,
    /// What the terminal said its background is, when it was asked at
    /// start: light or not, or nothing when it could not say.
    pub terminal_light: Option<bool>,
    /// Whether the user list is shown beside the chat.
    pub show_users: bool,
    /// Which frame of the little wheel to draw. Advanced by the main
    /// loop while anything is waiting, and left alone otherwise, so a
    /// quiet terminal redraws for nothing.
    pub spinner: usize,
    /// What has been asked of the engine and not answered yet, with the
    /// moment of asking: opening a user card or a list of people is a
    /// question to Twitch, and the terminal used to sit still until the
    /// answer came.
    pub waiting: std::collections::HashMap<liveside_proto::RequestId, std::time::Instant>,
    /// The settings screen, and the file behind it.
    pub settings: liveside_client::settings::Settings,
    /// Lines the chat area can show, kept up to date by the main loop. The
    /// scroll limit depends on it: at the very top a screenful must still
    /// be visible, not a single line.
    pub chat_height: usize,
    /// What is worth a notification, decided as events arrive.
    pub notifier: liveside_client::notify::Notifier,
    /// Raises desktop notifications, once the main loop has started it.
    pub desktop: Option<liveside_client::notify::Desktop>,
    /// A channel joined from a notification, to be opened once it is in.
    open_when_joined: Option<Login>,
    /// What lit up and what was hidden, in every channel.
    collected: liveside_client::collected::Collected,
    /// Whether the terminal has the focus. Taken to be true until it says
    /// otherwise, since one that does not report focus never will.
    pub focused: bool,
    /// The bell is to ring once the screen is drawn.
    pub bell: bool,
    /// The channel whose viewer graph is open over the chat.
    pub graph: Option<UserId>,
}

impl Default for App {
    fn default() -> Self {
        Self {
            buffers: vec![Buffer::new(None, String::new())],
            active: 0,
            input: Input::default(),
            search: None,
            streamlink_ended: None,
            account: None,
            data_flows: Vec::new(),
            link_reports: std::collections::HashMap::new(),
            backend: String::new(),
            language: liveside_client::lang::Language::default(),
            terminal_light: None,
            show_users: false,
            spinner: 0,
            waiting: std::collections::HashMap::new(),
            settings: liveside_client::settings::Settings::default(),
            connection: ConnectionState::Disconnected,
            quit: false,
            server: String::new(),
            chat_height: 20,
            notifier: liveside_client::notify::Notifier::new(Default::default()),
            desktop: None,
            open_when_joined: None,
            collected: Default::default(),
            focused: true,
            bell: false,
            graph: None,
        }
    }
}

impl App {
    /// What to say, in the language in use.
    pub fn text(&self) -> &'static liveside_client::lang::Strings {
        self.language.strings()
    }

    pub fn buffer(&self) -> &Buffer {
        &self.buffers[self.active]
    }

    fn buffer_mut(&mut self) -> &mut Buffer {
        &mut self.buffers[self.active]
    }

    pub(crate) fn index_of(&self, channel: &UserId) -> Option<usize> {
        self.buffers
            .iter()
            .position(|b| b.channel.as_ref() == Some(channel))
    }

    /// Adds a line the engine knows nothing about (errors, local help).
    /// What a custom command run here may name: this channel and its
    /// stream, the others joined, the account.
    fn command_context(&self) -> liveside_client::commands::Context {
        let buffer = self.buffer();
        let mut context = liveside_client::commands::Context {
            channel: buffer
                .channel
                .as_ref()
                .map(|_| buffer.title.trim_start_matches('#').to_owned()),
            channels: self
                .buffers
                .iter()
                .filter(|b| b.channel.is_some())
                .map(|b| b.title.trim_start_matches('#').to_owned())
                .collect(),
            me: self.account.as_ref().map(|a| a.user.login.to_string()),
            ..Default::default()
        };
        if buffer.channel.is_some() {
            liveside_client::custom::stream_context(
                &mut context,
                buffer.stats.as_ref().filter(|_| buffer.live),
                Some(&buffer.viewers),
                liveside_proto::Timestamp::now(),
            );
        }
        context
    }

    /// `/ignore name` and `/unignore name`.
    fn ignore_command(&mut self, name: &str, args: &str) -> Vec<Command> {
        use liveside_client::lang::fill;
        let text = self.text();
        let who = args.trim().trim_start_matches('@').to_lowercase();
        let ignoring = name == "ignore";
        if who.is_empty() || who.contains(char::is_whitespace) {
            let usage = if ignoring {
                text.ignore_usage
            } else {
                text.unignore_usage
            };
            self.local(NoticeLevel::Error, usage);
            return Vec::new();
        }
        let done = if ignoring {
            self.settings.ignore_user(&who)
        } else {
            self.settings.unignore_user(&who)
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
        self.local(level, said);
        match level {
            NoticeLevel::Info => vec![self.settings.rules_command()],
            _ => Vec::new(),
        }
    }

    /// A channel's login, with a tick when a buffer has it joined.
    fn marked_if_joined(&self, channel: &liveside_proto::model::Channel) -> String {
        let joined = self.buffers.iter().any(|buffer| {
            buffer.channel.is_some()
                && buffer.title.trim_start_matches('#') == channel.login.as_str()
        });
        match joined {
            true => format!("{} ✓", channel.login),
            false => channel.login.to_string(),
        }
    }

    pub fn local(&mut self, level: NoticeLevel, text: impl Into<String>) {
        let line = Line {
            time: Timestamp::now(),
            content: LineContent::Notice {
                level,
                text: text.into(),
                // Written here, so already in the user's language.
                code: String::new(),
                args: Vec::new(),
            },
            flags: LineFlags::default(),
        };
        self.buffer_mut().push(line);
    }

    pub fn status(&mut self, level: NoticeLevel, text: impl Into<String>) {
        let line = Line {
            time: Timestamp::now(),
            content: LineContent::Notice {
                level,
                text: text.into(),
                // Written here, so already in the user's language.
                code: String::new(),
                args: Vec::new(),
            },
            flags: LineFlags::default(),
        };
        self.buffers[0].push(line);
    }

    pub fn apply(&mut self, message: ServerMessage) {
        let visible = self.chat_height.max(1);
        self.apply_inner(message);
        for buffer in &mut self.buffers {
            buffer.clamp_scroll(visible);
        }
    }

    fn apply_inner(&mut self, message: ServerMessage) {
        match message {
            ServerMessage::Welcome { server, .. } => {
                self.server = server.clone();
                let said =
                    liveside_client::lang::fill(self.text().conn_server, &[("server", &server)]);
                self.status(NoticeLevel::Info, said);
            }
            ServerMessage::Rejected { reason } => {
                let said =
                    liveside_client::lang::fill(self.text().conn_rejected, &[("reason", &reason)]);
                self.status(NoticeLevel::Error, said);
                self.quit = true;
            }
            ServerMessage::Snapshot { snapshot, .. } => self.reset(snapshot),
            ServerMessage::Event { event, .. } => {
                self.notify(&event);
                self.event(event);
            }
            ServerMessage::Response { id, error } => {
                self.waiting.remove(&id);
                if let Some(error) = error {
                    let said = liveside_client::lang::from_engine(
                        self.text(),
                        &error.code,
                        &error.args,
                        &error.message,
                    );
                    self.local(NoticeLevel::Error, said);
                }
            }
        }
    }

    /// Replaces all state with a snapshot, keeping the active buffer and
    /// scroll position where possible. Snapshots arrive on attach and
    /// whenever the engine decides we fell behind.
    fn reset(&mut self, snapshot: Snapshot) {
        let active_channel = self.buffer().channel.clone();
        let active_collected = self.buffer().collected;
        let open_lists: Vec<CollectedKind> =
            self.buffers.iter().filter_map(|b| b.collected).collect();
        self.collected = liveside_client::collected::Collected::from_snapshot(
            snapshot.highlighted,
            snapshot.ignored,
        );
        let mut status = std::mem::take(&mut self.buffers)
            .into_iter()
            .next()
            .unwrap_or_else(|| Buffer::new(None, "status".into()));
        status.scroll = 0;
        self.buffers.push(status);

        self.account = snapshot.account;
        self.connection = snapshot.connection;
        self.data_flows = snapshot.data_flows;
        self.link_reports = snapshot
            .link_reports
            .into_iter()
            .map(|report| ((report.url, report.source), report.state))
            .collect();
        self.backend = snapshot.backend;
        for channel in snapshot.channels {
            let mut buffer = Buffer::new(
                Some(channel.channel.id.clone()),
                format!("#{}", channel.channel.login),
            );
            buffer.modes = channel.modes;
            buffer.chatters = channel.chatters.into_iter().collect();
            buffer.live = channel.live;
            self.notifier.know_channel(&channel.channel, channel.live);
            buffer.stats = channel.stats;
            buffer.viewers = channel.viewers;
            buffer.held = channel.held;
            buffer.unban_requests = channel.unban_requests;
            buffer.lines = channel.lines.into_iter().collect();
            self.buffers.push(buffer);
        }
        // Channels on their way, joined by the engine before this attached:
        // tabs for them as for a join seen happening.
        for login in snapshot.joining {
            self.event(Event::ChannelJoining { login });
        }
        // The lists that were open come back, filled from the snapshot.
        for kind in open_lists {
            self.open_collected(kind);
        }
        self.active = match active_collected {
            Some(kind) => self.collected_buffer(kind),
            None => active_channel.and_then(|id| self.index_of(&id)),
        }
        .unwrap_or(0);
    }

    fn collected_buffer(&self, kind: CollectedKind) -> Option<usize> {
        self.buffers.iter().position(|b| b.collected == Some(kind))
    }

    /// Opens the list of what lit up or of what was hidden, as a buffer of
    /// its own, or goes to it when it is open.
    fn open_collected(&mut self, kind: CollectedKind) {
        if let Some(i) = self.collected_buffer(kind) {
            self.active = i;
            return;
        }
        let text = self.text();
        let (title, list, none) = match kind {
            CollectedKind::Highlighted => (
                text.collected_highlighted,
                &self.collected.highlighted,
                text.collected_highlighted_none,
            ),
            CollectedKind::Ignored => (
                text.collected_ignored,
                &self.collected.ignored,
                text.collected_ignored_none,
            ),
        };
        let mut buffer = Buffer::new(None, format!("!{title}"));
        buffer.collected = Some(kind);
        buffer.lines = list
            .iter()
            .map(|item| liveside_client::collected::shown(item, text))
            .collect();
        if buffer.lines.is_empty() {
            buffer
                .lines
                .push_back(notice_line(NoticeLevel::Info, none.to_owned()));
        }
        self.buffers.push(buffer);
        self.active = self.buffers.len() - 1;
    }

    /// Adds a line to a list, and to its buffer if that is open.
    fn add_collected(&mut self, kind: CollectedKind, channel: &UserId, line: &Line) {
        let Some(i) = self.index_of(channel) else {
            return;
        };
        let login = self.buffers[i].title.trim_start_matches('#').to_owned();
        let channel = liveside_proto::model::Channel {
            id: channel.clone(),
            login: login.as_str().into(),
            display_name: login,
        };
        let kept = match kind {
            CollectedKind::Highlighted => self.collected.line(&channel, line),
            CollectedKind::Ignored => {
                self.collected.ignored(&channel, line);
                true
            }
        };
        if !kept {
            return;
        }
        let text = self.text();
        if let Some(i) = self.collected_buffer(kind) {
            let item = liveside_proto::model::CollectedLine {
                channel,
                line: line.clone(),
            };
            self.buffers[i].push(liveside_client::collected::shown(&item, text));
        }
    }

    /// Rings the bell and raises a desktop notification for what is worth
    /// one, unless it is on screen already.
    fn notify(&mut self, event: &Event) {
        use liveside_client::notify::{Place, Seen};
        let Some(alert) = self.notifier.consider(event, self.text()) else {
            return;
        };
        let buffer = self.buffer();
        let place = match (&buffer.whisper, &buffer.channel) {
            (Some(login), _) => Some(Place::Whisper(login.clone())),
            (None, Some(channel)) => Some(Place::Channel(channel.clone())),
            (None, None) => None,
        };
        let seen = Seen {
            focused: self.focused,
            place,
            beside: Vec::new(),
        };
        if !self.notifier.wanted(&alert, &seen) {
            return;
        }
        if !alert.silent && self.notifier.sound_now(alert.kind) {
            self.bell = true;
        }
        if self.notifier.config().desktop
            && !alert.quiet
            && let Some(desktop) = &self.desktop
        {
            desktop.show(alert, self.notifier.config().close_after);
        }
    }

    fn event(&mut self, event: Event) {
        match event {
            // Listed with /bots and /bot log; only a bot that falls over
            // is said unasked.
            Event::Bots { .. } | Event::BotLog { .. } => {}
            Event::BotState { name, state } => {
                if liveside_client::bots::went_wrong(&state) {
                    let strings = self.text();
                    let said = liveside_client::bots::state_said(strings, &state);
                    let text = liveside_client::lang::fill(
                        strings.bots_said,
                        &[("bot", &name), ("state", &said)],
                    );
                    self.status(liveside_proto::model::NoticeLevel::Warning, text);
                }
            }
            // Drawn after the link on the next frame, wherever it is.
            Event::LinkReport { report } => {
                self.link_reports
                    .insert((report.url, report.source), report.state);
            }
            // What became of the account is said where the command was
            // typed, not only in the status buffer: someone in a channel
            // would see nothing at all and think it had not worked.
            Event::LoginPending { uri, code, .. } => {
                let said = liveside_client::lang::fill(
                    self.text().login_pending,
                    &[("uri", &uri), ("code", &code)],
                );
                self.local(NoticeLevel::Warning, said);
            }
            Event::LoggedIn { account } => {
                let said = liveside_client::lang::fill(
                    self.text().logged_in_as,
                    &[("user", &account.user.display_name)],
                );
                self.local(NoticeLevel::Info, said);
                self.account = Some(account);
            }
            Event::LoginFailed { reason } => {
                let said =
                    liveside_client::lang::fill(self.text().login_failed, &[("reason", &reason)]);
                self.local(NoticeLevel::Error, said);
            }
            Event::LoggedOut => {
                self.account = None;
                let said = self.text().logged_out;
                self.local(NoticeLevel::Info, said);
            }
            Event::Log {
                level,
                text,
                code,
                args,
            } => {
                let said = liveside_client::lang::from_engine(self.text(), &code, &args, &text);
                self.status(level, said);
            }
            Event::DataFlows { flows } => self.data_flows = flows,
            Event::Backend { name } => self.backend = name,
            Event::Connection { state } => {
                let strings = self.text();
                let text = match &state {
                    ConnectionState::Disconnected => strings.conn_lost.to_owned(),
                    ConnectionState::Connecting => strings.conn_trying.to_owned(),
                    // Whatever we are actually talking to: in the demo
                    // this used to claim a connection to Twitch.
                    ConnectionState::Connected => liveside_client::lang::fill(
                        strings.conn_backend,
                        &[("backend", &self.backend)],
                    ),
                    ConnectionState::Reconnecting { attempt, reason } => {
                        liveside_client::lang::fill(
                            strings.conn_retry,
                            &[("attempt", &attempt.to_string()), ("reason", reason)],
                        )
                    }
                };
                self.connection = state;
                self.status(NoticeLevel::Info, text);
            }
            Event::ChannelJoined { channel, modes } => {
                match self.index_of(&channel.id) {
                    Some(i) => self.buffers[i].modes = modes,
                    // The tab that was waiting for it becomes it, so the
                    // tabs do not shuffle as channels arrive one by one.
                    None => match self.joining_buffer(&channel.login) {
                        Some(i) => {
                            self.buffers[i].joining = None;
                            self.buffers[i].channel = Some(channel.id.clone());
                            self.buffers[i].modes = modes;
                        }
                        None => {
                            let mut buffer = Buffer::new(
                                Some(channel.id.clone()),
                                format!("#{}", channel.login),
                            );
                            buffer.modes = modes;
                            self.buffers.push(buffer);
                        }
                    },
                }
                self.open_first_ready();
                // Clicked in a notification: that is where the reader is
                // going, whichever way the tab came about.
                if self.open_when_joined.as_ref() == Some(&channel.login) {
                    self.open_when_joined = None;
                    if let Some(i) = self.index_of(&channel.id) {
                        self.active = i;
                    }
                }
            }
            Event::ChatterJoined { channel, login } => {
                if let Some(i) = self.index_of(&channel) {
                    self.buffers[i].chatters.insert(login);
                }
            }
            Event::ChatterParted { channel, login } => {
                if let Some(i) = self.index_of(&channel) {
                    self.buffers[i].chatters.remove(&login);
                }
            }
            // What AutoMod caught is said in the channel it was caught
            // in, with what to type about it.
            Event::MessageHeld { channel, held } => {
                if let Some(i) = self.index_of(&channel) {
                    let said = liveside_client::lang::fill(
                        self.text().held_message,
                        &[
                            ("user", &held.sender.display_name),
                            ("detail", &held.detail),
                            ("text", &held.text),
                        ],
                    );
                    if !self.buffers[i].held.iter().any(|other| other.id == held.id) {
                        self.buffers[i].held.push(held);
                    }
                    self.buffers[i].push(notice_line(NoticeLevel::Warning, said));
                }
            }
            Event::HeldResolved {
                channel,
                message,
                status,
                ..
            } => {
                if let Some(i) = self.index_of(&channel) {
                    let held = self.buffers[i]
                        .held
                        .iter()
                        .position(|held| held.id == message)
                        .map(|at| self.buffers[i].held.remove(at));
                    let Some(held) = held else { return };
                    let pattern = match status.as_str() {
                        "approved" => self.text().held_approved,
                        "denied" => self.text().held_denied,
                        _ => self.text().held_expired,
                    };
                    let said = liveside_client::lang::fill(
                        pattern,
                        &[("user", &held.sender.display_name)],
                    );
                    self.buffers[i].push(notice_line(NoticeLevel::Info, said));
                }
            }
            // A poll: said when it starts and when it ends, and counted
            // along the way only when asked, since a line per vote would
            // bury the chat it is running in.
            Event::PollUpdate { channel, poll } => {
                let Some(i) = self.index_of(&channel) else {
                    return;
                };
                let text = self.text();
                let said = match poll.status {
                    PollStatus::Active if self.buffers[i].poll.is_none() => Some(
                        liveside_client::lang::fill(text.poll_started, &[("title", &poll.title)]),
                    ),
                    // A count on the way: kept, not said.
                    PollStatus::Active => None,
                    PollStatus::Completed => Some(liveside_client::lang::fill(
                        text.poll_result,
                        &[("choices", &poll_standing(&poll))],
                    )),
                    PollStatus::Terminated => Some(liveside_client::lang::fill(
                        text.poll_stopped,
                        &[("choices", &poll_standing(&poll))],
                    )),
                    PollStatus::Archived => Some(text.poll_hidden.to_owned()),
                };
                self.buffers[i].poll = match poll.status {
                    PollStatus::Active => Some(poll),
                    _ => None,
                };
                if let Some(said) = said {
                    self.buffers[i].push(notice_line(NoticeLevel::Info, said));
                }
            }
            // A prediction. Like a poll, the amounts are kept rather than
            // said; unlike a poll, closing betting is worth a line of its
            // own, because from there somebody has to act.
            Event::PredictionUpdate {
                channel,
                prediction,
            } => {
                let Some(i) = self.index_of(&channel) else {
                    return;
                };
                let text = self.text();
                let said = match prediction.status {
                    PredictionStatus::Active if self.buffers[i].prediction.is_none() => {
                        Some(liveside_client::lang::fill(
                            text.pred_started,
                            &[("title", &prediction.title)],
                        ))
                    }
                    PredictionStatus::Active => None,
                    PredictionStatus::Locked
                        if self.buffers[i]
                            .prediction
                            .as_ref()
                            .is_none_or(|p| p.status != PredictionStatus::Locked) =>
                    {
                        Some(liveside_client::lang::fill(
                            text.pred_locked,
                            &[("outcomes", &prediction_standing(&prediction, text))],
                        ))
                    }
                    PredictionStatus::Locked => None,
                    PredictionStatus::Resolved => {
                        let won = prediction
                            .winner
                            .as_ref()
                            .and_then(|id| prediction.outcomes.iter().find(|o| &o.id == id))
                            .map(|outcome| outcome.title.clone())
                            .unwrap_or_default();
                        Some(liveside_client::lang::fill(
                            text.pred_paid,
                            &[("outcome", &won)],
                        ))
                    }
                    PredictionStatus::Canceled => Some(text.pred_canceled.to_owned()),
                };
                self.buffers[i].prediction = match prediction.status {
                    PredictionStatus::Active | PredictionStatus::Locked => Some(prediction),
                    _ => None,
                };
                if let Some(said) = said {
                    self.buffers[i].push(notice_line(NoticeLevel::Info, said));
                }
            }
            // A channel asked for: a tab at once, so the window says what
            // it is doing rather than standing empty while four channels
            // are looked up one after another.
            Event::ChannelJoining { login } => {
                if self.joining_buffer(&login).is_none()
                    && !self.buffers.iter().any(|b| b.title == format!("#{login}"))
                {
                    let mut buffer = Buffer::new(None, format!("#{login}"));
                    buffer.joining = Some(login);
                    self.buffers.push(buffer);
                }
            }
            // ...and it will not arrive after all.
            Event::ChannelJoinFailed { login } => {
                if let Some(i) = self.joining_buffer(&login) {
                    self.buffers.remove(i);
                    if self.active >= self.buffers.len() {
                        self.active = self.buffers.len().saturating_sub(1);
                    }
                }
                // One fewer to wait for before a channel below it opens.
                self.open_first_ready();
            }
            // Shield Mode, kept so a frontend can show it rather than
            // guess; the line about it comes separately.
            Event::ShieldMode { channel, on } => {
                if let Some(i) = self.index_of(&channel) {
                    self.buffers[i].shield = on;
                }
            }
            // A whisper, in the thread with whoever it is with. The
            // thread is a buffer of its own, made when the first one
            // arrives in either direction.
            Event::Whisper { user, line } => {
                let i = match self.whisper_buffer(&user.login) {
                    Some(i) => i,
                    None => {
                        let mut buffer = Buffer::new(None, format!("@{}", user.login));
                        buffer.whisper = Some(user.login.clone());
                        self.buffers.push(buffer);
                        self.buffers.len() - 1
                    }
                };
                self.buffers[i].push(line);
            }
            // When the stream is next. The terminal says the next few in
            // lines; the window has the list that edits them.
            Event::Schedule { channel, schedule } => {
                let Some(i) = self.index_of(&channel) else {
                    return;
                };
                let text = self.text();
                let mut said = Vec::new();
                if let Some((start, end)) = &schedule.vacation {
                    said.push(liveside_client::lang::fill(
                        text.schedule_away,
                        &[("start", &day_of(start)), ("end", &day_of(end))],
                    ));
                }
                if schedule.segments.is_empty() {
                    said.push(text.schedule_none.to_owned());
                }
                for segment in &schedule.segments {
                    let mut line = liveside_client::lang::fill(
                        text.schedule_line,
                        &[
                            ("start", &when_of(&segment.start)),
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
                        line.push(' ');
                        line.push_str(text.schedule_weekly);
                    }
                    if segment.canceled {
                        line.push(' ');
                        line.push_str(text.schedule_canceled);
                    }
                    said.push(line);
                }
                for line in said {
                    self.buffers[i].push(notice_line(NoticeLevel::Info, line));
                }
            }
            // What the channel says about itself. The terminal says it in
            // a line; the window has the form that edits it.
            Event::ChannelInfo { channel, info } => {
                let Some(i) = self.index_of(&channel) else {
                    return;
                };
                let text = self.text();
                let mut said = vec![match info.title.is_empty() && info.game.is_empty() {
                    true => text.channel_said_nothing.to_owned(),
                    false => liveside_client::lang::fill(
                        text.channel_said,
                        &[("title", &info.title), ("game", &info.game)],
                    ),
                }];
                if !info.tags.is_empty() {
                    said.push(format!("{}: {}", text.channel_tags, info.tags.join(", ")));
                }
                if !info.labels.is_empty() {
                    said.push(format!(
                        "{}: {}",
                        text.channel_labels,
                        info.labels.join(", ")
                    ));
                }
                if info.delay > 0 {
                    said.push(format!("{}: {} s", text.channel_delay, info.delay));
                }
                for line in said {
                    self.buffers[i].push(notice_line(NoticeLevel::Info, line));
                }
            }
            // What the channel offers, asked for with /rewards.
            Event::Rewards { channel, rewards } => {
                let Some(i) = self.index_of(&channel) else {
                    return;
                };
                let text = self.text();
                let said = if rewards.is_empty() {
                    text.reward_none.to_owned()
                } else {
                    let list = rewards
                        .iter()
                        .map(|reward| {
                            // A reward made elsewhere can be seen and not
                            // answered, which is worth saying once rather
                            // than discovering at the button. So is one
                            // nobody can redeem just now.
                            let mut mark = String::new();
                            if !reward.enabled {
                                mark.push(' ');
                                mark.push_str(text.reward_off);
                            } else if reward.paused {
                                mark.push(' ');
                                mark.push_str(text.reward_paused);
                            }
                            if !reward.ours {
                                mark.push(' ');
                                mark.push_str(text.reward_theirs);
                            }
                            format!("{} ({}){mark}", reward.title, reward.cost)
                        })
                        .collect::<Vec<_>>()
                        .join(", ");
                    liveside_client::lang::fill(
                        text.reward_list,
                        &[("count", &rewards.len().to_string()), ("rewards", &list)],
                    )
                };
                self.buffers[i].push(notice_line(NoticeLevel::Info, said));
            }
            // Somebody spent their points, or what they spent them on was
            // answered. Only rewards this client made ever arrive here.
            Event::RedemptionUpdate {
                channel,
                redemption,
            } => {
                use liveside_proto::model::RedemptionStatus;
                let Some(i) = self.index_of(&channel) else {
                    return;
                };
                let text = self.text();
                self.buffers[i]
                    .redemptions
                    .retain(|kept| kept.id != redemption.id);
                let said = match redemption.status {
                    // Said by the notice that comes with it; the queue is
                    // what this adds.
                    RedemptionStatus::Unfulfilled => {
                        self.buffers[i].redemptions.push(redemption);
                        None
                    }
                    RedemptionStatus::Fulfilled => Some(liveside_client::lang::fill(
                        text.reward_done,
                        &[
                            ("reward", &redemption.reward_title),
                            ("user", &redemption.user.display_name),
                        ],
                    )),
                    RedemptionStatus::Canceled => Some(liveside_client::lang::fill(
                        text.reward_refunded,
                        &[
                            ("reward", &redemption.reward_title),
                            ("user", &redemption.user.display_name),
                        ],
                    )),
                };
                if let Some(said) = said {
                    self.buffers[i].push(notice_line(NoticeLevel::Info, said));
                }
            }
            // A list asked for with /mods, /vips, /banned, /followerlist
            // or /sublist. The terminal has no table, so it says it in a
            // line, and a long list only begins there.
            Event::People {
                channel,
                list,
                people,
                total,
                points,
            } => {
                let Some(i) = self.index_of(&channel) else {
                    return;
                };
                const NAMED: usize = 100;
                let text = self.text();
                let heading =
                    liveside_client::lang::people_heading(text, list, people.len(), total, points);
                let said = if people.is_empty() {
                    match total {
                        // Counted but not named: the heading says why.
                        Some(total) if total > 0 => heading,
                        _ => format!("{heading}: {}", text.list_empty),
                    }
                } else {
                    let mut names: Vec<String> = people
                        .iter()
                        .take(NAMED)
                        .map(|person| {
                            match liveside_client::lang::person_detail(text, list, person) {
                                Some(detail) => format!("{} ({detail})", person.name),
                                None => person.name.clone(),
                            }
                        })
                        .collect();
                    if people.len() > NAMED {
                        names.push(liveside_client::lang::fill(
                            text.list_more,
                            &[("count", &(people.len() - NAMED).to_string())],
                        ));
                    }
                    format!("{heading}: {}", names.join(", "))
                };
                self.buffers[i].push(notice_line(NoticeLevel::Info, said));
            }
            // The channels this account moderates, said where the reader
            // is; the ones already joined are marked.
            Event::ModeratedChannels { channels } => {
                let text = self.text();
                let said = if channels.is_empty() {
                    text.moderated_none.to_owned()
                } else {
                    let names = channels
                        .iter()
                        .map(|channel| self.marked_if_joined(channel))
                        .collect::<Vec<_>>()
                        .join(", ");
                    liveside_client::lang::fill(
                        text.moderated_line,
                        &[("count", &channels.len().to_string()), ("channels", &names)],
                    )
                };
                self.local(NoticeLevel::Info, said);
            }
            // The followed channels, said where the reader is: a line for
            // each that streams, with what and for how long, and the rest
            // on one line, all by the login /join takes.
            Event::FollowedChannels { channels } => {
                let text = self.text();
                if channels.is_empty() {
                    self.local(NoticeLevel::Info, text.followed_none);
                    return;
                }
                let now = Timestamp::now();
                let (live, offline): (Vec<_>, Vec<_>) = channels
                    .iter()
                    .partition(|followed| followed.live.is_some());
                let mut said = vec![liveside_client::lang::fill(
                    text.followed_heading,
                    &[
                        ("count", &channels.len().to_string()),
                        ("live", &live.len().to_string()),
                    ],
                )];
                for followed in &live {
                    let Some(stream) = &followed.live else {
                        continue;
                    };
                    let parts = [
                        format!("● {}", self.marked_if_joined(&followed.channel)),
                        stream.category.clone(),
                        stream.title.clone(),
                        liveside_client::lang::stream_stats(text, &stream.stats, now),
                    ];
                    said.push(
                        parts
                            .into_iter()
                            .filter(|part| !part.trim().is_empty())
                            .collect::<Vec<_>>()
                            .join(" · "),
                    );
                }
                if !offline.is_empty() {
                    let names = offline
                        .iter()
                        .map(|followed| self.marked_if_joined(&followed.channel))
                        .collect::<Vec<_>>()
                        .join(", ");
                    said.push(liveside_client::lang::fill(
                        text.followed_offline,
                        &[("count", &offline.len().to_string()), ("channels", &names)],
                    ));
                }
                for line in said {
                    self.local(NoticeLevel::Info, line);
                }
            }
            // A followed channel that began streaming, said in the status
            // buffer with what it streams and how to get there. The list
            // itself is printed only when asked for.
            Event::FollowedStreams { live, started } => {
                let text = self.text();
                for followed in live.iter().filter(|f| started.contains(&f.channel.id)) {
                    let about = followed
                        .live
                        .as_ref()
                        .map(|stream| liveside_client::followed::about(stream, text))
                        .unwrap_or_default();
                    self.status(
                        NoticeLevel::Info,
                        liveside_client::lang::fill(
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
            // What the channel blocks, as one line where it was asked;
            // /unblockterm takes a term by its text.
            Event::BlockedTerms { channel, terms } => {
                let Some(i) = self.index_of(&channel) else {
                    return;
                };
                let said = blocked_terms_line(self.text(), &self.buffers[i].title, &terms);
                self.buffers[i].push(notice_line(NoticeLevel::Info, said));
            }
            // A card asked for with /user, printed where it was asked.
            Event::UserCard { channel, card } => {
                let Some(i) = self.index_of(&channel) else {
                    return;
                };
                let text = self.text();
                let mut said = vec![liveside_client::lang::fill(
                    text.card_user,
                    &[
                        ("user", &card.user.display_name),
                        ("login", card.user.login.as_str()),
                    ],
                )];
                // Twitch's times carry the hour and the time zone; in a
                // card the day is what is read.
                let day = |stamp: &str| stamp.split('T').next().unwrap_or(stamp).to_owned();
                if !card.created_at.is_empty() {
                    said.push(liveside_client::lang::fill(
                        text.card_created,
                        &[("date", &day(&card.created_at))],
                    ));
                }
                said.push(match &card.follows_since {
                    Some(date) => {
                        liveside_client::lang::fill(text.card_follows, &[("date", &day(date))])
                    }
                    None => text.card_not_following.to_owned(),
                });
                // Empty means both "not subscribed" and "we may not
                // ask", which Twitch does not tell apart either.
                said.push(match (card.sub_tier.is_empty(), card.sub_known) {
                    (true, false) => text.card_unknown.to_owned(),
                    (true, true) => text.card_not_subscribed.to_owned(),
                    (false, _) => liveside_client::lang::fill(
                        match card.sub_gift {
                            true => text.card_sub_gift,
                            false => text.card_subscribed,
                        },
                        &[("tier", &card.sub_tier)],
                    ),
                });
                if card.banned {
                    said.push(match &card.banned_until {
                        Some(date) => liveside_client::lang::fill(
                            text.card_timed_out,
                            &[("date", &day(date))],
                        ),
                        None => text.card_banned.to_owned(),
                    });
                }
                for line in said {
                    self.buffers[i].push(notice_line(NoticeLevel::Info, line));
                }
            }
            Event::UnbanRequested { channel, request } => {
                if let Some(i) = self.index_of(&channel) {
                    let said = liveside_client::lang::fill(
                        self.text().unban_request,
                        &[
                            ("user", &request.user.display_name),
                            ("text", &request.text),
                        ],
                    );
                    if !self.buffers[i]
                        .unban_requests
                        .iter()
                        .any(|other| other.id == request.id)
                    {
                        self.buffers[i].unban_requests.push(request);
                    }
                    self.buffers[i].push(notice_line(NoticeLevel::Warning, said));
                }
            }
            Event::UnbanResolved {
                channel,
                request,
                status,
                ..
            } => {
                if let Some(i) = self.index_of(&channel) {
                    let at = self.buffers[i]
                        .unban_requests
                        .iter()
                        .position(|waiting| waiting.id == request);
                    let Some(at) = at else { return };
                    let who = self.buffers[i].unban_requests.remove(at);
                    let pattern = if status == "approved" {
                        self.text().unban_approved
                    } else {
                        self.text().unban_denied
                    };
                    let said =
                        liveside_client::lang::fill(pattern, &[("user", &who.user.display_name)]);
                    self.buffers[i].push(notice_line(NoticeLevel::Info, said));
                }
            }
            Event::Stream { channel, live } => {
                if let Some(i) = self.index_of(&channel) {
                    self.buffers[i].live = live;
                    if !live {
                        self.buffers[i].stats = None;
                    }
                }
            }
            Event::StreamStats { channel, stats } => {
                if let Some(i) = self.index_of(&channel) {
                    self.buffers[i].stats = Some(stats);
                    self.buffers[i]
                        .viewers
                        .record(liveside_proto::Timestamp::now(), &stats);
                }
            }
            Event::Chatters { channel, logins } => {
                if let Some(i) = self.index_of(&channel) {
                    self.buffers[i].chatters = logins.into_iter().collect();
                }
            }
            Event::ChannelParted { channel } => {
                if self.graph.as_ref() == Some(&channel) {
                    self.graph = None;
                }
                if let Some(i) = self.index_of(&channel) {
                    self.buffers.remove(i);
                    if self.active >= i && self.active > 0 {
                        self.active -= 1;
                    }
                }
            }
            Event::RoomModesChanged { channel, modes } => {
                if let Some(i) = self.index_of(&channel) {
                    self.buffers[i].modes = modes;
                }
            }
            Event::Line { channel, line } => {
                self.add_collected(CollectedKind::Highlighted, &channel, &line);
                if let Some(i) = self.index_of(&channel) {
                    // A new title or category, for /viewers to list.
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
                        self.buffers[i]
                            .viewers
                            .mark(line.time, &arg("title"), &arg("game"));
                    }
                    self.buffers[i].push(line);
                }
            }
            // Hidden from the channel, kept in the list of what was.
            Event::Ignored { channel, line } => {
                self.add_collected(CollectedKind::Ignored, &channel, &line);
            }

            Event::History { channel, lines } => {
                if let Some(i) = self.index_of(&channel) {
                    for line in lines.into_iter().rev() {
                        self.buffers[i].lines.push_front(line);
                    }
                }
            }
            // Marks the message rather than adding a line; it may arrive
            // before the message does, so it is remembered until then.
            Event::SuspiciousMessage {
                channel,
                message,
                suspicion,
                ..
            } => {
                let Some(i) = self.index_of(&channel) else {
                    return;
                };
                let mut marked = false;
                for line in self.buffers[i].lines.iter_mut() {
                    if let LineContent::Message(m) = &line.content
                        && m.id == message
                    {
                        line.flags.suspicion = Some(suspicion.clone());
                        marked = true;
                    }
                }
                if !marked {
                    self.buffers[i]
                        .pending_suspicion
                        .push((message, *suspicion));
                    if self.buffers[i].pending_suspicion.len() > 50 {
                        self.buffers[i].pending_suspicion.remove(0);
                    }
                }
            }
            Event::MessageDeleted { channel, message } => {
                let Some(i) = self.index_of(&channel) else {
                    return;
                };
                for line in self.buffers[i].lines.iter_mut() {
                    if let LineContent::Message(m) = &line.content
                        && m.id == message
                    {
                        line.flags.deleted = true;
                    }
                }
            }
            Event::UserCleared { channel, user, .. } => {
                let Some(i) = self.index_of(&channel) else {
                    return;
                };
                for line in self.buffers[i].lines.iter_mut() {
                    if let LineContent::Message(m) = &line.content
                        && m.sender.id == user
                    {
                        line.flags.deleted = true;
                    }
                }
            }
            Event::ChatCleared { channel } => {
                if let Some(i) = self.index_of(&channel) {
                    self.buffers[i].lines.clear();
                }
            }
        }
    }

    /// Handles a key. Returns the commands to send to the engine, if any.
    pub fn on_key(&mut self, key: KeyEvent) -> Vec<Command> {
        if self.settings.open {
            return self.settings_key(key);
        }
        // The graph is only looked at: Esc, Enter or q put it away, and
        // nothing else is typed underneath while it is up.
        if self.graph.is_some() {
            if matches!(key.code, KeyCode::Esc | KeyCode::Enter | KeyCode::Char('q')) {
                self.graph = None;
            }
            return Vec::new();
        }
        if self.search.is_some() {
            self.search_key(key);
            return Vec::new();
        }
        // Only Tab continues a completion; anything else ends it.
        if !matches!(key.code, KeyCode::Tab | KeyCode::BackTab) {
            self.input.completion.stop();
        }
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        // A hotkey of the user's own comes before the built-in keys, so it
        // can take one over; quitting stays, as the way out of anything.
        if !(ctrl && matches!(key.code, KeyCode::Char('c' | 'q' | 'C' | 'Q')))
            && let Some(commands) = self.hotkey(key)
        {
            return commands;
        }
        match key.code {
            KeyCode::Char('c' | 'q') if ctrl => {
                self.quit = true;
                Vec::new()
            }
            KeyCode::Char('u') if ctrl => {
                self.input.clear();
                Vec::new()
            }
            KeyCode::Char('w') if ctrl => {
                self.input.delete_word();
                Vec::new()
            }
            // As in a terminal: the buffer on screen is emptied for the
            // reader only. Twitch's /clear would empty it for everyone.
            KeyCode::Char('l') if ctrl => {
                let buffer = self.buffer_mut();
                buffer.lines.clear();
                buffer.scroll = 0;
                buffer.selected = None;
                Vec::new()
            }
            KeyCode::Char('n') if ctrl => {
                self.switch(1);
                Vec::new()
            }
            KeyCode::Char('p') if ctrl => {
                self.switch(-1);
                Vec::new()
            }
            KeyCode::Char('r') if ctrl => {
                self.start_reply();
                Vec::new()
            }
            KeyCode::Char('f') if ctrl => {
                self.search = Some(Search::default());
                Vec::new()
            }
            // Moderation on the selected message. These write the command
            // into the input instead of running it: a ban is not something
            // a single keystroke should do, and the line can still be
            // edited (a different duration, a reason) before Enter.
            KeyCode::Char('t') if ctrl => {
                self.prepare_command("/timeout {user} 10m");
                Vec::new()
            }
            KeyCode::Char('b') if ctrl => {
                self.prepare_command("/ban {user} ");
                Vec::new()
            }
            KeyCode::Char('d') if ctrl => {
                self.prepare_command("/delete {id}");
                Vec::new()
            }
            // Alt, because Ctrl-P already walks back through the buffers.
            KeyCode::Char('p') if alt => {
                self.prepare_command("/pin {id}");
                Vec::new()
            }
            KeyCode::Char(c) => {
                self.input.insert(c);
                Vec::new()
            }
            KeyCode::Backspace => {
                self.input.backspace();
                Vec::new()
            }
            KeyCode::Delete => {
                self.input.delete();
                Vec::new()
            }
            KeyCode::Left => {
                self.input.left();
                Vec::new()
            }
            KeyCode::Right => {
                self.input.right();
                Vec::new()
            }
            KeyCode::Home => {
                self.input.cursor = 0;
                Vec::new()
            }
            KeyCode::End => {
                self.input.cursor = self.input.text.len();
                Vec::new()
            }
            KeyCode::Tab => {
                // With nothing typed there is nothing to complete, and Tab
                // keeps its old meaning.
                if !self.complete(1) {
                    self.switch(1);
                }
                Vec::new()
            }
            KeyCode::BackTab => {
                if !self.complete(-1) {
                    self.switch(-1);
                }
                Vec::new()
            }
            // Ctrl walks over messages; plain Up/Down are the input history.
            KeyCode::Up if ctrl => {
                self.select(-1);
                Vec::new()
            }
            KeyCode::Down if ctrl => {
                self.select(1);
                Vec::new()
            }
            KeyCode::Up => {
                self.input.earlier();
                Vec::new()
            }
            KeyCode::Down => {
                self.input.later();
                Vec::new()
            }
            KeyCode::PageUp => {
                let page = self.page();
                let limit = self.scroll_limit();
                let buffer = self.buffer_mut();
                buffer.scroll = (buffer.scroll + page).min(limit);
                Vec::new()
            }
            KeyCode::PageDown => {
                let page = self.page();
                let buffer = self.buffer_mut();
                // Within one page of the bottom, go live: in a fast channel
                // the view drifts back a line per message, and paging by a
                // fixed amount would never catch up.
                buffer.scroll = match buffer.scroll.saturating_sub(page) {
                    remaining if remaining < page => 0,
                    remaining => remaining,
                };
                Vec::new()
            }
            KeyCode::Esc => {
                // Esc backs out of the innermost thing first.
                let buffer = self.buffer_mut();
                if buffer.replying_to.take().is_none() && buffer.selected.take().is_none() {
                    buffer.scroll = 0;
                }
                Vec::new()
            }
            KeyCode::F(2) => {
                self.settings.open();
                Vec::new()
            }
            // Ctrl-U is already "clear the line", as in any shell.
            KeyCode::F(3) => {
                self.show_users = !self.show_users;
                Vec::new()
            }
            // Clipping is the one thing that has to happen now rather
            // than after finding the right words for a command.
            KeyCode::F(4) => self.clip(),
            KeyCode::Enter => self.submit(),
            _ => Vec::new(),
        }
    }

    /// Goes where a clicked notification happened: the channel's or the
    /// whisper's buffer, a followed channel joined first (its buffer
    /// opens when it arrives), or the list of followed channels for
    /// several. The terminal cannot be brought to the front from here.
    pub fn go_to(&mut self, place: liveside_client::notify::Place) -> Vec<Command> {
        use liveside_client::notify::Place;
        let found = match &place {
            Place::Channel(id) => self.index_of(id),
            Place::Whisper(login) => self.whisper_buffer(login),
            Place::Followed(Some(login)) => self.buffers.iter().position(|buffer| {
                buffer.channel.is_some() && buffer.title.trim_start_matches('#') == login.as_str()
            }),
            Place::Followed(None) => return vec![Command::ShowFollowedChannels],
        };
        match (found, place) {
            (Some(i), _) => {
                self.active = i;
                Vec::new()
            }
            (None, Place::Followed(Some(login))) => {
                self.open_when_joined = Some(login.clone());
                vec![Command::Join { channel: login }]
            }
            (None, _) => Vec::new(),
        }
    }

    fn switch(&mut self, delta: isize) {
        let n = self.buffers.len() as isize;
        self.active = (self.active as isize + delta).rem_euclid(n) as usize;
    }

    /// Turns the typed line into commands. Only what the engine cannot do
    /// without a channel is handled here; everything else goes to the engine
    /// as typed, so its command language stays the single source of truth.
    fn submit(&mut self) -> Vec<Command> {
        let text = self.input.take();
        if text.trim().is_empty() {
            return Vec::new();
        }
        self.input.history.remember(&text);
        self.submit_text(text)
    }

    /// Runs a hotkey, if the key is one: the line it makes goes on as a
    /// typed one would, and the input line is left as it was.
    fn hotkey(&mut self, key: KeyEvent) -> Option<Vec<Command>> {
        use liveside_client::hotkeys::{Key, KeyCode as HotCode};
        let code = match key.code {
            KeyCode::F(n) => HotCode::F(n),
            KeyCode::Char(c) if c.is_ascii_alphanumeric() => HotCode::Char(c.to_ascii_lowercase()),
            _ => return None,
        };
        let pressed = Key {
            ctrl: key.modifiers.contains(KeyModifiers::CONTROL),
            alt: key.modifiers.contains(KeyModifiers::ALT),
            shift: key.modifiers.contains(KeyModifiers::SHIFT)
                || matches!(key.code, KeyCode::Char(c) if c.is_ascii_uppercase()),
            code,
        };
        let hotkeys = self.settings.hotkeys();
        let hotkey = hotkeys.find(&pressed)?;
        let selected = self.selected_user();
        Some(
            match hotkey.run(selected.as_deref(), &self.command_context()) {
                Ok(line) if line.is_empty() => Vec::new(),
                Ok(line) => self.submit_text(line),
                Err(problem) => {
                    let said = liveside_client::lang::custom_problem(self.text(), &problem);
                    self.local(NoticeLevel::Error, said);
                    Vec::new()
                }
            },
        )
    }

    /// Who wrote the selected message, for a hotkey's `$1`.
    fn selected_user(&self) -> Option<String> {
        let buffer = self.buffer();
        let selected = buffer.selected.as_ref()?;
        buffer
            .lines
            .iter()
            .rev()
            .find_map(|line| match &line.content {
                LineContent::Message(m) if &m.id == selected => Some(m.sender.login.to_string()),
                _ => None,
            })
    }

    /// A line as if typed: what Enter does with the input, and what a
    /// hotkey does with what it makes.
    fn submit_text(&mut self, text: String) -> Vec<Command> {
        // Acting on a buffer returns to its newest lines: the answer to
        // what was just typed must not appear off screen.
        self.buffer_mut().scroll = 0;
        // A custom command first: what it makes goes on as if typed, and
        // is not expanded again.
        let text = match liveside_client::custom::expand(
            &self.settings.custom_commands(),
            &text,
            &self.command_context(),
            self.text(),
        ) {
            liveside_client::custom::Typed::Unchanged => text,
            liveside_client::custom::Typed::Replaced(line) if line.is_empty() => {
                return Vec::new();
            }
            liveside_client::custom::Typed::Replaced(line) => line,
            liveside_client::custom::Typed::Refused(why) => {
                self.local(NoticeLevel::Error, why);
                return Vec::new();
            }
        };
        let (name, args) = match text.strip_prefix('/') {
            Some(rest) if !rest.starts_with('/') => match rest.split_once(' ') {
                Some((name, args)) => (name.to_ascii_lowercase(), args.trim()),
                None => (rest.to_ascii_lowercase(), ""),
            },
            _ => (String::new(), ""),
        };
        // In a whisper thread, what is typed goes to that person. A
        // command is still a command, since /quit and the rest work
        // everywhere.
        if name.is_empty()
            && let Some(login) = self.buffer().whisper.clone()
        {
            // `//` is how a line that starts with `/` is sent.
            let text = match text.strip_prefix("//") {
                Some(rest) => format!("/{rest}"),
                None => text,
            };
            return vec![Command::SendWhisper { user: login, text }];
        }
        // Commands the frontend answers itself, in any buffer.
        match name.as_str() {
            "quit" | "exit" => {
                self.quit = true;
                return Vec::new();
            }
            // Said here and to nobody else, as Chatty's /echo is: what a
            // custom command uses to tell its own reader something.
            "echo" => {
                if !args.is_empty() {
                    self.local(NoticeLevel::Info, args.to_owned());
                }
                return Vec::new();
            }
            // Whispering, from any buffer: a whisper belongs to the
            // account rather than to a channel, so it is answered here
            // rather than by the engine's channel parser.
            "w" | "whisper" => {
                let Some((who, said)) = args.trim().split_once(char::is_whitespace) else {
                    self.local(NoticeLevel::Error, self.text().whisper_usage);
                    return Vec::new();
                };
                let who = who.trim_start_matches('@').to_ascii_lowercase();
                if who.is_empty() || said.trim().is_empty() {
                    self.local(NoticeLevel::Error, self.text().whisper_usage);
                    return Vec::new();
                }
                return vec![Command::SendWhisper {
                    user: who.into(),
                    text: said.trim().to_owned(),
                }];
            }
            // Answers "what does this program send where?" from what the
            // engine reports about itself, not from documentation.
            "privacy" => {
                self.show_data_flows();
                return Vec::new();
            }
            "users" | "names" => {
                self.show_users = !self.show_users;
                return Vec::new();
            }
            // What there is to join: an account's question, from any
            // buffer.
            "moderated" => return vec![Command::ShowModeratedChannels],
            "followed" | "follows" => return vec![Command::ShowFollowedChannels],
            // What lit up and what was hidden, in every channel.
            "highlights" | "highlighted" => {
                self.open_collected(CollectedKind::Highlighted);
                return Vec::new();
            }
            "ignored" => {
                self.open_collected(CollectedKind::Ignored);
                return Vec::new();
            }
            // Ignoring somebody is a rule in the settings, saved at once
            // and handed to the engine.
            "ignore" | "unignore" => return self.ignore_command(&name, args),
            // How many watched, as a graph of braille dots over the chat.
            "viewers" => {
                let Some(channel) = self.buffer().channel.clone() else {
                    self.local(NoticeLevel::Error, self.text().not_in_channel);
                    return Vec::new();
                };
                self.graph = Some(channel);
                return Vec::new();
            }
            // The address book, which rules and /join [category] use.
            "ab" => {
                let text = self.text();
                let answer = liveside_client::addressbook::command(&mut self.settings, args, text);
                self.local(answer.level, answer.said);
                return match answer.changed {
                    true => vec![self.settings.rules_command()],
                    false => Vec::new(),
                };
            }
            // A list closes like a channel is left, but it is the window's.
            "close" | "part" | "leave" if self.buffer().collected.is_some() => {
                let i = self.active;
                self.buffers.remove(i);
                self.active = i.saturating_sub(1);
                return Vec::new();
            }
            // Watching the channel on screen, here, with Streamlink.
            "stream" => {
                self.start_streamlink(args.trim());
                return Vec::new();
            }
            // Asking VirusTotal about a link in the selected message, or
            // in the newest one that has a link.
            "link" => return self.link_command(args),
            "settings" | "config" => {
                self.settings.open();
                return Vec::new();
            }
            // Starting a poll, and stopping the one that runs.
            // Bare, these say how the one that is running stands, which
            // is the terminal's own business: the engine knows the poll,
            // the buffer knows what this window has been told. With
            // arguments they fall through and the engine parses them.
            "poll" if args.trim().is_empty() => {
                let said = match &self.buffer().poll {
                    Some(poll) => liveside_client::lang::fill(
                        self.text().poll_standing,
                        &[
                            ("total", &poll_total(poll).to_string()),
                            ("choices", &poll_standing(poll)),
                        ],
                    ),
                    None => self.text().poll_none.to_owned(),
                };
                self.local(NoticeLevel::Info, &said);
                return Vec::new();
            }
            // Starting a prediction, closing betting, paying it out.
            "prediction" | "pred" if args.trim().is_empty() => {
                let said = match &self.buffer().prediction {
                    Some(prediction) => {
                        let status = match prediction.status {
                            PredictionStatus::Locked => self.text().pred_waiting,
                            _ => self.text().pred_betting,
                        };
                        liveside_client::lang::fill(
                            self.text().pred_standing,
                            &[
                                ("status", status),
                                ("outcomes", &prediction_standing(prediction, self.text())),
                            ],
                        )
                    }
                    None => self.text().pred_none.to_owned(),
                };
                self.local(NoticeLevel::Info, &said);
                return Vec::new();
            }
            "queue" | "redemptions" => {
                let text = self.text();
                let waiting = self.buffer().redemptions.clone();
                if waiting.is_empty() {
                    self.local(NoticeLevel::Info, text.reward_queue_empty);
                    return Vec::new();
                }
                self.local(
                    NoticeLevel::Info,
                    liveside_client::lang::fill(
                        text.reward_queue,
                        &[("count", &waiting.len().to_string())],
                    ),
                );
                for (n, redemption) in waiting.iter().enumerate() {
                    let said = liveside_client::lang::fill(
                        text.reward_waiting,
                        &[
                            ("user", &redemption.user.display_name),
                            ("reward", &redemption.reward_title),
                            ("cost", &redemption.cost.to_string()),
                            (
                                "text",
                                &match redemption.text.is_empty() {
                                    true => String::new(),
                                    false => format!(": {}", redemption.text),
                                },
                            ),
                        ],
                    );
                    self.local(NoticeLevel::Info, format!("{}. {said}", n + 1));
                }
                return Vec::new();
            }
            "fulfil" | "fulfill" | "refund" => {
                let Some(channel) = self.buffer().channel.clone() else {
                    self.local(NoticeLevel::Error, self.text().not_in_channel);
                    return Vec::new();
                };
                let wanted = args.trim().parse::<usize>().ok().filter(|n| *n >= 1);
                let Some(redemption) = wanted
                    .and_then(|n| self.buffer().redemptions.get(n - 1))
                    .cloned()
                else {
                    self.local(NoticeLevel::Error, self.text().reward_usage);
                    return Vec::new();
                };
                return vec![Command::AnswerRedemption {
                    channel,
                    reward: redemption.reward_id,
                    redemption: redemption.id,
                    fulfil: name != "refund",
                }];
            }
            // Answering someone asking to be let back in.
            "approve" | "refuse" => {
                let allow = name == "approve";
                let wanted = args.trim_start_matches('@').to_ascii_lowercase();
                let Some(channel) = self.buffer().channel.clone() else {
                    self.local(NoticeLevel::Error, self.text().not_in_channel);
                    return Vec::new();
                };
                let found = self
                    .buffer()
                    .unban_requests
                    .iter()
                    .find(|request| wanted.is_empty() || request.user.login.as_str() == wanted);
                let Some(request) = found else {
                    self.local(NoticeLevel::Error, self.text().nothing_requested);
                    return Vec::new();
                };
                return vec![Command::ResolveUnban {
                    channel,
                    request: request.id.clone(),
                    allow,
                }];
            }
            // Deciding what AutoMod caught. Without a name it answers the
            // oldest, which is what a queue is usually worked through as.
            "allow" | "deny" => {
                let allow = name == "allow";
                let wanted = args.trim_start_matches('@').to_ascii_lowercase();
                let Some(channel) = self.buffer().channel.clone() else {
                    self.local(NoticeLevel::Error, self.text().not_in_channel);
                    return Vec::new();
                };
                let found = self
                    .buffer()
                    .held
                    .iter()
                    .find(|held| wanted.is_empty() || held.sender.login.as_str() == wanted);
                let Some(held) = found else {
                    self.local(NoticeLevel::Error, self.text().nothing_held);
                    return Vec::new();
                };
                return vec![Command::ResolveHeld {
                    channel,
                    message: held.id.clone(),
                    allow,
                }];
            }
            _ => {}
        }
        // In a channel the engine parses the line, including `/join a,b`.
        if let Some(channel) = self.buffer().channel.clone() {
            let buffer = &mut self.buffers[self.active];
            let reply_to = buffer.replying_to.take().map(|target| target.id);
            buffer.selected = None;
            return vec![Command::Input {
                channel,
                text,
                reply_to,
            }];
        }
        match name.as_str() {
            "login" => vec![Command::Login],
            "logout" => vec![Command::Logout],
            // Without a channel there is no engine-side parsing, so the list
            // of channels is split here.
            // `[category #]` stays whole; the engine expands it.
            "join" if !args.is_empty() => liveside_proto::join_targets(args)
                .into_iter()
                .map(|name| Command::Join {
                    channel: name.as_str().into(),
                })
                .collect(),
            _ => {
                self.local(NoticeLevel::Error, self.text().not_in_channel);
                Vec::new()
            }
        }
    }

    /// Commands this frontend answers itself, for completion; aliases are
    /// left out, as they would only make the cycle longer. Keep in step
    /// with `submit_text`.
    const OWN_COMMANDS: &'static [&'static str] = &[
        "quit",
        "echo",
        "w",
        "whisper",
        "privacy",
        "users",
        "moderated",
        "followed",
        "highlights",
        "ignored",
        "ignore",
        "unignore",
        "viewers",
        "ab",
        "stream",
        "link",
        "settings",
        "poll",
        "prediction",
        "queue",
        "fulfil",
        "refund",
        "approve",
        "refuse",
        "allow",
        "deny",
        "close",
    ];

    /// Who has been active in the active buffer, ranked then alphabetical,
    /// as the user list shows them.
    pub fn chatters(&self) -> Vec<Chatter> {
        let buffer = self.buffer();
        let mut gathering = liveside_client::chatters::Gathering::default();
        for line in buffer.lines.iter().rev() {
            if let LineContent::Message(message) = &line.content {
                gathering.talked(
                    &message.sender.login,
                    &message.sender.display_name,
                    &message.badges,
                );
            }
        }
        for login in &buffer.chatters {
            gathering.present(login);
        }
        gathering.finish()
    }

    /// Keys while the settings screen is open. Editing a list of words
    /// takes the keyboard entirely, so Esc there only leaves the field.
    fn settings_key(&mut self, key: KeyEvent) -> Vec<Command> {
        if let Some(text) = &mut self.settings.editing {
            match key.code {
                KeyCode::Char(c) => text.push(c),
                KeyCode::Backspace => {
                    text.pop();
                }
                KeyCode::Enter => self.settings.finish_editing(),
                KeyCode::Esc if self.settings.rules.is_some() => self.settings.rules_cancel_line(),
                KeyCode::Esc => self.settings.editing = None,
                _ => {}
            }
            return Vec::new();
        }
        // A list of rules, open in place of the settings.
        if self.settings.rules.is_some() {
            match key.code {
                KeyCode::Up => self.settings.rules_move(-1),
                KeyCode::Down => self.settings.rules_move(1),
                KeyCode::Enter => self.settings.rules_edit(),
                KeyCode::Char('a') | KeyCode::Insert => self.settings.rules_add(),
                KeyCode::Char('d') | KeyCode::Delete => self.settings.rules_delete(),
                KeyCode::Esc | KeyCode::Char('q') => self.settings.rules_close(),
                _ => {}
            }
            return Vec::new();
        }
        match key.code {
            KeyCode::Up => self.settings.move_by(-1),
            KeyCode::Down => self.settings.move_by(1),
            KeyCode::Left => self.settings.adjust(-1),
            KeyCode::Right => self.settings.adjust(1),
            KeyCode::Char(' ') | KeyCode::Enter => self.settings.activate(),
            KeyCode::Char('s') => {
                if !self.settings.save() {
                    return Vec::new();
                }
                self.notifier
                    .set_config(self.settings.config().notify.clone());
                // The interface reads its text every frame, so a new
                // language applies as soon as it is saved.
                if let Some(language) =
                    liveside_client::lang::Language::parse(&self.settings.config().ui.language)
                {
                    self.language = language;
                    self.settings.set_language(language);
                }
                // So does the background, which the colours are made for.
                crate::render::set_light_background(light_background(
                    self.settings.config().ui.theme,
                    self.terminal_light,
                ));
                // The engine takes the rules as saved, so they apply at once.
                return vec![self.settings.rules_command()];
            }
            KeyCode::Esc | KeyCode::F(2) | KeyCode::Char('q') => self.settings.close(),
            _ => {}
        }
        Vec::new()
    }

    /// Moves the selection through the messages of the active buffer:
    /// -1 towards older, +1 towards newer. Starts at the newest message.
    ///
    /// Only messages can be selected; notices and events are skipped, since
    /// there is nothing to answer there.
    /// Keys while searching: typing changes the query and goes to the
    /// newest match, Enter, Up or Ctrl-F go further back, Down forward,
    /// Esc ends the search where it is.
    fn search_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let Some(search) = self.search.as_mut() else {
            return;
        };
        match key.code {
            KeyCode::Char('c' | 'q') if ctrl => self.quit = true,
            KeyCode::Esc => self.search = None,
            KeyCode::Enter | KeyCode::Up => self.step_search(-1),
            KeyCode::Char('f') if ctrl => self.step_search(-1),
            KeyCode::Down => self.step_search(1),
            KeyCode::Backspace => {
                search.query.pop();
                search.current = None;
                self.step_search(-1);
            }
            KeyCode::Char(c) if !ctrl => {
                search.query.push(c);
                search.current = None;
                self.step_search(-1);
            }
            _ => {}
        }
    }

    /// Goes to the next older (`-1`) or newer (`1`) line that matches,
    /// wrapping round; the first step goes to the newest. The line is
    /// selected, so Ctrl-R and the moderation keys act on it.
    fn step_search(&mut self, direction: isize) {
        let strings = self.text();
        let page = self.chat_height / 2;
        let buffer = &mut self.buffers[self.active];
        let Some(search) = self.search.as_mut() else {
            return;
        };
        let found: Vec<usize> = if search.query.is_empty() {
            Vec::new()
        } else {
            buffer
                .lines
                .iter()
                .enumerate()
                .filter(|(_, line)| {
                    contains_ignoring_case(&line_text(line, strings), &search.query)
                })
                .map(|(i, _)| i)
                .collect()
        };
        if found.is_empty() {
            search.current = None;
            search.at = None;
            // Nor is what an earlier, shorter query found still selected:
            // Ctrl-R would answer a line this query does not match.
            buffer.selected = None;
            return;
        }
        let key_of = |line: &Line| (line.time, line_text(line, strings));
        let here = search.current.as_ref().and_then(|current| {
            found
                .iter()
                .position(|&i| &key_of(&buffer.lines[i]) == current)
        });
        let next = match here {
            None => found.len() - 1,
            Some(at) => (at as isize + direction).rem_euclid(found.len() as isize) as usize,
        };
        let index = found[next];
        let line = &buffer.lines[index];
        search.current = Some(key_of(line));
        search.at = Some((found.len() - next, found.len()));
        buffer.selected = match &line.content {
            LineContent::Message(message) => Some(message.id.clone()),
            _ => None,
        };
        // About the middle of the screen, as far as lines are one row.
        let from_end = buffer.lines.len() - 1 - index;
        buffer.scroll = from_end.saturating_sub(page);
    }

    fn select(&mut self, direction: isize) {
        let buffer = &mut self.buffers[self.active];
        let ids: Vec<MessageId> = buffer
            .lines
            .iter()
            .filter_map(|line| match &line.content {
                LineContent::Message(message) => Some(message.id.clone()),
                _ => None,
            })
            .collect();
        if ids.is_empty() {
            return;
        }
        let next = match buffer
            .selected
            .as_ref()
            .and_then(|id| ids.iter().position(|other| other == id))
        {
            None => ids.len() - 1,
            Some(current) => {
                let moved = current as isize + direction;
                moved.clamp(0, ids.len() as isize - 1) as usize
            }
        };
        buffer.selected = Some(ids[next].clone());
    }

    /// Starts Streamlink for the channel on screen, at `quality` or the
    /// one in the settings, and says so.
    fn start_streamlink(&mut self, quality: &str) {
        let text = self.text();
        let buffer = self.buffer();
        if buffer.channel.is_none() {
            self.local(NoticeLevel::Error, text.streamlink_needs_channel);
            return;
        }
        let title = buffer.title.clone();
        let login = title.trim_start_matches('#').to_owned();
        let config = self.settings.config().streamlink.clone();
        let quality = Some(quality).filter(|q| !q.is_empty());
        let said_quality = quality.unwrap_or(&config.quality).to_owned();
        let ended = self.streamlink_ended.clone();
        let from = title.clone();
        let started =
            liveside_client::streamlink::start(&config, &login, quality, move |outcome| {
                if let Some(ended) = ended {
                    let _ = ended.send((from, outcome));
                }
            });
        match started {
            Ok(()) => self.local(
                NoticeLevel::Info,
                liveside_client::lang::fill(
                    text.streamlink_started,
                    &[("channel", &login), ("quality", &said_quality)],
                ),
            ),
            Err(e) => self.local(
                NoticeLevel::Error,
                liveside_client::lang::fill(
                    text.streamlink_missing,
                    &[("command", &config.command), ("error", &e.to_string())],
                ),
            ),
        }
    }

    /// Says why Streamlink stopped, in the buffer it was started from.
    pub fn streamlink_ended(&mut self, from: &str, outcome: liveside_client::streamlink::Outcome) {
        let Some(said) = liveside_client::lang::streamlink_ended(
            self.text(),
            from.trim_start_matches('#'),
            &outcome,
        ) else {
            return;
        };
        match self.buffers.iter().position(|buffer| buffer.title == from) {
            Some(i) => self.buffers[i].push(notice_line(NoticeLevel::Error, said)),
            None => self.status(NoticeLevel::Error, said),
        }
    }

    /// `/link virustotal [n]` asks about the n-th link (the first by
    /// default) of the selected message, or of the newest message with a
    /// link; `/link scan [n]` also sends it to be scanned when VirusTotal
    /// does not know it; `/link where [n]` follows its redirects.
    fn link_command(&mut self, args: &str) -> Vec<Command> {
        enum Ask {
            VirusTotal { scan: bool },
            Where,
        }
        let mut words = args.split_whitespace();
        let ask = match words.next().map(str::to_ascii_lowercase).as_deref() {
            Some("virustotal" | "vt" | "check") => Ask::VirusTotal { scan: false },
            Some("scan") => Ask::VirusTotal { scan: true },
            Some("where" | "follow") => Ask::Where,
            _ => {
                self.local(NoticeLevel::Error, self.text().link_usage);
                return Vec::new();
            }
        };
        let nth = match words.next().map(str::parse::<usize>) {
            None => 1,
            Some(Ok(n)) if n >= 1 => n,
            Some(_) => {
                self.local(NoticeLevel::Error, self.text().link_usage);
                return Vec::new();
            }
        };
        let buffer = self.buffer();
        let links_of = |message: &liveside_proto::model::ChatMessage| -> Vec<String> {
            message
                .fragments
                .iter()
                .filter_map(|fragment| match fragment {
                    Fragment::Link { url, .. } => Some(url.clone()),
                    _ => None,
                })
                .collect()
        };
        let links = buffer
            .lines
            .iter()
            .rev()
            .filter_map(|line| match &line.content {
                LineContent::Message(message) => Some(message),
                _ => None,
            })
            .find(|message| match &buffer.selected {
                Some(id) => &message.id == id,
                None => !links_of(message).is_empty(),
            })
            .map(links_of)
            .unwrap_or_default();
        match (links.get(nth - 1), ask) {
            (Some(url), Ask::VirusTotal { scan }) => vec![Command::CheckLink {
                url: url.clone(),
                scan,
            }],
            (Some(url), Ask::Where) => vec![Command::FollowLink { url: url.clone() }],
            (None, _) => {
                self.local(NoticeLevel::Error, self.text().link_none);
                Vec::new()
            }
        }
    }

    /// Fills the input with `template` for the selected message, ready to
    /// be looked at and sent. `{user}` and `{id}` are filled in.
    fn prepare_command(&mut self, template: &str) {
        if self.buffer().channel.is_none() {
            self.local(NoticeLevel::Error, self.text().moderation_needs_channel);
            return;
        }
        if self.buffer().selected.is_none() {
            self.select(-1);
        }
        let buffer = &self.buffers[self.active];
        let Some(id) = buffer.selected.clone() else {
            self.local(NoticeLevel::Error, self.text().no_message_selected);
            return;
        };
        let Some(message) = buffer.lines.iter().find_map(|line| match &line.content {
            LineContent::Message(message) if message.id == id => Some(message),
            _ => None,
        }) else {
            return;
        };
        let text = template
            .replace("{user}", message.sender.login.as_str())
            .replace("{id}", id.as_str());
        self.input.text = text;
        self.input.cursor = self.input.text.len();
    }

    /// Answers the selected message; without a selection, the newest one.
    fn start_reply(&mut self) {
        if self.buffer().channel.is_none() {
            self.local(NoticeLevel::Error, self.text().reply_needs_channel);
            return;
        }
        if self.buffer().selected.is_none() {
            self.select(-1);
        }
        let buffer = &mut self.buffers[self.active];
        let Some(id) = buffer.selected.clone() else {
            self.local(NoticeLevel::Error, self.text().nothing_to_reply_to);
            return;
        };
        let name = buffer.lines.iter().find_map(|line| match &line.content {
            LineContent::Message(message) if message.id == id => {
                Some(message.sender.display_name.clone())
            }
            _ => None,
        });
        buffer.replying_to = name.map(|name| ReplyTarget { id, name });
    }

    /// Completes the word before the cursor, cycling on repeated presses.
    /// Returns false when there is nothing to complete, so the caller can
    /// give the key its other meaning.
    ///
    /// Candidates come from what is on screen: who talked in this channel
    /// and which emotes they used, newest first. A command is completed
    /// from the engine's own list.
    fn complete(&mut self, direction: isize) -> bool {
        let names =
            liveside_client::command_names(Self::OWN_COMMANDS, &self.settings.custom_commands());
        let names: Vec<&str> = names.iter().map(String::as_str).collect();
        let lines = &self.buffers[self.active].lines;
        let step = self.input.completion.step(
            &self.input.text,
            self.input.cursor,
            direction,
            &names,
            |prefix| {
                liveside_client::input::names_and_emotes(
                    prefix,
                    lines.iter().rev().filter_map(|line| match &line.content {
                        LineContent::Message(message) => Some((
                            message.sender.display_name.as_str(),
                            message
                                .fragments
                                .iter()
                                .filter_map(|fragment| match fragment {
                                    Fragment::Emote { emote } => Some(emote.code.as_str()),
                                    _ => None,
                                })
                                .collect(),
                        )),
                        _ => None,
                    }),
                )
            },
        );
        match step {
            Step::Changed(done) => {
                self.input.text = done.text;
                self.input.cursor = done.cursor;
                true
            }
            Step::NoMatch => true, // Nothing matches: leave the typed word alone.
            Step::Nothing => false,
        }
    }

    /// How far one Page key moves: a screenful minus a line of overlap, so
    /// nothing is skipped between screens.
    fn page(&self) -> usize {
        self.chat_height.saturating_sub(1).max(1)
    }

    /// The furthest the active buffer can scroll back while still filling
    /// the screen with lines that exist.
    fn scroll_limit(&self) -> usize {
        self.buffers[self.active]
            .lines
            .len()
            .saturating_sub(self.chat_height)
    }

    /// Writes the engine's data flows into the status buffer: who gets what
    /// with the current settings, and how to switch each one off.
    ///
    /// Always in the status buffer, and the view follows: in a busy channel
    /// the answer would otherwise scroll away within seconds.
    fn show_data_flows(&mut self) {
        self.active = 0;
        self.buffers[0].scroll = 0;
        if self.data_flows.is_empty() {
            self.status(NoticeLevel::Warning, self.text().privacy_unknown);
            return;
        }
        self.status(NoticeLevel::Info, self.text().privacy_title);
        let text = self.text();
        // The words are padded to line up, in columns: "on" and "off"
        // differ in length in most languages, and a Korean word is wide.
        let state_width = [text.privacy_on, text.privacy_off]
            .map(|word| word.trim().width())
            .into_iter()
            .max()
            .unwrap_or(0);
        let kind_width = [text.privacy_network, text.privacy_disk]
            .map(UnicodeWidthStr::width)
            .into_iter()
            .max()
            .unwrap_or(0);
        for flow in self.data_flows.clone() {
            let state = crate::render::pad(
                if flow.enabled {
                    text.privacy_on.trim()
                } else {
                    text.privacy_off.trim()
                },
                state_width,
            );
            let kind = crate::render::pad(
                match flow.kind {
                    DataFlowKind::Network => text.privacy_network,
                    DataFlowKind::Disk => text.privacy_disk,
                },
                kind_width,
            );
            let switch = if flow.switch.is_empty() {
                String::new()
            } else {
                format!(" [{}]", flow.switch)
            };
            self.status(
                if flow.enabled {
                    NoticeLevel::Warning
                } else {
                    NoticeLevel::Info
                },
                format!(
                    "  {state} {kind} {}: {}{switch}",
                    // The engine says it in English and names a code; a
                    // frontend that knows the code says it in the
                    // reader's own language.
                    liveside_client::lang::from_engine(text, &flow.party_code, &[], &flow.party),
                    liveside_client::lang::from_engine(text, &flow.code, &flow.args, &flow.detail)
                ),
            );
        }
    }
}

impl App {
    /// Whether anything is waiting: the wheel turns only then.
    pub fn busy(&self) -> bool {
        !self.waiting.is_empty() || self.buffers.iter().any(|buffer| buffer.joining.is_some())
    }

    /// Whether an answer has been owed long enough to be worth a wheel
    /// of its own in the header. Most are answered in a moment, and a
    /// wheel that flashes at every sent line is worse than none.
    pub fn working(&self) -> bool {
        let now = std::time::Instant::now();
        self.waiting
            .values()
            .any(|asked| now.duration_since(*asked) >= std::time::Duration::from_millis(300))
    }

    /// Notes a question put to the engine. Anything owed for a minute is
    /// forgotten: an answer that never came must not leave the wheel
    /// turning for the rest of the evening.
    pub fn asked(&mut self, id: liveside_proto::RequestId) {
        let now = std::time::Instant::now();
        self.waiting
            .retain(|_, asked| now.duration_since(*asked) < std::time::Duration::from_secs(60));
        self.waiting.insert(id, now);
    }

    /// While only the status buffer is on screen, opens the first channel
    /// that has arrived, in the order of the tabs, as the window does: a
    /// channel joined from the status buffer is shown, and channels
    /// arriving one by one at start do not take turns in front. A tab
    /// still waiting before it decides nothing yet. With a channel already
    /// open, the reader stays where they are.
    fn open_first_ready(&mut self) {
        if self.active != 0 {
            return;
        }
        for (i, buffer) in self.buffers.iter().enumerate().skip(1) {
            if buffer.joining.is_some() {
                return;
            }
            if buffer.channel.is_some() {
                self.active = i;
                return;
            }
        }
    }

    /// The tab waiting for this channel, if there is one.
    fn joining_buffer(&self, login: &Login) -> Option<usize> {
        self.buffers
            .iter()
            .position(|buffer| buffer.joining.as_ref() == Some(login))
    }

    /// The buffer of the thread with this person, if it is open.
    fn whisper_buffer(&self, login: &Login) -> Option<usize> {
        self.buffers
            .iter()
            .position(|buffer| buffer.whisper.as_ref() == Some(login))
    }

    /// Clips the channel on screen, if there is one.
    fn clip(&mut self) -> Vec<Command> {
        let Some(channel) = self.buffer().channel.clone() else {
            self.local(NoticeLevel::Error, self.text().not_in_channel);
            return Vec::new();
        };
        vec![Command::CreateClip { channel }]
    }
}

/// The day out of a Twitch timestamp.
fn day_of(stamp: &str) -> String {
    stamp.split('T').next().unwrap_or(stamp).to_owned()
}

/// The day and the hour, which is what a schedule is read for.
fn when_of(stamp: &str) -> String {
    match stamp.split_once('T') {
        Some((day, rest)) => {
            let time: String = rest.chars().take(5).collect();
            format!("{day} {time}")
        }
        None => stamp.to_owned(),
    }
}

/// How a poll stands, as one line: every choice with its count, in the
/// order they were offered.
fn poll_standing(poll: &liveside_proto::model::Poll) -> String {
    poll.choices
        .iter()
        .map(|choice| format!("{} {}", choice.title, choice.votes))
        .collect::<Vec<_>>()
        .join(", ")
}

/// How a prediction stands, as one line: each outcome numbered, because
/// /payout takes that number, with who has bet what on it.
fn prediction_standing(
    prediction: &liveside_proto::model::Prediction,
    strings: &'static liveside_client::lang::Strings,
) -> String {
    prediction
        .outcomes
        .iter()
        .enumerate()
        .map(|(i, outcome)| {
            // Numbered, because /payout takes the number: nobody is
            // going to type a Twitch id while the chat waits.
            format!(
                "{}. {} — {}",
                i + 1,
                outcome.title,
                liveside_client::lang::fill(
                    strings.pred_points,
                    &[
                        ("users", &outcome.users.to_string()),
                        ("points", &outcome.points.to_string()),
                    ],
                )
            )
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn poll_total(poll: &liveside_proto::model::Poll) -> u32 {
    poll.choices.iter().map(|choice| choice.votes).sum()
}

/// The terminal's answer to /blockedterms: how many, and which, with when
/// a term added for a while stops being blocked.
fn blocked_terms_line(
    text: &'static liveside_client::lang::Strings,
    title: &str,
    terms: &[liveside_proto::model::BlockedTerm],
) -> String {
    let channel = title.trim_start_matches('#');
    if terms.is_empty() {
        return liveside_client::lang::from_engine(
            text,
            "blockterm.none",
            &[("channel".to_owned(), channel.to_owned())],
            &format!("#{channel} blocks no terms"),
        );
    }
    let list = terms
        .iter()
        .map(|term| match &term.expires_at {
            Some(until) => format!(
                "{} (→ {})",
                term.text,
                until.split('T').next().unwrap_or(until)
            ),
            None => term.text.clone(),
        })
        .collect::<Vec<_>>()
        .join(", ");
    let count = terms.len().to_string();
    liveside_client::lang::from_engine(
        text,
        "blockterm.list",
        &[
            ("count".to_owned(), count.clone()),
            ("terms".to_owned(), list.clone()),
        ],
        &format!("{count} blocked terms: {list}"),
    )
}
/// A line as plain text, the way it reads on screen, for searching.
pub fn line_text(line: &Line, strings: &'static liveside_client::lang::Strings) -> String {
    let fragments =
        |fragments: &[Fragment]| -> String { fragments.iter().map(Fragment::text).collect() };
    match &line.content {
        LineContent::Message(message) => format!(
            "{}: {}",
            message.sender.display_name,
            fragments(&message.fragments)
        ),
        LineContent::UserNotice {
            system_text,
            message,
        } => match message {
            Some(message) => format!("{system_text} {}", fragments(&message.fragments)),
            None => system_text.clone(),
        },
        LineContent::Notice {
            text, code, args, ..
        } => liveside_client::lang::from_engine(strings, code, args, text),
    }
}

/// Where `needle` is in `text`, ignoring case, as byte ranges of `text`.
pub fn find_ignoring_case(text: &str, needle: &str) -> Vec<std::ops::Range<usize>> {
    let needle: Vec<char> = needle.chars().flat_map(char::to_lowercase).collect();
    if needle.is_empty() {
        return Vec::new();
    }
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let mut found = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        // Compared a letter at a time, lowercased, so the ranges stay in
        // the original text whatever lowercasing does to its length.
        let mut want = needle.iter();
        let mut j = i;
        let mut matched = true;
        'letters: while let Some(&wanted) = want.next() {
            let Some(&(_, c)) = chars.get(j) else {
                matched = false;
                break;
            };
            let mut lower = c.to_lowercase();
            let first = lower.next();
            if first != Some(wanted) {
                matched = false;
                break;
            }
            for rest in lower {
                if want.next() != Some(&rest) {
                    matched = false;
                    break 'letters;
                }
            }
            j += 1;
        }
        if matched {
            let start = chars[i].0;
            let end = chars.get(j).map_or(text.len(), |&(at, _)| at);
            found.push(start..end);
            i = j;
        } else {
            i += 1;
        }
    }
    found
}

fn contains_ignoring_case(text: &str, needle: &str) -> bool {
    !find_ignoring_case(text, needle).is_empty()
}

/// Whether to draw for a light background: as the settings say, or, left
/// to the system, as the terminal said. A terminal that could not say is
/// taken to be dark, as most are.
pub fn light_background(theme: liveside_client::config::Theme, asked: Option<bool>) -> bool {
    use liveside_client::config::Theme;
    match theme {
        Theme::Light => true,
        Theme::Dark => false,
        Theme::System => asked.unwrap_or(false),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use liveside_proto::model::{Channel, RoomModes};
    use liveside_proto::{DataFlow, DataFlowKind};

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn ctrl(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
    }

    fn type_text(app: &mut App, text: &str) {
        for c in text.chars() {
            app.on_key(key(KeyCode::Char(c)));
        }
    }

    /// An app with the status buffer and one joined channel, active.
    fn app_with_channel() -> App {
        let mut app = App::default();
        joined(&mut app, "somechannel", "1");
        app.active = 1;
        app
    }

    #[test]
    fn ctrl_l_clears_the_buffer_on_screen_and_sends_nothing() {
        let mut app = app_with_channel();
        let channel: UserId = "1".into();
        for i in 0..3 {
            app.event(Event::Line {
                channel: channel.clone(),
                line: message_with_id(&format!("m{i}"), "alice", "hello"),
            });
        }
        app.buffers[1].scroll = 2;
        assert!(app.on_key(ctrl('l')).is_empty(), "nothing goes to Twitch");
        assert!(app.buffers[1].lines.is_empty());
        assert_eq!(app.buffers[1].scroll, 0);
        // What comes next is shown as usual, and other buffers keep theirs.
        app.event(Event::Line {
            channel,
            line: message_with_id("m3", "alice", "again"),
        });
        assert_eq!(app.buffers[1].lines.len(), 1);
    }

    #[test]
    fn a_suspicion_marks_the_message_it_is_about() {
        // The message comes first here, which is the order the window
        // cannot fix afterwards by itself.
        let mut app = app_with_channel();
        let channel: UserId = "1".into();
        app.event(Event::Line {
            channel: channel.clone(),
            line: message_with_id("m1", "alice", "hello"),
        });
        app.event(Event::SuspiciousMessage {
            channel,
            message: "m1".into(),
            user: liveside_proto::model::User {
                id: "7".into(),
                login: "alice".into(),
                display_name: "alice".to_owned(),
            },
            suspicion: Box::new(liveside_proto::model::Suspicion {
                status: liveside_proto::model::SuspicionStatus::Monitored,
                reasons: vec!["ban_evader".to_owned()],
                evasion: "likely".to_owned(),
                shared_bans: 0,
            }),
        });
        assert!(
            app.buffers[1]
                .lines
                .iter()
                .any(|line| line.flags.suspicion.is_some()),
            "the message was not marked"
        );
    }

    fn notice_line(text: &str) -> Line {
        Line {
            time: Timestamp::now(),
            content: LineContent::Notice {
                level: NoticeLevel::Info,
                text: text.to_owned(),
                code: String::new(),
                args: Vec::new(),
            },
            flags: LineFlags::default(),
        }
    }

    /// Renders and returns the screen as plain text lines.
    fn screen(app: &App, width: u16, height: u16) -> Vec<String> {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal
            .draw(|frame| {
                crate::render::draw(frame, app);
            })
            .unwrap();
        let buffer = terminal.backend().buffer().clone();
        (0..buffer.area.height)
            .map(|y| {
                (0..buffer.area.width)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
                    .trim_end()
                    .to_owned()
            })
            .collect()
    }

    fn joined(app: &mut App, login: &str, id: &str) {
        app.apply(ServerMessage::Event {
            seq: liveside_proto::Seq(1),
            event: Event::ChannelJoined {
                channel: Channel {
                    id: id.into(),
                    login: login.into(),
                    display_name: login.into(),
                },
                modes: RoomModes::default(),
            },
        });
    }

    #[test]
    fn typing_and_editing() {
        let mut app = App::default();
        type_text(&mut app, "hello wrold");
        app.on_key(ctrl('w'));
        type_text(&mut app, "world");
        assert_eq!(app.input.text, "hello world");
        app.on_key(key(KeyCode::Left));
        app.on_key(key(KeyCode::Backspace));
        assert_eq!(app.input.text, "hello word");
        app.on_key(key(KeyCode::Home));
        app.on_key(key(KeyCode::Delete));
        assert_eq!(app.input.text, "ello word");
        app.on_key(ctrl('u'));
        assert_eq!(app.input.text, "");
    }

    #[test]
    fn non_ascii_editing() {
        let mut app = App::default();
        type_text(&mut app, "příliš");
        app.on_key(key(KeyCode::Backspace));
        assert_eq!(app.input.text, "příli");
        app.on_key(key(KeyCode::Left));
        app.on_key(key(KeyCode::Left));
        app.on_key(key(KeyCode::Char('X')));
        assert_eq!(app.input.text, "příXli", "two lefts skip 'i' and 'l'");
    }

    #[test]
    fn commands_without_a_channel() {
        let mut app = App::default();
        type_text(&mut app, "hello");
        assert!(app.on_key(key(KeyCode::Enter)).is_empty());
        assert_eq!(app.buffers[0].lines.len(), 1, "expected an error line");

        type_text(&mut app, "/login");
        assert_eq!(app.on_key(key(KeyCode::Enter)), [Command::Login]);

        type_text(&mut app, "/join #abc");
        assert_eq!(
            app.on_key(key(KeyCode::Enter)),
            [Command::Join {
                channel: "#abc".into()
            }]
        );
        // A list of channels is split when there is no channel to ask.
        type_text(&mut app, "/join one, two three");
        assert_eq!(
            app.on_key(key(KeyCode::Enter)),
            [
                Command::Join {
                    channel: "one".into()
                },
                Command::Join {
                    channel: "two".into()
                },
                Command::Join {
                    channel: "three".into()
                },
            ]
        );

        type_text(&mut app, "/quit");
        assert!(app.on_key(key(KeyCode::Enter)).is_empty());
        assert!(app.quit);
    }

    #[test]
    fn text_and_slash_commands_go_to_the_engine() {
        let mut app = App::default();
        joined(&mut app, "abc", "1");
        assert_eq!(app.active, 1, "should follow the join");

        type_text(&mut app, "hello");
        assert_eq!(
            app.on_key(key(KeyCode::Enter)),
            [Command::Input {
                channel: "1".into(),
                text: "hello".into(),
                reply_to: None
            }]
        );
        // Unknown commands are the engine's business, not ours.
        type_text(&mut app, "/timeout bob 10m");
        assert_eq!(
            app.on_key(key(KeyCode::Enter)),
            [Command::Input {
                channel: "1".into(),
                text: "/timeout bob 10m".into(),
                reply_to: None
            }]
        );
        // In a channel, even /join goes to the engine, which knows lists.
        type_text(&mut app, "/join x,y");
        assert_eq!(
            app.on_key(key(KeyCode::Enter)),
            [Command::Input {
                channel: "1".into(),
                text: "/join x,y".into(),
                reply_to: None
            }]
        );
        // Empty input sends nothing.
        assert!(app.on_key(key(KeyCode::Enter)).is_empty());
    }

    #[test]
    fn switching_and_parting_buffers() {
        let mut app = App::default();
        joined(&mut app, "abc", "1");
        joined(&mut app, "def", "2");
        assert_eq!(app.active, 1, "the first to arrive is opened, and stays");
        app.on_key(key(KeyCode::Tab));
        assert_eq!(app.active, 2);
        app.on_key(key(KeyCode::Tab));
        assert_eq!(app.active, 0, "wraps around to status");
        app.on_key(key(KeyCode::BackTab));
        assert_eq!(app.active, 2);

        app.apply(ServerMessage::Event {
            seq: liveside_proto::Seq(2),
            event: Event::ChannelParted {
                channel: "2".into(),
            },
        });
        assert_eq!(app.buffers.len(), 2);
        assert_eq!(app.active, 1);
    }

    #[test]
    fn a_channel_joined_from_the_status_buffer_is_opened() {
        // Typed /join on the status buffer: the channel is shown, which
        // the waiting tab used to prevent.
        let mut app = App::default();
        app.event(Event::ChannelJoining {
            login: "quiet".into(),
        });
        assert_eq!(app.active, 0);
        joined(&mut app, "quiet", "7");
        assert_eq!(app.buffer().title, "#quiet");

        // At start, the first in the order of the tabs, not the first
        // to be looked up: one still waiting above holds it back.
        let mut app = App::default();
        for login in ["first", "second"] {
            app.event(Event::ChannelJoining {
                login: login.into(),
            });
        }
        joined(&mut app, "second", "2");
        assert_eq!(app.active, 0, "#first is still on its way");
        joined(&mut app, "first", "1");
        assert_eq!(app.buffer().title, "#first");

        // ...unless it never comes.
        let mut app = App::default();
        for login in ["gone", "here"] {
            app.event(Event::ChannelJoining {
                login: login.into(),
            });
        }
        joined(&mut app, "here", "2");
        app.event(Event::ChannelJoinFailed {
            login: "gone".into(),
        });
        assert_eq!(app.buffer().title, "#here");

        // With a channel open, another arriving leaves the reader there.
        let mut app = app_with_channel();
        joined(&mut app, "other", "3");
        assert_eq!(app.buffer().title, "#somechannel");
    }

    #[test]
    fn snapshot_keeps_the_active_channel() {
        let mut app = App::default();
        joined(&mut app, "abc", "1");
        joined(&mut app, "def", "2");
        assert_eq!(app.buffer().title, "#abc");

        let snapshot = Snapshot {
            account: None,
            backend: "twitch".into(),
            connection: ConnectionState::Connected,
            channels: vec![
                liveside_proto::ChannelState {
                    channel: Channel {
                        id: "2".into(),
                        login: "def".into(),
                        display_name: "def".into(),
                    },
                    modes: RoomModes::default(),
                    lines: Vec::new(),
                    chatters: Vec::new(),
                    live: false,
                    held: Vec::new(),
                    unban_requests: Vec::new(),
                    poll: None,
                    prediction: None,
                    redemptions: Vec::new(),
                    shield: false,
                    stats: None,
                    viewers: Default::default(),
                },
                liveside_proto::ChannelState {
                    channel: Channel {
                        id: "1".into(),
                        login: "abc".into(),
                        display_name: "abc".into(),
                    },
                    modes: RoomModes::default(),
                    lines: Vec::new(),
                    chatters: Vec::new(),
                    live: false,
                    held: Vec::new(),
                    unban_requests: Vec::new(),
                    poll: None,
                    prediction: None,
                    redemptions: Vec::new(),
                    shield: false,
                    stats: None,
                    viewers: Default::default(),
                },
            ],
            data_flows: Vec::new(),
            whispers: Vec::new(),
            link_reports: Vec::new(),
            highlighted: Vec::new(),
            ignored: Vec::new(),
            bots: Vec::new(),
            joining: Vec::new(),
        };
        app.apply(ServerMessage::Snapshot {
            seq: liveside_proto::Seq(9),
            snapshot,
        });
        // Channels came back in a different order; we still show #abc.
        assert_eq!(app.buffer().title, "#abc");
        assert_eq!(app.buffers.len(), 3);
    }
    #[test]
    fn moderated_channels_are_asked_for_and_the_joined_ones_marked() {
        let mut app = app_with_channel();
        app.input.text = "/moderated".to_owned();
        app.input.cursor = app.input.text.len();
        assert_eq!(
            app.on_key(key(KeyCode::Enter)),
            [Command::ShowModeratedChannels]
        );
        let channel = |id: &str, login: &str| liveside_proto::model::Channel {
            id: id.into(),
            login: login.into(),
            display_name: login.into(),
        };
        app.event(Event::ModeratedChannels {
            channels: vec![channel("9", "other"), channel("8", "somechannel")],
        });
        let LineContent::Notice { text, .. } = &app.buffers[1].lines.back().unwrap().content else {
            panic!("a notice");
        };
        assert!(text.contains("other, somechannel ✓"), "{text}");
    }

    #[test]
    fn followed_channels_say_what_streams_and_list_the_rest() {
        use liveside_proto::model::{Channel, FollowedChannel, LiveStream, StreamStats};
        let mut app = app_with_channel();
        app.input.text = "/followed".to_owned();
        app.input.cursor = app.input.text.len();
        assert_eq!(
            app.on_key(key(KeyCode::Enter)),
            [Command::ShowFollowedChannels]
        );
        let channel = |id: &str, login: &str| Channel {
            id: id.into(),
            login: login.into(),
            display_name: login.into(),
        };
        let started = Timestamp(Timestamp::now().0 - (2 * 60 + 5) * 60_000 - 30_000);
        app.event(Event::FollowedChannels {
            channels: vec![
                FollowedChannel {
                    channel: channel("1", "somechannel"),
                    live: Some(LiveStream {
                        title: "Cozy".into(),
                        category: "Art".into(),
                        stats: StreamStats {
                            viewers: 1234,
                            started,
                        },
                    }),
                },
                // No category: the line goes without it, not with an
                // empty gap.
                FollowedChannel {
                    channel: channel("2", "quiet"),
                    live: Some(LiveStream {
                        title: "Talk".into(),
                        category: String::new(),
                        stats: StreamStats {
                            viewers: 3,
                            started,
                        },
                    }),
                },
                FollowedChannel {
                    channel: channel("9", "other"),
                    live: None,
                },
            ],
        });
        let said: Vec<String> = app.buffers[1]
            .lines
            .iter()
            .rev()
            .take(4)
            .rev()
            .map(|line| match &line.content {
                LineContent::Notice { text, .. } => text.clone(),
                _ => panic!("a notice"),
            })
            .collect();
        assert_eq!(
            said,
            [
                "You follow 3, 2 live (join with /join):",
                "● somechannel ✓ · Art · Cozy · viewers: 1\u{a0}234 · live 2:05",
                "● quiet · Talk · viewers: 3 · live 2:05",
                "Offline (1): other",
            ]
        );

        app.event(Event::FollowedChannels {
            channels: Vec::new(),
        });
        let LineContent::Notice { text, .. } = &app.buffers[1].lines.back().unwrap().content else {
            panic!("a notice");
        };
        assert_eq!(text, "You follow no channel");
    }

    #[test]
    fn a_followed_channel_starting_is_said_in_the_status_buffer() {
        use liveside_proto::model::{Channel, FollowedChannel, LiveStream, StreamStats};
        let mut app = app_with_channel();
        let live = vec![FollowedChannel {
            channel: Channel {
                id: "7".into(),
                login: "quiet_corner".into(),
                display_name: "Quiet_Corner".into(),
            },
            live: Some(LiveStream {
                title: "Morning coffee".into(),
                category: "Just Chatting".into(),
                stats: StreamStats {
                    viewers: 12,
                    started: Timestamp::now(),
                },
            }),
        }];
        let before = app.buffers[0].lines.len();
        // The first look is how things stand: nothing to say.
        app.event(Event::FollowedStreams {
            live: live.clone(),
            started: Vec::new(),
        });
        assert_eq!(app.buffers[0].lines.len(), before);
        app.event(Event::FollowedStreams {
            live,
            started: vec!["7".into()],
        });
        let LineContent::Notice { text, .. } = &app.buffers[0].lines.back().unwrap().content else {
            panic!("a notice");
        };
        assert_eq!(
            text,
            "Quiet_Corner is live: Just Chatting · Morning coffee (/join quiet_corner)"
        );
    }

    #[test]
    fn a_clicked_notification_opens_its_buffer_or_joins_the_followed_channel() {
        use liveside_client::notify::Place;
        let mut app = app_with_channel();
        joined(&mut app, "other", "2");
        app.active = 0;
        assert!(app.go_to(Place::Channel("2".into())).is_empty());
        assert_eq!(app.buffers[app.active].title, "#other");
        // A followed channel already joined is just opened.
        assert!(
            app.go_to(Place::Followed(Some("somechannel".into())))
                .is_empty()
        );
        assert_eq!(app.buffers[app.active].title, "#somechannel");
        // Several at once: the list of them.
        assert_eq!(
            app.go_to(Place::Followed(None)),
            [Command::ShowFollowedChannels]
        );

        // One not joined is joined, and opened when it arrives, even
        // though the tab for it came first and would stay in the back.
        assert_eq!(
            app.go_to(Place::Followed(Some("quiet".into()))),
            [Command::Join {
                channel: "quiet".into()
            }]
        );
        app.event(Event::ChannelJoining {
            login: "quiet".into(),
        });
        assert_eq!(app.buffers[app.active].title, "#somechannel");
        joined(&mut app, "quiet", "7");
        assert_eq!(app.buffers[app.active].title, "#quiet");
        // Only that once: the next channel to arrive does not take over.
        app.event(Event::ChannelJoining {
            login: "later".into(),
        });
        joined(&mut app, "later", "8");
        assert_eq!(app.buffers[app.active].title, "#quiet");
    }

    #[test]
    fn the_lists_of_what_lit_up_and_was_hidden_are_buffers_that_keep_up() {
        let mut app = app_with_channel();
        let mut lit = message_with_id("m-hello", "alice", "hello");
        lit.flags.highlighted = true;
        app.event(Event::Line {
            channel: "1".into(),
            line: lit,
        });
        app.event(Event::Ignored {
            channel: "1".into(),
            line: message_with_id("m-spam", "bot", "spam"),
        });
        assert_eq!(
            app.buffers[1].lines.len(),
            1,
            "the hidden one is not in the channel"
        );

        app.input.text = "/ignored".to_owned();
        app.input.cursor = app.input.text.len();
        assert!(app.on_key(key(KeyCode::Enter)).is_empty());
        let shown = |app: &App| -> Vec<String> {
            app.buffer()
                .lines
                .iter()
                .map(|line| match &line.content {
                    LineContent::Message(m) => format!(
                        "#{} {}",
                        m.shared_source.as_ref().unwrap().channel.login,
                        m.plain_text()
                    ),
                    LineContent::Notice { text, .. } => text.clone(),
                    _ => String::new(),
                })
                .collect()
        };
        assert_eq!(app.buffer().title, "!ignored");
        assert_eq!(shown(&app), ["#somechannel spam"]);
        // Another hidden line goes into the open list.
        app.event(Event::Ignored {
            channel: "1".into(),
            line: message_with_id("m-more", "bot", "more spam"),
        });
        assert_eq!(shown(&app), ["#somechannel spam", "#somechannel more spam"]);

        app.input.text = "/highlights".to_owned();
        app.input.cursor = app.input.text.len();
        app.on_key(key(KeyCode::Enter));
        assert_eq!(shown(&app), ["#somechannel hello"]);
        let buffers = app.buffers.len();
        app.input.text = "/close".to_owned();
        app.input.cursor = app.input.text.len();
        assert!(
            app.on_key(key(KeyCode::Enter)).is_empty(),
            "nothing for the engine"
        );
        assert_eq!(app.buffers.len(), buffers - 1);
    }

    #[test]
    fn ignoring_somebody_is_a_saved_rule_handed_to_the_engine() {
        let path =
            std::env::temp_dir().join(format!("liveside-tui-ignore-{}.toml", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let mut app = app_with_channel();
        app.settings = liveside_client::settings::Settings::new(
            liveside_client::Config::default(),
            path.clone(),
            liveside_client::lang::Language::English,
            liveside_client::settings::Frontend::Terminal,
        );
        app.input.text = "/ignore @NightBot".to_owned();
        app.input.cursor = app.input.text.len();
        let commands = app.on_key(key(KeyCode::Enter));
        let saved = liveside_client::Config::load(&path).unwrap();
        std::fs::remove_file(&path).unwrap();
        assert_eq!(saved.ignore.rules, ["user:nightbot"]);
        assert!(matches!(
            commands.as_slice(),
            [Command::SetRules { ignore, .. }] if ignore == &["user:nightbot".to_owned()]
        ));
        let LineContent::Notice { text, .. } = &app.buffers[1].lines.back().unwrap().content else {
            panic!("a notice");
        };
        assert!(text.starts_with("nightbot is ignored everywhere"), "{text}");
    }

    #[test]
    fn a_custom_command_turns_into_what_it_says_before_anything_else() {
        let mut app = app_with_channel();
        let mut config = liveside_client::Config::default();
        config.commands.custom = vec![
            "/slap /me slaps $$1 with a trout".into(),
            "/where /echo here: $(chan) $1-".into(),
            "/hi#otherchannel Hello other!".into(),
            "/hi Hello $(chan)!".into(),
        ];
        app.settings = liveside_client::settings::Settings::new(
            config,
            std::env::temp_dir().join("liveside-tui-custom-never-written.toml"),
            liveside_client::lang::Language::English,
            liveside_client::settings::Frontend::Terminal,
        );
        let typed = |app: &mut App, line: &str| {
            app.input.text = line.to_owned();
            app.input.cursor = line.len();
            app.on_key(key(KeyCode::Enter))
        };
        let last = |app: &App| match &app.buffers[1].lines.back().unwrap().content {
            LineContent::Notice { text, .. } => text.clone(),
            _ => "a message".to_owned(),
        };
        assert!(matches!(
            typed(&mut app, "/slap bot").as_slice(),
            [Command::Input { text, .. }] if text == "/me slaps bot with a trout"
        ));
        assert!(typed(&mut app, "/slap").is_empty());
        assert_eq!(last(&app), "/slap needs $1");
        // What it makes is answered as typed: /echo is the frontend's.
        assert!(typed(&mut app, "/where a b").is_empty());
        assert_eq!(last(&app), "here: somechannel a b");
        assert!(matches!(
            typed(&mut app, "/hi").as_slice(),
            [Command::Input { text, .. }] if text == "Hello somechannel!"
        ));
        // Not a custom command: as before.
        assert!(matches!(
            typed(&mut app, "/ban troll").as_slice(),
            [Command::Input { text, .. }] if text == "/ban troll"
        ));
    }

    #[test]
    fn in_a_whisper_thread_text_is_whispered_and_a_command_stays_one() {
        let mut app = app_with_channel();
        let mut buffer = Buffer::new(None, "@eve".into());
        buffer.whisper = Some("eve".into());
        app.buffers.push(buffer);
        app.active = app.buffers.len() - 1;
        let typed = |app: &mut App, line: &str| {
            app.input.text = line.to_owned();
            app.input.cursor = line.len();
            app.on_key(key(KeyCode::Enter))
        };
        let whispered = |sent: &[Command]| match sent {
            [Command::SendWhisper { user, text }] => Some((user.to_string(), text.clone())),
            _ => None,
        };
        assert_eq!(
            whispered(&typed(&mut app, "hi")),
            Some(("eve".into(), "hi".into()))
        );
        assert_eq!(
            whispered(&typed(&mut app, "//shrug")),
            Some(("eve".into(), "/shrug".into()))
        );
        assert_eq!(whispered(&typed(&mut app, "/echo just me")), None);
    }

    #[test]
    fn tab_completes_the_frontends_own_and_custom_commands() {
        let mut app = app_with_channel();
        let mut config = liveside_client::Config::default();
        config.commands.custom = vec!["/slap x".into()];
        app.settings = liveside_client::settings::Settings::new(
            config,
            std::env::temp_dir().join("liveside-tui-complete-never-written.toml"),
            liveside_client::lang::Language::English,
            liveside_client::settings::Frontend::Terminal,
        );
        for (typed, done) in [
            ("/vie", "/viewers "),
            ("/sl", "/slap "),
            ("/foll", "/followed "),
        ] {
            app.input.text = typed.to_owned();
            app.input.cursor = typed.len();
            app.on_key(key(KeyCode::Tab));
            assert_eq!(app.input.text, done, "{typed}");
        }
    }

    #[test]
    fn every_command_offered_for_completion_is_answered_here() {
        // The engine's are checked in its own tests; these must not fall
        // through to it as unknown commands.
        for name in App::OWN_COMMANDS {
            let mut app = app_with_channel();
            app.input.text = format!("/{name}");
            app.input.cursor = app.input.text.len();
            let sent = app.on_key(key(KeyCode::Enter));
            assert!(
                !sent.iter().any(|c| matches!(c, Command::Input { text, .. } if text.starts_with(&format!("/{name}")) && *name != "close")),
                "/{name} went to the engine: {sent:?}"
            );
        }
    }

    #[test]
    fn a_hotkey_runs_its_line_with_the_selected_user() {
        let mut app = app_with_channel();
        let mut config = liveside_client::Config::default();
        config.commands.custom = vec!["/where /echo here: $(chan)".into()];
        config.hotkeys.bindings = vec![
            "F5 /where".into(),
            "Ctrl+Shift+B /ban $$1 spam".into(),
            "Ctrl+C /echo never".into(),
        ];
        app.settings = liveside_client::settings::Settings::new(
            config,
            std::env::temp_dir().join("liveside-tui-hotkeys-never-written.toml"),
            liveside_client::lang::Language::English,
            liveside_client::settings::Frontend::Terminal,
        );
        let last = |app: &App| match &app.buffers[1].lines.back().unwrap().content {
            LineContent::Notice { text, .. } => text.clone(),
            _ => "a message".to_owned(),
        };
        app.input.text = "half typed".to_owned();
        app.input.cursor = app.input.text.len();
        assert!(app.on_key(key(KeyCode::F(5))).is_empty());
        assert_eq!(
            last(&app),
            "here: somechannel",
            "through the custom command"
        );
        assert_eq!(app.input.text, "half typed", "the input is left alone");

        let ban = KeyEvent::new(
            KeyCode::Char('B'),
            KeyModifiers::CONTROL | KeyModifiers::SHIFT,
        );
        assert!(app.on_key(ban).is_empty());
        assert_eq!(
            last(&app),
            "Ctrl+Shift+B needs $1: select a message first",
            "nobody selected"
        );
        app.buffers[1].push(message_with_id("m-troll", "troll", "buy followers"));
        app.buffers[1].selected = Some("m-troll".into());
        assert!(matches!(
            app.on_key(ban).as_slice(),
            [Command::Input { text, .. }] if text == "/ban troll spam"
        ));
        // Quitting is not taken over.
        app.on_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
        assert!(app.quit);
    }

    #[test]
    fn viewers_open_a_graph_that_esc_puts_away() {
        let mut app = app_with_channel();
        app.input.text = "/viewers".to_owned();
        app.input.cursor = app.input.text.len();
        assert!(
            app.on_key(key(KeyCode::Enter)).is_empty(),
            "nothing for the engine"
        );
        assert_eq!(app.graph, Some("1".into()));
        // Typing goes nowhere while it is up.
        app.on_key(key(KeyCode::Char('x')));
        assert!(app.input.text.is_empty());
        app.on_key(key(KeyCode::Esc));
        assert_eq!(app.graph, None);
        // Not in the status buffer.
        app.active = 0;
        app.input.text = "/viewers".to_owned();
        app.input.cursor = app.input.text.len();
        app.on_key(key(KeyCode::Enter));
        assert_eq!(app.graph, None);
    }

    #[test]
    fn a_long_list_is_counted_and_only_begun_in_a_line() {
        use liveside_proto::event::{PeopleList, Person};
        let mut app = app_with_channel();
        let people: Vec<Person> = (0..150)
            .map(|i| Person {
                name: format!("user{i}"),
                detail: "2026-10-15T18:02:11Z".into(),
                gifted_by: String::new(),
            })
            .collect();
        app.event(Event::People {
            channel: "1".into(),
            list: PeopleList::Followers,
            people,
            total: Some(4000),
            points: None,
        });
        let LineContent::Notice { text, .. } = &app.buffers[1].lines.back().unwrap().content else {
            panic!("a notice");
        };
        assert!(
            text.starts_with("Followers: 4000 (the newest 150 shown): user0 (since 2026-10-15)"),
            "{text}"
        );
        assert!(
            text.ends_with("user99 (since 2026-10-15), and 50 more"),
            "{text}"
        );

        // Counted, nobody named.
        app.event(Event::People {
            channel: "1".into(),
            list: PeopleList::Followers,
            people: Vec::new(),
            total: Some(4000),
            points: None,
        });
        let LineContent::Notice { text, .. } = &app.buffers[1].lines.back().unwrap().content else {
            panic!("a notice");
        };
        assert!(
            text.contains("only the channel and its moderators"),
            "{text}"
        );
    }

    #[test]
    fn blocked_terms_are_one_line_with_when_a_term_runs_out() {
        use liveside_proto::model::BlockedTerm;
        let text = liveside_client::lang::Language::English.strings();
        let term = |text: &str, expires: Option<&str>| BlockedTerm {
            id: "1".into(),
            text: text.into(),
            created_at: String::new(),
            expires_at: expires.map(str::to_owned),
        };
        assert_eq!(
            blocked_terms_line(
                text,
                "#abc",
                &[
                    term("spoiler", None),
                    term("*spam*", Some("2026-10-20T18:30:00Z"))
                ]
            ),
            "2 blocked terms: spoiler, *spam* (→ 2026-10-20)"
        );
        assert_eq!(
            blocked_terms_line(text, "#abc", &[]),
            "#abc blocks no terms"
        );
    }

    #[test]
    fn finding_ignores_case_and_keeps_the_original_ranges() {
        assert_eq!(find_ignoring_case("GG gg Gg", "gg"), [0..2, 3..5, 6..8]);
        assert_eq!(
            find_ignoring_case("Šťastná ŠŤASTNÁ", "šťast"),
            [0..7, 11..18]
        );
        assert!(find_ignoring_case("hello", "").is_empty());
        assert!(find_ignoring_case("hel", "hello").is_empty());
    }

    #[test]
    fn search_goes_from_the_newest_match_back_and_selects_it() {
        let mut app = app_with_channel();
        let channel: UserId = "1".into();
        for (id, who, text) in [
            ("m1", "alice", "the schedule is up"),
            ("m2", "bob", "hello"),
            ("m3", "carol", "SCHEDULE again"),
            ("m4", "dave", "bye"),
        ] {
            app.event(Event::Line {
                channel: channel.clone(),
                line: message_with_id(id, who, text),
            });
        }
        app.chat_height = 2;
        app.on_key(ctrl_key(KeyCode::Char('f')));
        for c in "schedule".chars() {
            app.on_key(key(KeyCode::Char(c)));
        }
        let search = app.search.as_ref().expect("searching");
        assert_eq!(search.at, Some((1, 2)));
        assert_eq!(app.buffers[1].selected, Some("m3".into()));
        assert!(app.buffers[1].scroll <= 1, "{}", app.buffers[1].scroll);

        app.on_key(key(KeyCode::Enter));
        assert_eq!(app.search.as_ref().unwrap().at, Some((2, 2)));
        assert_eq!(app.buffers[1].selected, Some("m1".into()));
        // Round to the newest again.
        app.on_key(key(KeyCode::Up));
        assert_eq!(app.buffers[1].selected, Some("m3".into()));

        // A query that matches nothing says so, and Esc ends it all.
        app.on_key(key(KeyCode::Char('x')));
        assert_eq!(app.search.as_ref().unwrap().at, None);
        assert_eq!(app.buffers[1].selected, None);
        app.on_key(key(KeyCode::Esc));
        assert!(app.search.is_none());
        // Typing goes to the input line again.
        app.on_key(key(KeyCode::Char('h')));
        assert_eq!(app.input.text, "h");
    }

    #[test]
    fn link_asks_about_the_newest_link_or_the_selected_message() {
        let mut app = app_with_channel();
        let channel: UserId = "1".into();
        let with_links = |id: &str, urls: &[&str]| {
            let mut line = message_with_id(id, "alice", "see");
            if let LineContent::Message(message) = &mut line.content {
                for url in urls {
                    message.fragments.push(Fragment::Link {
                        text: (*url).to_owned(),
                        url: (*url).to_owned(),
                    });
                }
            }
            line
        };
        app.event(Event::Line {
            channel: channel.clone(),
            line: with_links("m1", &["https://a.cz/", "https://b.cz/"]),
        });
        app.event(Event::Line {
            channel: channel.clone(),
            line: with_links("m2", &["https://c.cz/"]),
        });
        app.event(Event::Line {
            channel,
            line: message_with_id("m3", "bob", "no link here"),
        });
        let run = |app: &mut App, text: &str| {
            app.input.text = text.to_owned();
            app.input.cursor = text.len();
            app.on_key(key(KeyCode::Enter))
        };
        // Without a selection, the newest message that has a link.
        assert_eq!(
            run(&mut app, "/link virustotal"),
            [Command::CheckLink {
                url: "https://c.cz/".into(),
                scan: false
            }]
        );
        // The selected message, its second link, sent for a scan.
        app.buffers[1].selected = Some("m1".into());
        assert_eq!(
            run(&mut app, "/link scan 2"),
            [Command::CheckLink {
                url: "https://b.cz/".into(),
                scan: true
            }]
        );
        assert_eq!(
            run(&mut app, "/link where"),
            [Command::FollowLink {
                url: "https://a.cz/".into()
            }]
        );
        assert!(run(&mut app, "/link scan 3").is_empty());
        assert!(run(&mut app, "/link").is_empty());
    }

    /// A channel with `count` lines, in a window showing `height` of them.
    fn app_with_lines(count: usize, height: usize) -> App {
        let mut app = app_with_channel();
        app.chat_height = height;
        for i in 0..count {
            app.buffers[1].push(notice_line(&format!("line {i}")));
        }
        app
    }

    #[test]
    fn scrolling_stops_with_a_full_screen_of_oldest_lines() {
        let mut app = app_with_lines(50, 5);
        // Paging up past the top must not run into empty space.
        for _ in 0..50 {
            app.on_key(key(KeyCode::PageUp));
        }
        assert_eq!(app.buffer().scroll, 45, "stops one screen from the top");

        // A fast channel pushes the view back one line per message, but
        // only as far as the buffer reaches, which is capped.
        for i in 0..2000 {
            app.buffers[1].push(notice_line(&format!("more {i}")));
        }
        app.apply(ServerMessage::Event {
            seq: liveside_proto::Seq(1),
            event: Event::LoggedOut,
        });
        let buffer = app.buffer();
        assert!(
            buffer.scroll + app.chat_height <= buffer.lines.len(),
            "scroll {} leaves less than {} lines of {}",
            buffer.scroll,
            app.chat_height,
            buffer.lines.len()
        );
        // The screen is full, not one line above an empty view.
        let view = screen(&app, 40, 7);
        let filled = view
            .iter()
            .filter(|row| row.contains("line ") || row.contains("more "))
            .count();
        assert!(filled >= 4, "only {filled} rows of content: {view:?}");
    }

    #[test]
    fn paging_down_near_the_bottom_goes_live() {
        let mut app = app_with_lines(100, 10);
        app.on_key(key(KeyCode::PageUp));
        assert_eq!(app.buffer().scroll, 9, "a screenful minus an overlap");
        // One page back down is the bottom, not a few lines above it: that
        // remainder would keep growing in a busy channel.
        app.on_key(key(KeyCode::PageDown));
        assert_eq!(app.buffer().scroll, 0);

        // From further back it is an ordinary page.
        for _ in 0..4 {
            app.on_key(key(KeyCode::PageUp));
        }
        assert_eq!(app.buffer().scroll, 36);
        app.on_key(key(KeyCode::PageDown));
        assert_eq!(app.buffer().scroll, 27);
    }

    #[test]
    fn sending_returns_to_the_newest_lines() {
        let mut app = app_with_lines(50, 10);
        app.on_key(key(KeyCode::PageUp));
        assert!(app.buffer().scroll > 0);
        type_text(&mut app, "hello");
        app.on_key(key(KeyCode::Enter));
        assert_eq!(app.buffer().scroll, 0, "typing returns to the bottom");
    }

    #[test]
    fn privacy_report_goes_to_the_status_buffer() {
        let mut app = app_with_channel();
        app.apply(ServerMessage::Event {
            seq: liveside_proto::Seq(9),
            event: Event::DataFlows {
                flows: vec![DataFlow {
                    party: "7TV".into(),
                    party_code: String::new(),
                    kind: DataFlowKind::Network,
                    detail: "Emotes of every channel joined".into(),
                    code: String::new(),
                    args: Vec::new(),
                    enabled: true,
                    switch: "--no-third-party-emotes".into(),
                }],
            },
        });
        // Typed in a channel, because that is where the user is.
        app.active = 1;
        type_text(&mut app, "/privacy");
        assert!(app.on_key(key(KeyCode::Enter)).is_empty(), "stays local");

        // The report lands in status, and the view follows it there: in a
        // busy channel it would otherwise be gone in seconds.
        assert_eq!(app.active, 0);
        let status: Vec<String> = app.buffers[0]
            .lines
            .iter()
            .filter_map(|line| match &line.content {
                LineContent::Notice { text, .. } => Some(text.clone()),
                _ => None,
            })
            .collect();
        assert!(
            status
                .iter()
                .any(|t| t.contains("7TV") && t.contains("--no-third-party-emotes")),
            "{status:?}"
        );
        assert!(app.buffers[1].lines.iter().all(|line| !matches!(
            &line.content,
            LineContent::Notice { text, .. } if text.contains("7TV")
        )));
    }
    /// A line from `sender` with one emote, as the engine delivers it.
    fn message_with_emote(sender: &str, code: &str) -> Line {
        use liveside_proto::model::{ChatMessage, EmoteProvider, EmoteRef, MessageKind, User};
        Line {
            time: Timestamp::now(),
            content: LineContent::Message(ChatMessage {
                id: "m".into(),
                sender: User {
                    id: "7".into(),
                    login: sender.to_lowercase().into(),
                    display_name: sender.to_owned(),
                },
                color: None,
                badges: Vec::new(),
                fragments: vec![Fragment::Emote {
                    emote: EmoteRef {
                        provider: EmoteProvider::SevenTv,
                        id: "1".into(),
                        code: code.to_owned(),
                        animated: false,
                        zero_width: false,
                    },
                }],
                kind: MessageKind::Text,
                bits: None,
                reply: None,
                shared_source: None,
            }),
            flags: LineFlags::default(),
        }
    }

    #[test]
    fn tab_completes_names_from_the_channel() {
        let mut app = app_with_channel();
        app.buffers[1].push(message_with_emote("Marigold_cz", "orbitGrin"));
        app.buffers[1].push(message_with_emote("Marina13", "orbitWave"));

        type_text(&mut app, "@mar");
        app.on_key(key(KeyCode::Tab));
        // Newest first: the person who just wrote.
        assert_eq!(app.input.text, "Marina13: ");
        // Tab again cycles to the next match, Shift-Tab goes back.
        app.on_key(key(KeyCode::Tab));
        assert_eq!(app.input.text, "Marigold_cz: ");
        app.on_key(key(KeyCode::BackTab));
        assert_eq!(app.input.text, "Marina13: ");
        assert_eq!(app.input.cursor, app.input.text.len());
    }

    #[test]
    fn tab_completes_emotes_and_commands() {
        let mut app = app_with_channel();
        app.buffers[1].push(message_with_emote("someone", "orbitGrin"));

        // Mid-line, so no colon and no name to prefer.
        type_text(&mut app, "hey orbitgr");
        app.on_key(key(KeyCode::Tab));
        assert_eq!(app.input.text, "hey orbitGrin ");

        // Commands come from the engine's own list.
        app.input.take();
        type_text(&mut app, "/tim");
        app.on_key(key(KeyCode::Tab));
        assert_eq!(app.input.text, "/timeout ");

        // Nothing matching leaves the typed word alone.
        app.input.take();
        type_text(&mut app, "zzzz");
        app.on_key(key(KeyCode::Tab));
        assert_eq!(app.input.text, "zzzz");
    }

    #[test]
    fn tab_still_switches_buffers_on_an_empty_line() {
        let mut app = app_with_channel();
        joined(&mut app, "other", "2");
        app.active = 1;
        app.on_key(key(KeyCode::Tab));
        assert_eq!(app.active, 2, "empty line: Tab keeps switching buffers");
    }

    #[test]
    fn up_and_down_walk_through_sent_lines() {
        let mut app = app_with_channel();
        for text in ["first", "second"] {
            type_text(&mut app, text);
            app.on_key(key(KeyCode::Enter));
        }
        // A line being typed is kept while walking back.
        type_text(&mut app, "draft");
        app.on_key(key(KeyCode::Up));
        assert_eq!(app.input.text, "second");
        app.on_key(key(KeyCode::Up));
        assert_eq!(app.input.text, "first");
        app.on_key(key(KeyCode::Up));
        assert_eq!(app.input.text, "first", "stops at the oldest");
        app.on_key(key(KeyCode::Down));
        assert_eq!(app.input.text, "second");
        app.on_key(key(KeyCode::Down));
        assert_eq!(app.input.text, "draft", "the typed line comes back");

        // The same line twice is remembered once.
        app.input.take();
        for _ in 0..2 {
            type_text(&mut app, "again");
            app.on_key(key(KeyCode::Enter));
        }
        app.on_key(key(KeyCode::Up));
        assert_eq!(app.input.text, "again");
        app.on_key(key(KeyCode::Up));
        assert_eq!(app.input.text, "second");
    }
    /// A message with an id, so it can be selected and answered.
    fn message_with_id(id: &str, sender: &str, text: &str) -> Line {
        use liveside_proto::model::{ChatMessage, MessageKind, User};
        Line {
            time: Timestamp::now(),
            content: LineContent::Message(ChatMessage {
                id: id.into(),
                sender: User {
                    id: "7".into(),
                    login: sender.to_lowercase().into(),
                    display_name: sender.to_owned(),
                },
                color: None,
                badges: Vec::new(),
                fragments: vec![Fragment::Text {
                    text: text.to_owned(),
                }],
                kind: MessageKind::Text,
                bits: None,
                reply: None,
                shared_source: None,
            }),
            flags: LineFlags::default(),
        }
    }

    fn ctrl_key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::CONTROL)
    }

    #[test]
    fn ctrl_r_replies_to_the_selected_message() {
        let mut app = app_with_channel();
        app.buffers[1].push(message_with_id("m1", "alice", "first"));
        app.buffers[1].push(notice_line("something happened"));
        app.buffers[1].push(message_with_id("m2", "bob", "second"));

        // Selection starts at the newest message and skips notices.
        app.on_key(ctrl_key(KeyCode::Up));
        assert_eq!(
            app.buffer().selected.as_ref().map(|id| id.as_str()),
            Some("m2")
        );
        app.on_key(ctrl_key(KeyCode::Up));
        assert_eq!(
            app.buffer().selected.as_ref().map(|id| id.as_str()),
            Some("m1")
        );
        app.on_key(ctrl_key(KeyCode::Up));
        assert_eq!(
            app.buffer().selected.as_ref().map(|id| id.as_str()),
            Some("m1"),
            "stops at the oldest"
        );
        app.on_key(ctrl_key(KeyCode::Down));
        assert_eq!(
            app.buffer().selected.as_ref().map(|id| id.as_str()),
            Some("m2")
        );

        app.on_key(ctrl_key(KeyCode::Char('r')));
        let target = app.buffer().replying_to.clone().expect("a reply target");
        assert_eq!(target.name, "bob");

        type_text(&mut app, "sure");
        let commands = app.on_key(key(KeyCode::Enter));
        assert_eq!(
            commands,
            [Command::Input {
                channel: "1".into(),
                text: "sure".into(),
                reply_to: Some("m2".into()),
            }]
        );
        // Sent: the next line is an ordinary message again.
        assert!(app.buffer().replying_to.is_none());
        assert!(app.buffer().selected.is_none());
    }

    #[test]
    fn ctrl_r_without_a_selection_answers_the_newest() {
        let mut app = app_with_channel();
        app.buffers[1].push(message_with_id("m1", "alice", "first"));
        app.on_key(ctrl_key(KeyCode::Char('r')));
        assert_eq!(
            app.buffer().replying_to.as_ref().map(|t| t.name.as_str()),
            Some("alice")
        );
    }

    #[test]
    fn esc_backs_out_of_replying_then_selection_then_scroll() {
        let mut app = app_with_lines(50, 10);
        app.buffers[1].push(message_with_id("m1", "alice", "first"));
        app.on_key(key(KeyCode::PageUp));
        app.on_key(ctrl_key(KeyCode::Char('r')));
        assert!(app.buffer().replying_to.is_some());
        let scrolled = app.buffer().scroll;
        assert!(scrolled > 0);

        app.on_key(key(KeyCode::Esc));
        assert!(app.buffer().replying_to.is_none(), "reply first");
        assert_eq!(app.buffer().scroll, scrolled, "still where the user reads");

        app.on_key(key(KeyCode::Esc));
        assert!(app.buffer().selected.is_none(), "then the selection");
        assert_eq!(app.buffer().scroll, scrolled);

        app.on_key(key(KeyCode::Esc));
        assert_eq!(app.buffer().scroll, 0, "finally back to the newest");
    }
    #[test]
    fn the_automod_queue_is_answered_oldest_first() {
        let mut app = app_with_channel();
        let held = |id: &str, who: &str| liveside_proto::event::HeldMessage {
            id: id.into(),
            sender: liveside_proto::model::User {
                id: who.into(),
                login: who.into(),
                display_name: who.to_owned(),
            },
            text: "something rude".to_owned(),
            reason: "automod".to_owned(),
            detail: "aggressive 2".to_owned(),
            time: Timestamp::now(),
        };
        for (id, who) in [("h1", "spammer"), ("h2", "someone")] {
            app.event(Event::MessageHeld {
                channel: "1".into(),
                held: held(id, who),
            });
        }
        // Without a name, the oldest.
        type_text(&mut app, "/allow");
        let commands = app.on_key(key(KeyCode::Enter));
        assert_eq!(
            commands,
            [Command::ResolveHeld {
                channel: "1".into(),
                message: "h1".into(),
                allow: true,
            }]
        );

        // With one, that person's.
        type_text(&mut app, "/deny someone");
        let commands = app.on_key(key(KeyCode::Enter));
        assert_eq!(
            commands,
            [Command::ResolveHeld {
                channel: "1".into(),
                message: "h2".into(),
                allow: false,
            }]
        );

        // The engine decides it; only then does it leave the queue.
        app.event(Event::HeldResolved {
            channel: "1".into(),
            message: "h1".into(),
            status: "approved".into(),
            moderator: "me".into(),
        });
        assert_eq!(app.buffer().held.len(), 1);
        type_text(&mut app, "/allow nobody");
        assert!(app.on_key(key(KeyCode::Enter)).is_empty());
    }

    #[test]
    fn moderation_keys_prepare_a_command_instead_of_running_it() {
        let mut app = app_with_channel();
        app.buffers[1].push(message_with_id("m1", "Spammer", "buy followers"));

        app.on_key(ctrl_key(KeyCode::Char('t')));
        // Nothing is sent: the line is there to be read and confirmed.
        assert_eq!(app.input.text, "/timeout spammer 10m");
        assert_eq!(app.input.cursor, app.input.text.len());
        assert_eq!(
            app.buffer().selected.as_ref().map(|id| id.as_str()),
            Some("m1")
        );

        app.on_key(ctrl_key(KeyCode::Char('b')));
        assert_eq!(app.input.text, "/ban spammer ");
        app.on_key(ctrl_key(KeyCode::Char('d')));
        assert_eq!(app.input.text, "/delete m1");
        // Pinning is on Alt, since Ctrl-P walks back through the buffers.
        app.on_key(KeyEvent::new(KeyCode::Char('p'), KeyModifiers::ALT));
        assert_eq!(app.input.text, "/pin m1");
        app.on_key(ctrl_key(KeyCode::Char('d')));
        assert_eq!(app.input.text, "/delete m1");

        // Only Enter actually sends it.
        let commands = app.on_key(key(KeyCode::Enter));
        assert_eq!(
            commands,
            [Command::Input {
                channel: "1".into(),
                text: "/delete m1".into(),
                reply_to: None,
            }]
        );
    }

    #[test]
    fn the_wheel_waits_a_moment_and_stops_when_the_answer_comes() {
        let mut app = App::default();
        let id = liveside_proto::RequestId(7);
        app.asked(id);
        // Asked just now: the loop redraws, but nothing is said yet.
        assert!(app.busy());
        assert!(!app.working());
        // Still owed a moment later: now the header says so.
        app.waiting.insert(
            id,
            std::time::Instant::now() - std::time::Duration::from_secs(1),
        );
        assert!(app.working());
        app.apply(ServerMessage::Response { id, error: None });
        assert!(!app.busy());
        assert!(!app.working());
    }

    #[test]
    fn engine_errors_are_said_in_the_users_language() {
        let mut app = App {
            language: liveside_client::lang::Language::Czech,
            ..App::default()
        };
        app.apply(ServerMessage::Response {
            id: liveside_proto::RequestId(1),
            error: Some(liveside_proto::CommandError::new(
                liveside_proto::ErrorKind::InvalidArgument,
                "channel.unknown",
                &[("channel", "orbitcast")],
                "No channel #orbitcast",
            )),
        });
        // And Twitch's own words are left exactly as Twitch wrote them.
        app.apply(ServerMessage::Response {
            id: liveside_proto::RequestId(2),
            error: Some(liveside_proto::CommandError::passthrough(
                liveside_proto::ErrorKind::Twitch,
                "The user specified in the user_id field may not be banned",
            )),
        });
        let said: Vec<String> = app.buffers[0]
            .lines
            .iter()
            .filter_map(|line| match &line.content {
                LineContent::Notice { text, .. } => Some(text.clone()),
                _ => None,
            })
            .collect();
        assert!(
            said.iter().any(|t| t == "Kanál #orbitcast neexistuje"),
            "{said:?}"
        );
        assert!(
            said.iter().any(|t| t.contains("may not be banned")),
            "{said:?}"
        );
    }

    #[test]
    fn moderation_keys_need_a_channel() {
        let mut app = App::default();
        app.on_key(ctrl_key(KeyCode::Char('t')));
        assert!(app.input.text.is_empty());
        let status: Vec<String> = app.buffers[0]
            .lines
            .iter()
            .filter_map(|line| match &line.content {
                LineContent::Notice { text, .. } => Some(text.clone()),
                _ => None,
            })
            .collect();
        assert!(status.iter().any(|t| t.contains("channel")), "{status:?}");
    }
}
