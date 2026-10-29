//! The Twitch backend: fulfils engine requests through Helix and turns
//! EventSub notifications into engine events.
//!
//! Chat is received over one EventSub WebSocket. Twitch allows 300
//! subscriptions per connection, which is plenty, but it also allows ten
//! *points* per token, and a subscription about a channel that has not
//! authorized this client costs one of them; once the ten are spent it
//! refuses every further subscription, chat included. So only the
//! channel that is ours subscribes to the three types that cost
//! (see `eventsub::costs_budget`) and the rest learn the same from Get
//! Streams, asked for all channels at once every minute. Sending and
//! moderation go through Helix.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::Duration;

use liveside_proto::event::{DataFlow, DataFlowKind};
use liveside_proto::model::{Channel, Line, LineContent, LineFlags, NoticeLevel, RoomModes};
use liveside_proto::{
    Account, CommandError, CommandResult, ConnectionState, ErrorKind, Event, Login, Timestamp,
    UserId,
};
use liveside_twitch::auth::{Auth, DeviceCode, Tokens};
use liveside_twitch::emotes::{EmoteSet, Emotes};
use liveside_twitch::eventsub::{self, CHANNEL_SUBSCRIPTIONS, EXTRA_SUBSCRIPTIONS};
use liveside_twitch::helix::Helix;
use liveside_twitch::mapping;
use tokio::sync::mpsc;
use tokio::time::Instant;

use crate::backend::{
    BackendIo, BackendMessage, BackendRequest, PeopleList, Role, RoomModeChange, Suspicion,
    UserTarget,
};

#[derive(Debug, Clone)]
pub struct TwitchConfig {
    pub client_id: String,
    pub scopes: Vec<String>,
    /// Where the access token is stored.
    pub token_path: PathBuf,
    pub eventsub_url: String,
    /// Where the list of people in a channel comes from.
    pub user_list: UserListSource,
    pub irc_url: String,
    /// Load emotes from 7TV, BetterTTV and FrankerFaceZ. Each is asked for
    /// its global set at login and for a channel's set on join, so those
    /// services learn which channels are being read.
    pub third_party_emotes: bool,
}

impl Default for TwitchConfig {
    fn default() -> Self {
        Self {
            client_id: liveside_twitch::CLIENT_ID.to_owned(),
            scopes: liveside_twitch::SCOPES
                .iter()
                .map(|s| (*s).to_owned())
                .collect(),
            token_path: liveside_twitch::auth::default_token_path(),
            eventsub_url: eventsub::DEFAULT_URL.to_owned(),
            user_list: UserListSource::Active,
            irc_url: liveside_twitch::irc::DEFAULT_URL.to_owned(),
            third_party_emotes: true,
        }
    }
}

/// How often the member list is asked for again: Twitch sends it on join
/// and then, in a busy channel, little else.
const MEMBERSHIP_REFRESH: Duration = Duration::from_secs(300);

/// How often to ask who is streaming. `stream.online` and `.offline` say
/// so as it happens, but only to a client that is listening at that
/// moment: this is what notices a stream that ended while the socket was
/// down, or an event Twitch simply did not deliver.
const LIVE_REFRESH: Duration = Duration::from_secs(60);

/// How the client learns who is in a channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum UserListSource {
    /// Whoever has talked; costs nothing and works in every channel.
    #[default]
    Active,
    /// Twitch IRC, the only interface that reports people joining and
    /// leaving. Opens a second connection to Twitch.
    Irc,
}

/// Returns a function suitable for [`crate::start`] that spawns this backend.
pub fn spawn(config: TwitchConfig) -> impl FnOnce(BackendIo) {
    move |io| {
        tokio::spawn(run(config, io));
    }
}

struct Joined {
    /// The channel's login, to match IRC membership, which names channels
    /// that way.
    login: Login,
    /// EventSub subscription ids, so they can be removed when parting.
    subscriptions: Vec<String>,
    /// 7TV, BTTV and FFZ emotes of this channel.
    emotes: EmoteSet,
    /// The channel's own badge pictures (subscriber tiers, bits), by set
    /// and version. Twitch's global ones are held once for all channels.
    badges: HashMap<(String, String), (String, String)>,
    /// Whether it was streaming when last asked, so the minute's check
    /// can say only what changed.
    live: bool,
}

/// Outcome of a request: either it is done, or its result comes later.
enum Outcome {
    Done(CommandResult),
    Deferred,
}

/// Emotes that finished loading in the background.
struct EmotesLoaded {
    /// The channel they belong to, or `None` for the global sets.
    channel: Option<UserId>,
    set: EmoteSet,
    problems: Vec<liveside_twitch::emotes::Problem>,
}

/// The result of a login attempt, from the task that polled Twitch.
struct LoginOutcome {
    token: u64,
    result: liveside_twitch::Result<Tokens>,
}

struct Twitch {
    config: TwitchConfig,
    http: reqwest::Client,
    auth: Auth,
    emotes: Emotes,
    /// Third-party emotes usable everywhere. Loaded once at login; providers
    /// do add emotes while we run, so refreshing them is still open.
    global_emotes: EmoteSet,
    /// The emote sets (`None` the global one) whose last load lacked a
    /// provider. They are fetched again when the connection comes back
    /// after an outage: one at login or join would otherwise leave them
    /// without those emotes until the channel is joined again.
    emotes_missing: HashSet<Option<UserId>>,
    events: mpsc::Sender<BackendMessage>,
    helix: Option<Helix>,
    account: Option<Account>,
    session_id: Option<String>,
    channels: HashMap<UserId, Joined>,
    /// Where every socket's messages go, each with the number of the
    /// socket it came from.
    ws_out: mpsc::Sender<(u64, eventsub::Message)>,
    /// The socket in use, and one being opened to take its place. Each is
    /// a task that holds the connection, and numbered, so that a socket
    /// replaced or logged out of is closed, and whatever it still says
    /// is not taken for the new one's. Two sockets alive at once deliver
    /// every event twice — seen after logging out and in again, when the
    /// first socket went on running for a day.
    socket: Option<(u64, tokio::task::JoinHandle<()>)>,
    socket_next: Option<(u64, tokio::task::JoinHandle<()>)>,
    socket_count: u64,
    login_out: mpsc::Sender<LoginOutcome>,
    /// When to open a new EventSub connection, after a failure.
    reconnect_at: Option<Instant>,
    /// The account's own subscriptions, remembered so the next socket
    /// can drop them rather than leave them behind.
    account_subscriptions: Vec<String>,
    /// Where to open it: the URL Twitch gave when it asked us to move,
    /// which carries the subscriptions over, or none for a fresh socket.
    reconnect_to: Option<String>,
    attempt: u32,
    /// The request waiting for a device-flow login to finish.
    pending_login: Option<u64>,
    emotes_out: mpsc::Sender<EmotesLoaded>,
    /// Commands to the IRC connection, when membership tracking is on.
    irc_out: Option<mpsc::Sender<liveside_twitch::irc::Command>>,
    /// The IRC connection that sends `/me`, logged in as the account;
    /// opened by the first one and kept.
    speaker: Speaker,
    /// The current access token, for the IRC login when the scopes allow.
    access_token: Option<String>,
    /// What renews it, and when to do that. A Twitch token lives about
    /// four hours; a client left running overnight without this wakes up
    /// to "invalid oauth token" on everything.
    refresh_token: Option<String>,
    renew_at: Option<Instant>,
    /// Joins asked for before the EventSub session was ready.
    pending_joins: Vec<(u64, Login)>,
    /// What was last said about a channel going wrong, so a failure that
    /// repeats every few seconds is said once rather than all night.
    said_broken: HashMap<UserId, String>,
    /// The poll running in each channel, by its Twitch id: ending one
    /// needs the id, and nobody should have to type it.
    polls: HashMap<UserId, String>,
    /// The same for the prediction, which is kept while it is locked as
    /// well: that is when its id is needed most.
    predictions: HashMap<UserId, String>,
    /// Rewards this application created, per channel. Only these can be
    /// answered for, so only their redemptions join the queue.
    manageable_rewards: HashMap<UserId, std::collections::HashSet<String>>,
    /// Twitch's own badge pictures, by set and version. Global ones,
    /// fetched once after logging in.
    global_badges: HashMap<(String, String), (String, String)>,
    /// The channels the frontend has asked to be in, whether or not they
    /// could be joined yet. A join before logging in fails — there is
    /// nobody to join as — and it is this list that gets us in once an
    /// account arrives, rather than the person typing it all again.
    wanted: Vec<Login>,
    /// The followed channels streaming at the last look, so the next one
    /// can tell who began. `None` until the first look for this account.
    followed_live: Option<HashSet<UserId>>,
}

async fn run(config: TwitchConfig, mut io: BackendIo) {
    // Without timeouts, one unresponsive service hangs login and joining
    // for ever: reqwest waits indefinitely by default.
    let http = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(20))
        .build()
        .unwrap_or_else(|_| reqwest::Client::new());
    let (mut twitch, mut inbox) = Twitch::new(config, http, io.events.clone());
    let irc_events_out = inbox.irc_events_out.clone();
    // Twitch sends the member list when a channel is joined, and then few
    // or no JOIN/PART messages in a busy channel, so the list is asked for
    // again now and then. Joining an already joined channel is answered
    // with a fresh list (measured), which is what this uses.
    let mut refresh =
        tokio::time::interval_at(Instant::now() + MEMBERSHIP_REFRESH, MEMBERSHIP_REFRESH);
    let mut live_check = tokio::time::interval_at(Instant::now() + LIVE_REFRESH, LIVE_REFRESH);
    let mut followed_check = tokio::time::interval_at(Instant::now() + LIVE_REFRESH, LIVE_REFRESH);

    twitch.report_data_flows().await;
    if twitch.config.user_list == UserListSource::Irc {
        twitch.connect_irc(&irc_events_out);
    }
    twitch.restore_session().await;

    loop {
        tokio::select! {
            request = io.requests.recv() => match request {
                Some(request) => {
                    let token = request.token;
                    match twitch.handle(token, request.request).await {
                        Outcome::Done(result) => {
                            if twitch.done(token, result).await.is_err() {
                                return;
                            }
                        }
                        // The handler keeps whatever it needs to answer later.
                        Outcome::Deferred => {}
                    }
                }
                None => return,
            },
            Some((socket, message)) = inbox.ws_in.recv() => twitch.on_socket(socket, message).await,
            Some(outcome) = inbox.login_in.recv() => twitch.on_login(outcome).await,
            Some(loaded) = inbox.emotes_in.recv() => twitch.emotes_loaded(loaded).await,
            Some(event) = inbox.irc_events.recv() => {
                twitch.on_irc(event, &irc_events_out).await;
            }
            Some(event) = inbox.speaker_events.recv() => twitch.on_speaker(event).await,
            _ = refresh.tick(), if twitch.irc_out.is_some() => twitch.refresh_membership().await,
            _ = live_check.tick(), if !twitch.channels.is_empty() => twitch.refresh_live().await,
            _ = followed_check.tick(), if twitch.may_read_follows() => {
                twitch.refresh_followed().await;
            }
            () = wait_until(twitch.reconnect_at) => {
                twitch.reconnect_at = None;
                twitch.connect_socket();
            }
            () = wait_until(twitch.renew_at) => {
                twitch.renew_at = None;
                twitch.renew_token().await;
            }
        }
    }

    /// Waits for a scheduled reconnect, or forever when none is scheduled.
    async fn wait_until(at: Option<Instant>) {
        match at {
            Some(at) => tokio::time::sleep_until(at).await,
            None => std::future::pending().await,
        }
    }
}

/// What the backend's own tasks send it: the sockets, the login being
/// waited for, emotes loaded in the background, and IRC.
struct Inbox {
    ws_in: mpsc::Receiver<(u64, eventsub::Message)>,
    login_in: mpsc::Receiver<LoginOutcome>,
    emotes_in: mpsc::Receiver<EmotesLoaded>,
    irc_events: mpsc::Receiver<liveside_twitch::irc::Event>,
    irc_events_out: mpsc::Sender<liveside_twitch::irc::Event>,
    speaker_events: mpsc::Receiver<liveside_twitch::irc::Event>,
}

/// The connection that sends `/me`, and what it is doing.
#[derive(Default)]
struct Speaker {
    out: Option<mpsc::Sender<liveside_twitch::irc::Command>>,
    /// Where its events go, given to each connection it opens.
    events: Option<mpsc::Sender<liveside_twitch::irc::Event>>,
    /// Logged in, so lines can go.
    ready: bool,
    /// The channels it has joined: IRC sends to a channel one is in.
    joined: HashSet<String>,
    /// Lines asked for before it was ready: channel and text.
    waiting: Vec<(String, String)>,
}

impl Twitch {
    fn new(
        config: TwitchConfig,
        http: reqwest::Client,
        events: mpsc::Sender<BackendMessage>,
    ) -> (Self, Inbox) {
        let (ws_out, ws_in) = mpsc::channel(256);
        let (login_out, login_in) = mpsc::channel(4);
        let (emotes_out, emotes_in) = mpsc::channel(16);
        let (irc_events_out, irc_events) = mpsc::channel(256);
        let (speaker_events_out, speaker_events) = mpsc::channel(64);
        let twitch = Twitch {
            auth: Auth::new(http.clone(), config.client_id.clone()),
            emotes: Emotes::new(http.clone()),
            global_emotes: EmoteSet::default(),
            emotes_missing: HashSet::new(),
            config,
            http,
            events,
            helix: None,
            account: None,
            session_id: None,
            channels: HashMap::new(),
            ws_out,
            socket: None,
            socket_next: None,
            socket_count: 0,
            login_out,
            reconnect_at: None,
            reconnect_to: None,
            account_subscriptions: Vec::new(),
            attempt: 0,
            pending_login: None,
            emotes_out,
            irc_out: None,
            speaker: Speaker {
                events: Some(speaker_events_out),
                ..Speaker::default()
            },
            access_token: None,
            refresh_token: None,
            renew_at: None,
            pending_joins: Vec::new(),
            said_broken: HashMap::new(),
            polls: HashMap::new(),
            predictions: HashMap::new(),
            manageable_rewards: HashMap::new(),
            wanted: Vec::new(),
            global_badges: HashMap::new(),
            followed_live: None,
        };
        (
            twitch,
            Inbox {
                ws_in,
                login_in,
                emotes_in,
                irc_events,
                irc_events_out,
                speaker_events,
            },
        )
    }
}

impl Twitch {
    async fn emit(&self, event: Event) {
        let _ = self.events.send(BackendMessage::Event(event)).await;
    }

    async fn done(&self, token: u64, result: CommandResult) -> Result<(), ()> {
        self.events
            .send(BackendMessage::Done { token, result })
            .await
            .map_err(|_| ())
    }

    /// Tells the engine what this backend sends where, so a frontend can
    /// show it. Built from the settings that drive the behaviour.
    async fn report_data_flows(&self) {
        self.emit(Event::Backend {
            name: "twitch".to_owned(),
        })
        .await;
        let third_party = |party: &str, what: &str| DataFlow {
            party: party.to_owned(),
            party_code: String::new(),
            kind: DataFlowKind::Network,
            detail: format!(
                "Asked for {what} at login and for every channel joined, \
                 which tells it the channels being read"
            ),
            code: "flow.third_party".to_owned(),
            args: Vec::new(),
            enabled: self.config.third_party_emotes,
            switch: "--no-third-party-emotes".to_owned(),
        };
        let flows = vec![
            DataFlow {
                party: "Twitch".to_owned(),
                party_code: String::new(),
                kind: DataFlowKind::Network,
                detail: "Chat is read and sent through Twitch's own API \
                         (Helix and EventSub), which sees the account, the \
                         channels joined and the messages sent"
                    .to_owned(),
                code: "flow.twitch.api".to_owned(),
                args: Vec::new(),
                enabled: true,
                switch: String::new(),
            },
            DataFlow {
                party: "This machine".to_owned(),
                party_code: "flow.party.me".to_owned(),
                kind: DataFlowKind::Disk,
                detail: format!(
                    "The Twitch access token is kept in {} (0600), so \
                     logging in again is not needed",
                    self.config.token_path.display()
                ),
                code: "flow.token".to_owned(),
                args: vec![(
                    "path".to_owned(),
                    self.config.token_path.display().to_string(),
                )],
                enabled: true,
                switch: "/logout".to_owned(),
            },
            DataFlow {
                party: "Twitch (IRC)".to_owned(),
                party_code: String::new(),
                kind: DataFlowKind::Network,
                detail: "A second connection, to the old chat interface, \
                         which is the only one that reports who joins and \
                         leaves a channel; it carries no messages"
                    .to_owned(),
                code: "flow.twitch.irc".to_owned(),
                args: Vec::new(),
                enabled: self.config.user_list == UserListSource::Irc,
                switch: "chat.user_list".to_owned(),
            },
            third_party("7TV", "its global and per-channel emotes"),
            third_party("BetterTTV", "its global and per-channel emotes"),
            third_party("FrankerFaceZ", "its global and per-channel emotes"),
        ];
        self.emit(Event::DataFlows { flows }).await;
    }

    /// Reports something that belongs to no channel. Never printed to
    /// stderr: a terminal frontend may own the screen in this process.
    async fn log(&self, level: NoticeLevel, code: &str, args: &[(&str, &str)], text: String) {
        self.emit(Event::Log {
            level,
            text,
            code: code.to_owned(),
            args: owned(args),
        })
        .await;
    }

    async fn notice(
        &self,
        channel: &UserId,
        level: NoticeLevel,
        code: &str,
        args: &[(&str, &str)],
        text: String,
    ) {
        self.emit(Event::Line {
            channel: channel.clone(),
            line: Line {
                time: Timestamp::now(),
                content: LineContent::Notice {
                    level,
                    text,
                    code: code.to_owned(),
                    args: owned(args),
                },
                flags: LineFlags::default(),
            },
        })
        .await;
    }

    /// Uses a stored token if it is still good, so a restart does not ask the
    /// user to log in again.
    async fn restore_session(&mut self) {
        let tokens = match liveside_twitch::auth::load_tokens(&self.config.token_path) {
            Ok(Some(tokens)) => tokens,
            Ok(None) => return,
            Err(e) => {
                self.log(
                    NoticeLevel::Error,
                    "token.unreadable",
                    &[("error", &e.to_string())],
                    format!("Cannot read the stored token: {e}"),
                )
                .await;
                return;
            }
        };
        let mut why = match self.use_tokens(tokens.clone()).await {
            Ok(()) => return,
            Err(e) => e,
        };
        // Expired or revoked: try the refresh token. One granted to
        // another Client ID is not worth it; Twitch refuses that too.
        if !is_other_client(&why)
            && let Some(refresh) = &tokens.refresh_token
        {
            match self.auth.refresh(refresh).await {
                Ok(fresh) => {
                    let _ = liveside_twitch::auth::save_tokens(&self.config.token_path, &fresh);
                    match self.use_tokens(fresh).await {
                        Ok(()) => return,
                        Err(e) => why = e,
                    }
                }
                Err(e) => why = e,
            }
        }
        // Without a network nothing is known about the login, so it is
        // kept for the next start. It used to be thrown away here, and
        // starting offline logged the user out.
        if let liveside_twitch::Error::Network(e) = &why {
            self.log(
                NoticeLevel::Warning,
                "token.unchecked",
                &[("error", e)],
                format!(
                    "Could not check the stored login ({e}). It is kept for the next start; /login logs in now."
                ),
            )
            .await;
            return;
        }
        let _ = liveside_twitch::auth::delete_tokens(&self.config.token_path);
        // Said rather than left to a window that simply shows nobody
        // logged in, which is all a renewal that failed used to leave.
        if is_other_client(&why) {
            self.log(
                NoticeLevel::Warning,
                "token.other_client",
                &[],
                "The stored login was made with another Client ID. Log in again.".to_owned(),
            )
            .await;
        } else {
            let e = why.to_string();
            self.log(
                NoticeLevel::Warning,
                "token.restore_failed",
                &[("error", &e)],
                format!("The stored login could not be renewed ({e}). Log in again."),
            )
            .await;
        }
    }

    /// Validates a token, adopts it, and starts receiving chat.
    async fn use_tokens(&mut self, tokens: Tokens) -> liveside_twitch::Result<()> {
        let valid = self.auth.validate(&tokens.access_token).await?;
        // Granted to another application, after `twitch.client_id` was
        // changed: Twitch accepts it here and refuses every call after.
        if !valid.client_id.is_empty() && valid.client_id != self.config.client_id {
            return Err(liveside_twitch::Error::Unauthorized(
                OTHER_CLIENT.to_owned(),
            ));
        }
        let account = Account {
            user: liveside_proto::model::User {
                id: valid.user_id.clone().into(),
                login: valid.login.clone().into(),
                display_name: valid.login.clone(),
            },
            scopes: valid.scopes.clone(),
        };
        self.helix = Some(Helix::new(
            self.http.clone(),
            self.config.client_id.clone(),
            tokens.access_token.clone(),
        ));
        // Who another account follows is no news for this one.
        if self.account.as_ref().map(|a| &a.user.id) != Some(&account.user.id) {
            self.followed_live = None;
        }
        self.account = Some(account.clone());
        self.access_token = Some(tokens.access_token.clone());
        self.refresh_token = tokens.refresh_token.clone();
        self.renew_at = Some(renew_at(valid.expires_in));
        self.emit(Event::LoggedIn { account }).await;
        self.warn_about_missing_scopes(&valid.scopes).await;
        // The first look right away: what streams already is how things
        // stand, and a channel starting in the next minute is then news.
        if self.followed_live.is_none() && self.may_read_follows() {
            self.refresh_followed().await;
        }
        if let Some(helix) = self.helix.clone() {
            self.global_badges = badges_of(&helix, None).await;
        }
        if self.config.third_party_emotes && self.global_emotes.is_empty() {
            self.load_emotes(None);
        }
        self.connect_socket();
        Ok(())
    }

    /// Renews the access token before it runs out. Twitch invalidates
    /// the old one, and EventSub subscriptions belong to the token that
    /// made them, so the socket is opened again with the new one —
    /// a gap of a second or two every few hours, against going deaf.
    async fn renew_token(&mut self) {
        let Some(refresh) = self.refresh_token.clone() else {
            return;
        };
        let fresh = match self.auth.refresh(&refresh).await {
            Ok(fresh) => fresh,
            Err(e) => {
                // Nothing can be done about it here: say so where it
                // will be read, rather than letting every command start
                // failing with Twitch's word for an expired login.
                self.log(
                    NoticeLevel::Error,
                    "token.renew_failed",
                    &[("error", &e.to_string())],
                    format!("Could not renew the login: {e}. Log in again."),
                )
                .await;
                self.renew_at = None;
                return;
            }
        };
        let _ = liveside_twitch::auth::save_tokens(&self.config.token_path, &fresh);
        let valid = match self.auth.validate(&fresh.access_token).await {
            Ok(valid) => valid,
            Err(_) => {
                self.renew_at = Some(Instant::now() + Duration::from_secs(60));
                return;
            }
        };
        self.helix = Some(Helix::new(
            self.http.clone(),
            self.config.client_id.clone(),
            fresh.access_token.clone(),
        ));
        self.access_token = Some(fresh.access_token.clone());
        self.refresh_token = fresh.refresh_token.clone();
        self.renew_at = Some(renew_at(valid.expires_in));
        // Said to nobody: a renewal that works is not news, and one that
        // does not says so above.
        //
        // The socket is left alone. A subscription belongs to the
        // authorisation — this user and this client — rather than to the
        // string of the token that made it, and the old token goes on
        // working until it expires, so the session outlives the renewal.
        // Rebuilding it here cost a round of every subscription and a
        // gap in the chat for nothing. If Twitch does drop them it says
        // so, and the revocation arm opens a new socket; if the session
        // is already gone, there is nothing to keep.
        if self.session_id.is_none() {
            self.connect_socket();
        }
    }

    /// Says once, after logging in, when the stored token was granted
    /// before a command existed. Twitch answers such a call with a refusal
    /// about scopes, which is no use to someone who has never heard the
    /// word: better to say it up front, while nothing has failed yet.
    async fn warn_about_missing_scopes(&self, granted: &[String]) {
        let missing: Vec<&str> = self
            .config
            .scopes
            .iter()
            .map(String::as_str)
            .filter(|wanted| !granted.iter().any(|have| have == wanted))
            .collect();
        if missing.is_empty() {
            return;
        }
        let list = missing.join(", ");
        self.log(
            NoticeLevel::Warning,
            "account.old_permissions",
            &[("scopes", &list)],
            format!(
                "This login is missing permissions added since: {list}. \
                 Log out and log in again to use the commands that need them."
            ),
        )
        .await;
    }

    /// Fetches emotes in the background. They are decoration: waiting for
    /// three third-party services before chat works would mean any of them
    /// can keep the user out of their own chat.
    fn load_emotes(&self, channel: Option<(UserId, Login)>) {
        let emotes = self.emotes.clone();
        let out = self.emotes_out.clone();
        tokio::spawn(async move {
            let loaded = match &channel {
                Some((id, _)) => {
                    let (set, problems) = emotes.channel(id.as_str()).await;
                    EmotesLoaded {
                        channel: Some(id.clone()),
                        set,
                        problems,
                    }
                }
                None => {
                    let (set, problems) = emotes.global().await;
                    EmotesLoaded {
                        channel: None,
                        set,
                        problems,
                    }
                }
            };
            let _ = out.send(loaded).await;
        });
    }

    /// Stores emotes that arrived, and reports providers that did not.
    async fn emotes_loaded(&mut self, loaded: EmotesLoaded) {
        let scope = loaded.channel.clone();
        match loaded.channel {
            Some(channel) => {
                let Some(joined) = self.channels.get_mut(&channel) else {
                    return; // Parted while they loaded.
                };
                joined.emotes = loaded.set;
            }
            None => self.global_emotes = loaded.set,
        }
        if loaded.problems.is_empty() {
            self.emotes_missing.remove(&scope);
        } else {
            self.emotes_missing.insert(scope);
        }
        for problem in loaded.problems {
            self.log(
                NoticeLevel::Warning,
                "emotes.unreadable",
                &[("provider", problem.provider), ("error", &problem.error)],
                format!("Cannot load {} emotes: {}", problem.provider, problem.error),
            )
            .await;
        }
    }

    /// The emote sets to fetch again, forgotten as missing until that load
    /// says how it went: the global one, and channels still joined.
    fn emotes_to_retry(&mut self) -> Vec<Option<(UserId, Login)>> {
        std::mem::take(&mut self.emotes_missing)
            .into_iter()
            .filter_map(|scope| match scope {
                None => Some(None),
                Some(id) => {
                    let login = self.channels.get(&id)?.login.clone();
                    Some(Some((id, login)))
                }
            })
            .collect()
    }

    /// Opens the membership connection. Anonymous unless the token has the
    /// `chat:read` scope: an anonymous connection sees people join and
    /// leave, which is most of the value, without asking for a scope that
    /// only this feature needs.
    fn connect_irc(&mut self, events: &mpsc::Sender<liveside_twitch::irc::Event>) {
        let auth = match (&self.account, &self.access_token) {
            (Some(account), Some(token)) if account.scopes.iter().any(|s| s == "chat:read") => {
                liveside_twitch::irc::Auth::Token {
                    login: account.user.login.to_string(),
                    token: token.clone(),
                }
            }
            _ => liveside_twitch::irc::Auth::Anonymous,
        };
        let (commands, commands_in) = mpsc::channel(64);
        self.irc_out = Some(commands);
        let url = self.config.irc_url.clone();
        let events = events.clone();
        tokio::spawn(async move {
            liveside_twitch::irc::run(
                &url,
                auth,
                liveside_twitch::irc::MEMBERSHIP,
                commands_in,
                events,
            )
            .await;
        });
    }

    /// Sends `/me` over IRC, logged in as the account: Twitch's API for
    /// sending chat takes no commands. The line comes back through
    /// EventSub like any other the account sends; a refusal comes as an
    /// IRC notice, which is said in the channel.
    async fn send_action(&mut self, channel: UserId, text: String) -> CommandResult {
        let account = self.require_account()?;
        if !account.scopes.iter().any(|s| s == "chat:edit") {
            return Err(error(
                ErrorKind::MissingScope,
                "me.needs_login",
                &[],
                "/me needs the chat:edit permission this login does not have: log out and in again",
            ));
        }
        let login = self.channel_name(&channel);
        if self.speaker.out.is_none() {
            self.connect_speaker();
        }
        match self.speaker.ready {
            true => self.speak(login, text).await,
            false => self.speaker.waiting.push((login, text)),
        }
        Ok(())
    }

    fn connect_speaker(&mut self) {
        let (Some(account), Some(token), Some(events)) = (
            &self.account,
            &self.access_token,
            self.speaker.events.clone(),
        ) else {
            return;
        };
        let auth = liveside_twitch::irc::Auth::Token {
            login: account.user.login.to_string(),
            token: token.clone(),
        };
        let (commands, commands_in) = mpsc::channel(64);
        self.speaker.out = Some(commands);
        self.speaker.ready = false;
        self.speaker.joined.clear();
        let url = self.config.irc_url.clone();
        tokio::spawn(async move {
            liveside_twitch::irc::run(
                &url,
                auth,
                liveside_twitch::irc::COMMANDS,
                commands_in,
                events,
            )
            .await;
        });
    }

    /// Joins the channel if not yet, and says the line there.
    async fn speak(&mut self, channel: String, text: String) {
        let Some(out) = self.speaker.out.clone() else {
            return;
        };
        if self.speaker.joined.insert(channel.clone()) {
            let _ = out
                .send(liveside_twitch::irc::Command::Join(channel.clone()))
                .await;
        }
        let _ = out
            .send(liveside_twitch::irc::Command::Say {
                channel,
                text: format!("/me {text}"),
            })
            .await;
    }

    async fn on_speaker(&mut self, event: liveside_twitch::irc::Event) {
        use liveside_twitch::irc::Event as Irc;
        match event {
            Irc::Ready => {
                self.speaker.ready = true;
                for (channel, text) in std::mem::take(&mut self.speaker.waiting) {
                    self.speak(channel, text).await;
                }
            }
            // Twitch saying no, in its words, where it was said.
            Irc::Notice { channel, text } => {
                let id = channel
                    .as_deref()
                    .and_then(|login| self.channel_by_login(login));
                match id {
                    Some(id) => {
                        self.notice(
                            &id,
                            NoticeLevel::Warning,
                            "me.refused",
                            &[("reason", &text)],
                            format!("Twitch did not send the /me: {text}"),
                        )
                        .await
                    }
                    None => {
                        self.log(
                            NoticeLevel::Warning,
                            "me.refused",
                            &[("reason", &text)],
                            format!("Twitch did not send the /me: {text}"),
                        )
                        .await
                    }
                }
            }
            Irc::Closed { reason } => {
                let lost = self.speaker.waiting.len();
                self.speaker = Speaker {
                    events: self.speaker.events.take(),
                    ..Speaker::default()
                };
                // Opened again by the next /me; what waited is said lost.
                if lost > 0 {
                    self.log(
                        NoticeLevel::Warning,
                        "me.closed",
                        &[("reason", &reason)],
                        format!("The connection for /me closed before it could send: {reason}"),
                    )
                    .await;
                }
            }
            Irc::Names { .. } | Irc::Joined { .. } | Irc::Parted { .. } => {}
        }
    }

    /// Membership from IRC, turned into engine events. Channels are named
    /// by login there, so they are matched against the joined channels.
    async fn on_irc(
        &mut self,
        event: liveside_twitch::irc::Event,
        events: &mpsc::Sender<liveside_twitch::irc::Event>,
    ) {
        use liveside_twitch::irc::Event as Irc;
        match event {
            Irc::Ready => {
                // Rejoin whatever is already joined, after a reconnect too.
                let logins: Vec<String> = self
                    .channels
                    .values()
                    .map(|joined| joined.login.to_string())
                    .collect();
                for login in logins {
                    self.irc_join(&login).await;
                }
            }
            Irc::Names { channel, users } => {
                let Some(id) = self.channel_by_login(&channel) else {
                    return;
                };
                // Our own anonymous nick is not a chatter.
                let logins: Vec<Login> = users
                    .into_iter()
                    .filter(|user| !user.starts_with("justinfan"))
                    .map(Login::from)
                    .collect();
                self.emit(Event::Chatters {
                    channel: id,
                    logins,
                })
                .await;
            }
            Irc::Joined { channel, user } => {
                if let Some(id) = self.channel_by_login(&channel)
                    && !user.starts_with("justinfan")
                {
                    self.emit(Event::ChatterJoined {
                        channel: id,
                        login: user.into(),
                    })
                    .await;
                }
            }
            Irc::Parted { channel, user } => {
                if let Some(id) = self.channel_by_login(&channel) {
                    self.emit(Event::ChatterParted {
                        channel: id,
                        login: user.into(),
                    })
                    .await;
                }
            }
            // Membership asks nothing of Twitch that it could refuse.
            Irc::Notice { .. } => {}
            Irc::Closed { reason } => {
                self.irc_out = None;
                self.log(
                    NoticeLevel::Warning,
                    "userlist.closed",
                    &[("reason", &reason)],
                    format!("User list connection closed: {reason}"),
                )
                .await;
                // The chat itself is unaffected, so this reconnects quietly
                // after a pause rather than tearing anything down.
                let events = events.clone();
                let url = self.config.irc_url.clone();
                let anonymous = liveside_twitch::irc::Auth::Anonymous;
                let (commands, commands_in) = mpsc::channel(64);
                self.irc_out = Some(commands);
                tokio::spawn(async move {
                    tokio::time::sleep(Duration::from_secs(5)).await;
                    liveside_twitch::irc::run(
                        &url,
                        anonymous,
                        liveside_twitch::irc::MEMBERSHIP,
                        commands_in,
                        events,
                    )
                    .await;
                });
            }
        }
    }

    fn channel_by_login(&self, login: &str) -> Option<UserId> {
        self.channels
            .iter()
            .find(|(_, joined)| joined.login.as_str() == login)
            .map(|(id, _)| id.clone())
    }

    /// Asks for every joined channel's member list again.
    /// Asks who of the joined channels is streaming, and says so only
    /// where the answer changed: this runs every minute, and a line in
    /// the chat each time would be unbearable.
    async fn refresh_live(&mut self) {
        let Some(helix) = self.helix.clone() else {
            return;
        };
        let ids: Vec<String> = self.channels.keys().map(|id| id.to_string()).collect();
        let Ok(live) = helix.live_among(&ids).await else {
            return;
        };
        self.say_stream_stats(&live).await;
        let channels: Vec<UserId> = self.channels.keys().cloned().collect();
        for channel in channels {
            let now = live.iter().any(|stream| stream.user_id == channel.as_str());
            let before = self.channels.get(&channel).is_some_and(|c| c.live);
            if now != before {
                if let Some(joined) = self.channels.get_mut(&channel) {
                    joined.live = now;
                }
                self.emit(Event::Stream {
                    channel: channel.clone(),
                    live: now,
                })
                .await;
            }
        }
    }

    /// Whether the token allows reading the account's follows.
    fn may_read_follows(&self) -> bool {
        self.account
            .as_ref()
            .is_some_and(|account| account.scopes.iter().any(|s| s == FOLLOWS_SCOPE))
    }

    /// Asks which followed channels stream, and says so, with the ones
    /// that began since the last look. A failed look changes nothing: a
    /// minute without network is not every channel going offline and
    /// then, a minute later, all of them starting at once.
    async fn refresh_followed(&mut self) {
        let (Some(helix), Some(account)) = (self.helix.clone(), self.account.as_ref()) else {
            return;
        };
        let user = account.user.id.to_string();
        let Ok(streams) = helix.followed_streams(&user).await else {
            return;
        };
        let live = followed_list(Vec::new(), streams);
        let now: HashSet<UserId> = live.iter().map(|f| f.channel.id.clone()).collect();
        let started = match &self.followed_live {
            Some(before) => started_streaming(before, &live, &self.channels),
            None => Vec::new(),
        };
        self.followed_live = Some(now);
        self.emit(Event::FollowedStreams { live, started }).await;
    }

    /// Says how many watch each joined channel that streams, and since
    /// when: it comes with the answer that says who streams at all.
    async fn say_stream_stats(&self, live: &[liveside_twitch::helix::LiveStream]) {
        for stream in live {
            let channel = UserId(stream.user_id.clone());
            if !self.channels.contains_key(&channel) {
                continue;
            }
            let Some(started) = liveside_proto::Timestamp::parse_utc(&stream.started_at) else {
                continue;
            };
            self.emit(Event::StreamStats {
                channel,
                stats: liveside_proto::model::StreamStats {
                    viewers: stream.viewer_count,
                    started,
                },
            })
            .await;
        }
    }

    async fn refresh_membership(&self) {
        for joined in self.channels.values() {
            self.irc_join(joined.login.as_str()).await;
        }
    }

    async fn irc_join(&self, login: &str) {
        if let Some(irc) = &self.irc_out {
            let _ = irc
                .send(liveside_twitch::irc::Command::Join(login.to_owned()))
                .await;
        }
    }

    /// Opens a socket to take the place of the one in use. The old one is
    /// closed when the new one is welcomed — Twitch asks for exactly that
    /// when it moves a session — and a socket still being opened is given
    /// up for this one.
    fn connect_socket(&mut self) {
        self.session_id = None;
        // A URL from a reconnect message keeps the subscriptions of the
        // socket it replaces; anywhere else they have to be made again.
        let resumed = self.reconnect_to.is_some();
        let url = self
            .reconnect_to
            .take()
            .unwrap_or_else(|| self.config.eventsub_url.clone());
        if let Some((_, opening)) = self.socket_next.take() {
            opening.abort();
        }
        self.socket_count += 1;
        let number = self.socket_count;
        let out = self.ws_out.clone();
        let task = tokio::spawn(async move {
            let (tx, mut rx) = mpsc::channel(64);
            let socket = eventsub::run(&url, resumed, tx);
            tokio::pin!(socket);
            let mut ended = false;
            loop {
                tokio::select! {
                    () = &mut socket, if !ended => ended = true,
                    message = rx.recv() => match message {
                        Some(message) => {
                            if out.send((number, message)).await.is_err() {
                                return;
                            }
                        }
                        // The socket is done and all it said is passed on.
                        None => return,
                    },
                }
            }
        });
        self.socket_next = Some((number, task));
    }

    /// Closes every socket: nothing more is to come from Twitch.
    fn close_sockets(&mut self) {
        for (_, task) in [self.socket.take(), self.socket_next.take()]
            .into_iter()
            .flatten()
        {
            task.abort();
        }
    }

    async fn handle(&mut self, token: u64, request: BackendRequest) -> Outcome {
        let result = match request {
            BackendRequest::Login => return self.login(token).await,
            BackendRequest::Logout => {
                self.logout().await;
                Ok(())
            }
            BackendRequest::Join { channel } => {
                if !self.wanted.contains(&channel) {
                    self.wanted.push(channel.clone());
                }
                // Joining needs a live EventSub session. Right after login
                // there is none yet, so hold the request instead of failing.
                if self.account.is_some() && self.session_id.is_none() {
                    self.pending_joins.push((token, channel));
                    return Outcome::Deferred;
                }
                self.join(&channel).await
            }
            BackendRequest::Part { channel } => self.part(&channel).await,
            BackendRequest::SendAction { channel, text } => self.send_action(channel, text).await,
            BackendRequest::Send {
                channel,
                text,
                reply_to,
            } => {
                let account = match self.require_account() {
                    Ok(account) => account,
                    Err(e) => return Outcome::Done(Err(e)),
                };
                let sender_id = account.user.id.to_string();
                self.with_helix(|helix| async move {
                    helix
                        .send_message(
                            channel.as_str(),
                            &sender_id,
                            &text,
                            reply_to.as_ref().map(|id| id.as_str()),
                        )
                        .await
                })
                .await
            }
            BackendRequest::Delete { channel, message } => {
                let moderator = match self.require_account() {
                    Ok(account) => account.user.id.to_string(),
                    Err(e) => return Outcome::Done(Err(e)),
                };
                self.with_helix(|helix| async move {
                    helix
                        .delete_message(channel.as_str(), &moderator, message.as_str())
                        .await
                })
                .await
            }
            BackendRequest::Timeout {
                channel,
                user,
                seconds,
                reason,
            } => self.ban(channel, user, Some(seconds), reason).await,
            BackendRequest::Ban {
                channel,
                user,
                reason,
            } => self.ban(channel, user, None, reason).await,
            BackendRequest::Warn {
                channel,
                user,
                reason,
            } => {
                let moderator = match self.require_account() {
                    Ok(account) => account.user.id.to_string(),
                    Err(e) => return Outcome::Done(Err(e)),
                };
                let user_id = match self.resolve(&user).await {
                    Ok(id) => id,
                    Err(e) => return Outcome::Done(Err(e)),
                };
                self.with_helix(|helix| async move {
                    helix
                        .warn(channel.as_str(), &moderator, &user_id, &reason)
                        .await
                })
                .await
            }
            BackendRequest::ClearChat { channel } => {
                let moderator = match self.require_account() {
                    Ok(account) => account.user.id.to_string(),
                    Err(e) => return Outcome::Done(Err(e)),
                };
                self.with_helix(|helix| async move {
                    helix.clear_chat(channel.as_str(), &moderator).await
                })
                .await
            }
            BackendRequest::Announce {
                channel,
                text,
                color,
            } => {
                let moderator = match self.require_account() {
                    Ok(account) => account.user.id.to_string(),
                    Err(e) => return Outcome::Done(Err(e)),
                };
                self.with_helix(|helix| async move {
                    helix
                        .announce(channel.as_str(), &moderator, &text, &color)
                        .await
                })
                .await
            }
            BackendRequest::SetRoomMode { channel, mode } => {
                let moderator = match self.require_account() {
                    Ok(account) => account.user.id.to_string(),
                    Err(e) => return Outcome::Done(Err(e)),
                };
                let change = room_mode_patch(&mode);
                self.with_helix(|helix| async move {
                    helix
                        .update_chat_settings(channel.as_str(), &moderator, &change)
                        .await
                })
                .await
            }
            BackendRequest::SetChannelInfo {
                channel,
                title,
                game,
                language,
                tags,
                delay,
                labels,
            } => {
                let account = match self.require_account() {
                    Ok(account) => account,
                    Err(e) => return Outcome::Done(Err(e)),
                };
                // Twitch refuses this for anyone but the broadcaster, and
                // a refusal that says "the ID must match the token" is not
                // what someone standing in a friend's channel needs to read.
                if account.user.id != channel {
                    let name = self.channel_name(&channel);
                    return Outcome::Done(Err(error(
                        ErrorKind::InvalidArgument,
                        "channel.not_yours",
                        &[("channel", &name)],
                        format!(
                            "Only the broadcaster can change that, and #{name} is not your channel"
                        ),
                    )));
                }
                self.set_channel_info(channel, title, game, language, tags, delay, labels)
                    .await
            }
            BackendRequest::ShieldMode { channel, on } => {
                let moderator = match self.require_account() {
                    Ok(account) => account.user.id.to_string(),
                    Err(e) => return Outcome::Done(Err(e)),
                };
                self.with_helix(|helix| async move {
                    helix
                        .set_shield_mode(channel.as_str(), &moderator, on)
                        .await
                })
                .await
            }
            BackendRequest::BlockTerm { channel, text, on } => {
                self.block_term(channel, text, on).await
            }
            BackendRequest::CreatePoll {
                channel,
                title,
                choices,
                seconds,
            } => self.create_poll(channel, title, choices, seconds).await,
            BackendRequest::EndPoll { channel, archive } => self.end_poll(channel, archive).await,
            BackendRequest::CreatePrediction {
                channel,
                title,
                outcomes,
                seconds,
            } => {
                self.create_prediction(channel, title, outcomes, seconds)
                    .await
            }
            BackendRequest::EndPrediction { channel, how } => {
                self.end_prediction(channel, how).await
            }
            BackendRequest::ShowChannelInfo { channel } => self.show_channel_info(channel).await,
            BackendRequest::ShowSchedule { channel } => self.show_schedule(channel).await,
            BackendRequest::AddSegment {
                channel,
                start,
                minutes,
                title,
                weekly,
            } => {
                self.add_segment(channel, start, minutes, title, weekly)
                    .await
            }
            BackendRequest::CancelSegment {
                channel,
                segment,
                canceled,
            } => self.cancel_segment(channel, segment, canceled).await,
            BackendRequest::DeleteSegment { channel, segment } => {
                self.delete_segment(channel, segment).await
            }
            BackendRequest::SetVacation { channel, away } => self.set_vacation(channel, away).await,
            BackendRequest::ShowAds { channel } => self.show_ads(channel, false).await,
            BackendRequest::SnoozeAd { channel } => self.show_ads(channel, true).await,
            BackendRequest::SendWhisper { user, text } => self.send_whisper(user, text).await,
            BackendRequest::ShowModeratedChannels => self.show_moderated_channels().await,
            BackendRequest::ShowFollowedChannels => self.show_followed_channels().await,
            BackendRequest::CreateClip { channel } => self.create_clip(channel).await,
            BackendRequest::ShowRewards { channel } => self.show_rewards(channel).await,
            BackendRequest::CreateReward {
                channel,
                title,
                cost,
                prompt,
            } => self.create_reward(channel, title, cost, prompt).await,
            BackendRequest::UpdateReward {
                channel,
                reward,
                title,
                cost,
                prompt,
                enabled,
                paused,
            } => {
                self.update_reward(channel, reward, title, cost, prompt, enabled, paused)
                    .await
            }
            BackendRequest::DeleteReward { channel, reward } => {
                self.delete_reward(channel, reward).await
            }
            BackendRequest::AnswerRedemption {
                channel,
                reward,
                redemption,
                fulfil,
            } => {
                self.answer_redemption(channel, reward, redemption, fulfil)
                    .await
            }
            BackendRequest::ShowUser { channel, user } => {
                let id = channel.clone();
                let login = user.to_string();
                let card = self
                    .ask_helix(|helix| async move { helix.user_card(&login, id.as_str()).await })
                    .await;
                match card {
                    Ok(card) => {
                        self.emit(Event::UserCard {
                            channel,
                            card: liveside_proto::event::UserCard {
                                user: card.user,
                                created_at: card.created_at,
                                follows_since: card.follows_since,
                                follows_known: card.follows_known,
                                sub_known: card.sub_known,
                                // Twitch counts tiers in thousands; a
                                // person says 1, 2 or 3.
                                sub_tier: match card.sub_tier.as_deref() {
                                    Some("2000") => "2".to_owned(),
                                    Some("3000") => "3".to_owned(),
                                    Some(_) => "1".to_owned(),
                                    None => String::new(),
                                },
                                sub_gift: card.sub_gift,
                                banned: card.banned,
                                banned_until: card.banned_until,
                            },
                        })
                        .await;
                        Ok(())
                    }
                    Err(e) => Err(e),
                }
            }
            BackendRequest::ShowPeople { channel, list } => self.show_people(channel, list).await,
            BackendRequest::ShowBlockedTerms { channel } => self.show_blocked_terms(channel).await,
            BackendRequest::SetSuspicious {
                channel,
                user,
                status,
            } => {
                let moderator = match self.require_account() {
                    Ok(account) => account.user.id.to_string(),
                    Err(e) => return Outcome::Done(Err(e)),
                };
                let user_id = match self.resolve(&user).await {
                    Ok(id) => id,
                    Err(e) => return Outcome::Done(Err(e)),
                };
                let status = match status {
                    Suspicion::Monitored => Some("ACTIVE_MONITORING"),
                    Suspicion::Restricted => Some("RESTRICTED"),
                    Suspicion::None => None,
                };
                self.with_helix(|helix| async move {
                    helix
                        .set_suspicious(channel.as_str(), &moderator, &user_id, status)
                        .await
                })
                .await
            }
            BackendRequest::Shoutout { channel, to } => {
                let moderator = match self.require_account() {
                    Ok(account) => account.user.id.to_string(),
                    Err(e) => return Outcome::Done(Err(e)),
                };
                let to_id = match self.resolve(&to).await {
                    Ok(id) => id,
                    Err(e) => return Outcome::Done(Err(e)),
                };
                self.with_helix(|helix| async move {
                    helix.shoutout(channel.as_str(), &to_id, &moderator).await
                })
                .await
            }
            BackendRequest::Raid { channel, to } => {
                if let Err(e) = self.require_own_channel(&channel) {
                    return Outcome::Done(Err(e));
                }
                let to_id = match self.resolve(&to).await {
                    Ok(id) => id,
                    Err(e) => return Outcome::Done(Err(e)),
                };
                self.with_helix(
                    |helix| async move { helix.start_raid(channel.as_str(), &to_id).await },
                )
                .await
            }
            BackendRequest::CancelRaid { channel } => {
                if let Err(e) = self.require_own_channel(&channel) {
                    return Outcome::Done(Err(e));
                }
                self.with_helix(|helix| async move { helix.cancel_raid(channel.as_str()).await })
                    .await
            }
            BackendRequest::Marker {
                channel,
                description,
            } => self.marker(channel, description).await,
            BackendRequest::Commercial { channel, seconds } => {
                if let Err(e) = self.require_own_channel(&channel) {
                    return Outcome::Done(Err(e));
                }
                self.commercial(channel, seconds).await
            }
            BackendRequest::ResolveUnban {
                channel,
                request,
                allow,
            } => {
                let moderator = match self.require_account() {
                    Ok(account) => account.user.id.to_string(),
                    Err(e) => return Outcome::Done(Err(e)),
                };
                self.with_helix(|helix| async move {
                    helix
                        .resolve_unban_request(channel.as_str(), &moderator, &request, allow)
                        .await
                })
                .await
            }
            BackendRequest::ResolveHeld {
                channel,
                message,
                allow,
            } => {
                let moderator = match self.require_account() {
                    Ok(account) => account.user.id.to_string(),
                    Err(e) => return Outcome::Done(Err(e)),
                };
                let _ = &channel;
                self.with_helix(|helix| async move {
                    helix
                        .resolve_held(&moderator, message.as_str(), allow)
                        .await
                })
                .await
            }
            BackendRequest::Pin {
                channel,
                message,
                seconds,
            } => {
                let moderator = match self.require_account() {
                    Ok(account) => account.user.id.to_string(),
                    Err(e) => return Outcome::Done(Err(e)),
                };
                self.with_helix(|helix| async move {
                    helix
                        .pin_message(channel.as_str(), &moderator, message.as_str(), seconds)
                        .await
                })
                .await
            }
            BackendRequest::Unpin { channel } => {
                let moderator = match self.require_account() {
                    Ok(account) => account.user.id.to_string(),
                    Err(e) => return Outcome::Done(Err(e)),
                };
                self.with_helix(|helix| async move {
                    helix.unpin_message(channel.as_str(), &moderator).await
                })
                .await
            }
            BackendRequest::SetRole {
                channel,
                user,
                role,
                on,
            } => {
                let user_id = match self.resolve(&user).await {
                    Ok(id) => id,
                    Err(e) => return Outcome::Done(Err(e)),
                };
                self.with_helix(|helix| async move {
                    match role {
                        Role::Moderator => {
                            helix.set_moderator(channel.as_str(), &user_id, on).await
                        }
                        Role::Vip => helix.set_vip(channel.as_str(), &user_id, on).await,
                    }
                })
                .await
            }
            BackendRequest::Unban { channel, user } => {
                let moderator = match self.require_account() {
                    Ok(account) => account.user.id.to_string(),
                    Err(e) => return Outcome::Done(Err(e)),
                };
                let user_id = match self.resolve(&user).await {
                    Ok(id) => id,
                    Err(e) => return Outcome::Done(Err(e)),
                };
                self.with_helix(|helix| async move {
                    helix.unban(channel.as_str(), &moderator, &user_id).await
                })
                .await
            }
        };
        Outcome::Done(result)
    }

    /// The login of a joined channel, for a message about it; the id
    /// itself if it is not joined, which is better than nothing.
    fn channel_name(&self, channel: &UserId) -> String {
        self.channels
            .get(channel)
            .map(|joined| joined.login.to_string())
            .unwrap_or_else(|| channel.to_string())
    }

    /// Refuses what Twitch takes only from the channel's own login: the
    /// title and category, raids and advertisements. Its own refusal says
    /// "the ID must match the token", which is no use to someone standing
    /// in a friend's channel — and misleading to a moderator, who sees
    /// titles change every day (through a bot the broadcaster authorised,
    /// or from the dashboard as an *editor*, which is a different role
    /// from moderator and is not this endpoint's business either).
    fn require_own_channel(&self, channel: &UserId) -> Result<(), liveside_proto::CommandError> {
        let account = self.require_account()?;
        if &account.user.id == channel {
            return Ok(());
        }
        let name = self.channel_name(channel);
        Err(error(
            ErrorKind::InvalidArgument,
            "channel.not_yours",
            &[("channel", &name)],
            format!(
                "Twitch takes this only from #{name}'s own login; a moderator does it \
                 through a bot the broadcaster authorised"
            ),
        ))
    }

    /// Blocks or unblocks a word. Twitch removes a term by id, not by
    /// text, so unblocking means finding it in the channel's list first.
    async fn block_term(&mut self, channel: UserId, text: String, on: bool) -> CommandResult {
        let moderator = self.require_account()?.user.id.to_string();
        let id = channel.clone();
        if on {
            let wanted = text.clone();
            let moderator = moderator.clone();
            return self
                .with_helix(|helix| async move {
                    helix
                        .add_blocked_term(id.as_str(), &moderator, &wanted)
                        .await
                })
                .await;
        }
        let terms = {
            let id = id.clone();
            let moderator = moderator.clone();
            self.ask_helix(
                |helix| async move { helix.blocked_terms(id.as_str(), &moderator).await },
            )
            .await?
        };
        let wanted = text.to_lowercase();
        let Some(term) = terms.iter().find(|t| t.text.to_lowercase() == wanted) else {
            return Err(error(
                ErrorKind::InvalidArgument,
                "blockterm.unknown",
                &[("term", &text)],
                format!("{text} is not blocked here"),
            ));
        };
        let term_id = term.id.clone();
        self.with_helix(|helix| async move {
            helix
                .remove_blocked_term(id.as_str(), &moderator, &term_id)
                .await
        })
        .await
    }

    /// Says who moderates the channel, who is a VIP there, or who is
    /// banned. Twitch takes all three only from the channel's own login,
    /// however many channels one moderates.
    /// Starts a poll and says how it stands, without waiting for
    /// EventSub: the answer to the request is certain, while the
    /// subscription may have been refused.
    async fn create_poll(
        &mut self,
        channel: UserId,
        title: String,
        choices: Vec<String>,
        seconds: u32,
    ) -> CommandResult {
        self.require_own_channel(&channel)?;
        let id = channel.clone();
        let poll = self
            .ask_helix(|helix| async move {
                helix
                    .create_poll(id.as_str(), &title, &choices, seconds)
                    .await
            })
            .await?;
        let poll = from_helix_poll(poll);
        self.polls.insert(channel.clone(), poll.id.clone());
        self.emit(Event::PollUpdate { channel, poll }).await;
        Ok(())
    }

    /// Stops the poll running in the channel. Twitch wants its id, which
    /// nobody should have to type: it is whatever started last.
    async fn end_poll(&mut self, channel: UserId, archive: bool) -> CommandResult {
        self.require_own_channel(&channel)?;
        let Some(running) = self.polls.get(&channel).cloned() else {
            return Err(error(
                ErrorKind::InvalidArgument,
                "poll.none",
                &[],
                "No poll is running".to_owned(),
            ));
        };
        let id = channel.clone();
        let poll = self
            .ask_helix(|helix| async move { helix.end_poll(id.as_str(), &running, archive).await })
            .await?;
        self.polls.remove(&channel);
        self.emit(Event::PollUpdate {
            channel,
            poll: from_helix_poll(poll),
        })
        .await;
        Ok(())
    }

    async fn create_prediction(
        &mut self,
        channel: UserId,
        title: String,
        outcomes: Vec<String>,
        seconds: u32,
    ) -> CommandResult {
        self.require_own_channel(&channel)?;
        let id = channel.clone();
        let prediction = self
            .ask_helix(|helix| async move {
                helix
                    .create_prediction(id.as_str(), &title, &outcomes, seconds)
                    .await
            })
            .await?;
        let prediction = from_helix_prediction(prediction);
        self.predictions
            .insert(channel.clone(), prediction.id.clone());
        self.emit(Event::PredictionUpdate {
            channel,
            prediction,
        })
        .await;
        Ok(())
    }

    /// Closes betting, pays out, or calls it off. Only paying out leaves
    /// nothing running; a locked prediction is still the channel's.
    async fn end_prediction(
        &mut self,
        channel: UserId,
        how: liveside_proto::PredictionEnd,
    ) -> CommandResult {
        self.require_own_channel(&channel)?;
        let Some(running) = self.predictions.get(&channel).cloned() else {
            return Err(error(
                ErrorKind::InvalidArgument,
                "prediction.none",
                &[],
                "No prediction is running".to_owned(),
            ));
        };
        let (status, winner) = match &how {
            liveside_proto::PredictionEnd::Lock => ("LOCKED", None),
            liveside_proto::PredictionEnd::Resolve { outcome } => {
                ("RESOLVED", Some(outcome.clone()))
            }
            liveside_proto::PredictionEnd::Cancel => ("CANCELED", None),
        };
        let id = channel.clone();
        let prediction = self
            .ask_helix(|helix| async move {
                helix
                    .end_prediction(id.as_str(), &running, status, winner.as_deref())
                    .await
            })
            .await?;
        if !matches!(how, liveside_proto::PredictionEnd::Lock) {
            self.predictions.remove(&channel);
        }
        self.emit(Event::PredictionUpdate {
            channel,
            prediction: from_helix_prediction(prediction),
        })
        .await;
        Ok(())
    }

    async fn create_reward(
        &mut self,
        channel: UserId,
        title: String,
        cost: u64,
        prompt: String,
    ) -> CommandResult {
        self.require_own_channel(&channel)?;
        let id = channel.clone();
        let reward = self
            .ask_helix(|helix| async move {
                helix
                    .create_custom_reward(id.as_str(), &title, cost, &prompt)
                    .await
            })
            .await?;
        self.manageable_rewards
            .entry(channel.clone())
            .or_default()
            .insert(reward.id);
        self.show_rewards(channel).await
    }

    /// Changes a reward, by its id or by its name: a command takes the
    /// name, because nobody is going to type a Twitch id into chat.
    #[allow(clippy::too_many_arguments)]
    async fn update_reward(
        &mut self,
        channel: UserId,
        reward: String,
        title: Option<String>,
        cost: Option<u64>,
        prompt: Option<String>,
        enabled: Option<bool>,
        paused: Option<bool>,
    ) -> CommandResult {
        self.require_own_channel(&channel)?;
        let id = self.reward_id(&channel, &reward).await?;
        let mut change = serde_json::Map::new();
        if let Some(title) = title {
            change.insert("title".to_owned(), serde_json::Value::from(title));
        }
        if let Some(cost) = cost {
            change.insert("cost".to_owned(), serde_json::Value::from(cost));
        }
        if let Some(prompt) = prompt {
            // A prompt without the flag is refused, and clearing it means
            // the reward stops asking.
            let asks = !prompt.is_empty();
            change.insert("prompt".to_owned(), serde_json::Value::from(prompt));
            change.insert(
                "is_user_input_required".to_owned(),
                serde_json::Value::Bool(asks),
            );
        }
        if let Some(enabled) = enabled {
            change.insert("is_enabled".to_owned(), serde_json::Value::Bool(enabled));
        }
        if let Some(paused) = paused {
            change.insert("is_paused".to_owned(), serde_json::Value::Bool(paused));
        }
        let change = serde_json::Value::Object(change);
        let broadcaster = channel.clone();
        self.ask_helix(|helix| async move {
            helix
                .update_custom_reward(broadcaster.as_str(), &id, &change)
                .await
        })
        .await?;
        self.show_rewards(channel).await
    }

    /// The id of a reward named by id or by title, among the ones this
    /// client may change at all.
    async fn reward_id(&mut self, channel: &UserId, wanted: &str) -> Result<String, CommandError> {
        let id = channel.clone();
        let ours = self
            .ask_helix(|helix| async move { helix.custom_rewards(id.as_str(), true).await })
            .await?;
        ours.iter()
            .find(|reward| reward.id == wanted || reward.title.eq_ignore_ascii_case(wanted))
            .map(|reward| reward.id.clone())
            .ok_or_else(|| {
                error(
                    ErrorKind::InvalidArgument,
                    "reward.unknown",
                    &[("reward", wanted)],
                    format!("No reward of ours called {wanted}"),
                )
            })
    }

    async fn delete_reward(&mut self, channel: UserId, reward: String) -> CommandResult {
        self.require_own_channel(&channel)?;
        let id = channel.clone();
        let gone = reward.clone();
        self.ask_helix(|helix| async move { helix.delete_custom_reward(id.as_str(), &gone).await })
            .await?;
        if let Some(ours) = self.manageable_rewards.get_mut(&channel) {
            ours.remove(&reward);
        }
        self.show_rewards(channel).await
    }

    /// Fulfilled, or refunded. Twitch takes this only from the
    /// application that created the reward, which is why the queue holds
    /// nothing else.
    async fn answer_redemption(
        &mut self,
        channel: UserId,
        reward: String,
        redemption: String,
        fulfil: bool,
    ) -> CommandResult {
        self.require_own_channel(&channel)?;
        let id = channel.clone();
        let answered = self
            .ask_helix(|helix| async move {
                helix
                    .answer_redemption(id.as_str(), &reward, &redemption, fulfil)
                    .await
            })
            .await?;
        self.emit(Event::RedemptionUpdate {
            channel,
            redemption: from_helix_redemption(answered),
        })
        .await;
        Ok(())
    }

    /// What the channel says about itself, for the form that edits it.
    async fn show_channel_info(&mut self, channel: UserId) -> CommandResult {
        let id = channel.clone();
        let info = self
            .ask_helix(|helix| async move { helix.channel_information(id.as_str()).await })
            .await?;
        self.emit(Event::ChannelInfo {
            channel,
            info: liveside_proto::model::ChannelInfo {
                title: info.title,
                game: info.game_name,
                language: info.broadcaster_language,
                tags: info.tags,
                delay: info.delay,
                labels: info.content_classification_labels,
            },
        })
        .await;
        Ok(())
    }

    async fn add_segment(
        &mut self,
        channel: UserId,
        start: String,
        minutes: u32,
        title: String,
        weekly: bool,
    ) -> CommandResult {
        self.require_own_channel(&channel)?;
        let id = channel.clone();
        let zone = local_timezone();
        self.ask_helix(|helix| async move {
            helix
                .add_segment(id.as_str(), &start, &zone, minutes, &title, weekly)
                .await
        })
        .await?;
        self.show_schedule(channel).await
    }

    async fn cancel_segment(
        &mut self,
        channel: UserId,
        segment: String,
        canceled: bool,
    ) -> CommandResult {
        self.require_own_channel(&channel)?;
        let id = channel.clone();
        self.ask_helix(|helix| async move {
            helix.cancel_segment(id.as_str(), &segment, canceled).await
        })
        .await?;
        self.show_schedule(channel).await
    }

    async fn delete_segment(&mut self, channel: UserId, segment: String) -> CommandResult {
        self.require_own_channel(&channel)?;
        let id = channel.clone();
        self.ask_helix(|helix| async move { helix.delete_segment(id.as_str(), &segment).await })
            .await?;
        self.show_schedule(channel).await
    }

    async fn set_vacation(
        &mut self,
        channel: UserId,
        away: Option<(String, String)>,
    ) -> CommandResult {
        self.require_own_channel(&channel)?;
        let id = channel.clone();
        let zone = local_timezone();
        self.ask_helix(|helix| async move {
            let away = away.as_ref().map(|(a, b)| (a.as_str(), b.as_str()));
            helix.set_vacation(id.as_str(), away, &zone).await
        })
        .await?;
        self.show_schedule(channel).await
    }

    /// What the channel has planned, for the window that shows it.
    async fn show_schedule(&mut self, channel: UserId) -> CommandResult {
        let id = channel.clone();
        let found = self
            .ask_helix(|helix| async move { helix.schedule(id.as_str()).await })
            .await?;
        let schedule = liveside_proto::model::Schedule {
            segments: found
                .segments
                .into_iter()
                .map(|segment| liveside_proto::model::ScheduleSegment {
                    id: segment.id,
                    start: segment.start_time,
                    end: segment.end_time,
                    title: segment.title,
                    category: segment.category.map(|c| c.name).unwrap_or_default(),
                    weekly: segment.is_recurring,
                    // Twitch says "cancelled until", which for one
                    // occurrence means that occurrence is off.
                    canceled: segment.canceled_until.is_some(),
                })
                .collect(),
            vacation: found.vacation.map(|away| (away.start_time, away.end_time)),
        };
        self.emit(Event::Schedule { channel, schedule }).await;
        Ok(())
    }

    /// When the next advertisement break is due, and — with `snooze` —
    /// pushing it back first. Both answer the same way, because what
    /// anybody wants to know afterwards is when it is now due.
    async fn show_ads(&mut self, channel: UserId, snooze: bool) -> CommandResult {
        self.require_own_channel(&channel)?;
        let id = channel.clone();
        let schedule = match snooze {
            true => {
                self.ask_helix(|helix| async move { helix.snooze_ad(id.as_str()).await })
                    .await?
            }
            false => {
                self.ask_helix(|helix| async move { helix.ad_schedule(id.as_str()).await })
                    .await?
            }
        };
        // Twitch writes "no break scheduled" as a zero rather than as
        // nothing at all, and sends the time as a number where the
        // documentation promises a string.
        let next = schedule.next_at();
        let none = next.is_empty() || next.starts_with("1970");
        let (code, args): (&str, Vec<(&str, String)>) = match none {
            true => ("ads.none", Vec::new()),
            false => (
                "ads.next",
                vec![
                    // The day and the hour; the seconds of a break due
                    // in twenty minutes are nobody's business. A unix
                    // timestamp is passed on as it stands, since that is
                    // what Twitch sends and inventing a date from it
                    // here would be a second guess.
                    (
                        "at",
                        next.replace('T', " ").trim_end_matches('Z').to_owned(),
                    ),
                    ("seconds", schedule.duration.to_string()),
                    ("snoozes", schedule.snooze_count.to_string()),
                ],
            ),
        };
        let args: Vec<(&str, &str)> = args.iter().map(|(k, v)| (*k, v.as_str())).collect();
        let text = english_event(code, &args);
        self.notice(&channel, NoticeLevel::Info, code, &args, text)
            .await;
        Ok(())
    }

    /// Whispers somebody. Twitch wants the other person's id, and a
    /// verified phone number on this account — which it only says when
    /// it refuses, so the refusal is passed on as it comes.
    async fn send_whisper(&mut self, user: Login, text: String) -> CommandResult {
        let me = self.require_account()?.user.id.to_string();
        let login = user.to_string();
        let found = self
            .ask_helix(move |helix| async move { helix.users_by_login(&[&login]).await })
            .await?;
        let Some(other) = found.into_iter().next() else {
            return Err(error(
                ErrorKind::InvalidArgument,
                "user.unknown",
                &[("user", user.as_str())],
                format!("No such user {user}"),
            ));
        };
        let to = other.id.to_string();
        let said = text.clone();
        self.ask_helix(move |helix| async move { helix.send_whisper(&me, &to, &said).await })
            .await?;
        // Twitch does not send back what we whispered, so the thread is
        // told about it here.
        let account = self.require_account()?;
        let line = Line {
            time: Timestamp::now(),
            content: LineContent::Message(liveside_proto::model::ChatMessage {
                id: format!("whisper-{}", Timestamp::now().0).into(),
                sender: account.user.clone(),
                color: None,
                badges: Vec::new(),
                fragments: vec![liveside_proto::model::Fragment::Text { text }],
                kind: liveside_proto::model::MessageKind::Text,
                bits: None,
                reply: None,
                shared_source: None,
            }),
            flags: liveside_proto::model::LineFlags {
                own: true,
                ..Default::default()
            },
        };
        self.emit(Event::Whisper { user: other, line }).await;
        Ok(())
    }

    /// Clips what just happened. Twitch answers before the clip exists,
    /// so this says where it will be rather than that it is there — and
    /// the URL is the one for trimming and titling it, which is what
    /// anybody who just clipped something wants next.
    async fn create_clip(&mut self, channel: UserId) -> CommandResult {
        let id = channel.clone();
        let (_, edit_url) = self
            .ask_helix(|helix| async move { helix.create_clip(id.as_str()).await })
            .await?;
        self.notice(
            &channel,
            NoticeLevel::Info,
            "clip.created",
            &[("url", &edit_url)],
            format!("Clip made: {edit_url}"),
        )
        .await;
        Ok(())
    }

    /// What the channel offers, with a mark on the ones this client may
    /// answer for. Twitch gives the two lists separately, so this asks
    /// twice and compares them.
    async fn show_rewards(&mut self, channel: UserId) -> CommandResult {
        self.require_own_channel(&channel)?;
        let id = channel.clone();
        let all = self
            .ask_helix(|helix| async move { helix.custom_rewards(id.as_str(), false).await })
            .await?;
        let id = channel.clone();
        let ours: std::collections::HashSet<String> = self
            .ask_helix(|helix| async move { helix.custom_rewards(id.as_str(), true).await })
            .await
            .unwrap_or_default()
            .into_iter()
            .map(|reward| reward.id)
            .collect();
        self.manageable_rewards
            .insert(channel.clone(), ours.clone());
        let rewards = all
            .into_iter()
            .map(|reward| liveside_proto::model::Reward {
                ours: ours.contains(&reward.id),
                id: reward.id,
                title: reward.title,
                cost: reward.cost,
                prompt: reward.prompt,
                enabled: reward.is_enabled,
                paused: reward.is_paused,
            })
            .collect();
        self.emit(Event::Rewards { channel, rewards }).await;
        Ok(())
    }

    async fn show_people(&mut self, channel: UserId, list: PeopleList) -> CommandResult {
        if list == PeopleList::Followers || list == PeopleList::Subscribers {
            return self.show_counted(channel, list).await;
        }
        self.require_own_channel(&channel)?;
        let id = channel.clone();
        let people: Vec<liveside_proto::event::Person> = match list {
            PeopleList::Moderators => self
                .ask_helix(|helix| async move { helix.moderators(id.as_str()).await })
                .await?
                .into_iter()
                .map(|name| liveside_proto::event::Person {
                    name,
                    detail: String::new(),
                    gifted_by: String::new(),
                })
                .collect(),
            PeopleList::Vips => self
                .ask_helix(|helix| async move { helix.vips(id.as_str()).await })
                .await?
                .into_iter()
                .map(|name| liveside_proto::event::Person {
                    name,
                    detail: String::new(),
                    gifted_by: String::new(),
                })
                .collect(),
            PeopleList::Banned => self
                .ask_helix(|helix| async move { helix.banned(id.as_str()).await })
                .await?
                .into_iter()
                // A timeout runs out; a ban does not, and the difference
                // is the whole point of the list.
                .map(|(name, until)| liveside_proto::event::Person {
                    name,
                    detail: until.unwrap_or_default(),
                    gifted_by: String::new(),
                })
                .collect(),
            PeopleList::Followers | PeopleList::Subscribers => unreachable!("counted above"),
        };
        self.emit(Event::People {
            channel,
            list: match list {
                PeopleList::Moderators => liveside_proto::event::PeopleList::Moderators,
                PeopleList::Vips => liveside_proto::event::PeopleList::Vips,
                PeopleList::Banned => liveside_proto::event::PeopleList::Banned,
                PeopleList::Followers | PeopleList::Subscribers => unreachable!("counted above"),
            },
            people,
            total: None,
            points: None,
        })
        .await;
        Ok(())
    }

    /// Followers or subscribers: lists too long to read whole, so the
    /// newest few hundred and how many there are. Anyone may count a
    /// channel's followers and Twitch names them only to the channel and
    /// its moderators, so it is asked and Twitch decides; subscribers are
    /// the channel's own business.
    async fn show_counted(&mut self, channel: UserId, list: PeopleList) -> CommandResult {
        const SHOWN: usize = 500;
        let id = channel.clone();
        let (kind, people, total, points) = if list == PeopleList::Subscribers {
            self.require_own_channel(&channel)?;
            let read = self
                .ask_helix(|helix| async move { helix.subscribers(id.as_str(), SHOWN).await })
                .await?;
            let people = read
                .items
                .into_iter()
                .map(|sub| liveside_proto::event::Person {
                    name: sub.user_name,
                    // Counted in thousands by Twitch, 1 to 3 by people.
                    detail: match sub.tier.as_str() {
                        "2000" => "2".to_owned(),
                        "3000" => "3".to_owned(),
                        _ => "1".to_owned(),
                    },
                    gifted_by: if sub.is_gift {
                        sub.gifter_name
                    } else {
                        String::new()
                    },
                })
                .collect();
            (
                liveside_proto::event::PeopleList::Subscribers,
                people,
                read.total,
                read.points,
            )
        } else {
            self.require_account()?;
            let read = self
                .ask_helix(|helix| async move { helix.followers(id.as_str(), SHOWN).await })
                .await?;
            let people = read
                .items
                .into_iter()
                .map(|follower| liveside_proto::event::Person {
                    name: follower.user_name,
                    detail: follower.followed_at,
                    gifted_by: String::new(),
                })
                .collect();
            (
                liveside_proto::event::PeopleList::Followers,
                people,
                read.total,
                None,
            )
        };
        self.emit(Event::People {
            channel,
            list: kind,
            people,
            total: Some(total),
            points,
        })
        .await;
        Ok(())
    }

    /// Says which channels this account moderates, by name and display
    /// name, sorted the way a list is read.
    async fn show_moderated_channels(&mut self) -> CommandResult {
        let user = self.require_account()?.user.id.to_string();
        let mut channels: Vec<Channel> = self
            .ask_helix(|helix| async move { helix.moderated_channels(&user).await })
            .await?
            .into_iter()
            .map(|channel| Channel {
                id: channel.broadcaster_id.into(),
                login: channel.broadcaster_login.into(),
                display_name: channel.broadcaster_name,
            })
            .collect();
        channels.sort_by(|a, b| a.login.as_str().cmp(b.login.as_str()));
        self.emit(Event::ModeratedChannels { channels }).await;
        Ok(())
    }

    /// Says which channels this account follows and which of them
    /// stream. A token from before this existed lacks the permission, and
    /// is told so before anything is asked.
    async fn show_followed_channels(&mut self) -> CommandResult {
        let account = self.require_account()?;
        if !account.scopes.iter().any(|s| s == FOLLOWS_SCOPE) {
            return Err(missing_scope(FOLLOWS_SCOPE));
        }
        let user = account.user.id.to_string();
        let helix = self.require_helix()?.clone();
        // Both at once: with a few hundred follows the channel list alone
        // is several pages.
        let (channels, streams) = tokio::join!(
            helix.followed_channels(&user),
            helix.followed_streams(&user)
        );
        let channels = followed_list(
            channels.map_err(from_twitch)?,
            streams.map_err(from_twitch)?,
        );
        self.emit(Event::FollowedChannels { channels }).await;
        Ok(())
    }

    /// Says what the channel blocks: a table in the window, a line in
    /// the terminal, each frontend its own way.
    async fn show_blocked_terms(&mut self, channel: UserId) -> CommandResult {
        let moderator = self.require_account()?.user.id.to_string();
        let id = channel.clone();
        let terms = self
            .ask_helix(|helix| async move { helix.blocked_terms(id.as_str(), &moderator).await })
            .await?
            .into_iter()
            .map(|term| liveside_proto::model::BlockedTerm {
                id: term.id,
                text: term.text,
                created_at: term.created_at,
                expires_at: term.expires_at,
            })
            .collect();
        self.emit(Event::BlockedTerms { channel, terms }).await;
        Ok(())
    }

    /// Marks this moment in the stream and says where it landed.
    async fn marker(&mut self, channel: UserId, description: Option<String>) -> CommandResult {
        // No check of our own here: Twitch takes a marker from the
        // broadcaster *or* from one of their editors, and nothing tells
        // this client which channels it is an editor of. Its refusal
        // names editors itself, which is better than a guess.
        let id = channel.clone();
        let at = self
            .ask_helix(|helix| async move {
                helix
                    .create_marker(id.as_str(), description.as_deref())
                    .await
            })
            .await?;
        let (minutes, seconds) = (at / 60, at % 60);
        let position = format!("{minutes}:{seconds:02}");
        self.notice(
            &channel,
            NoticeLevel::Info,
            "marker.set",
            &[("position", &position)],
            format!("Marker set at {position} into the stream"),
        )
        .await;
        Ok(())
    }

    /// Runs an advertisement and repeats what Twitch said about it.
    async fn commercial(&mut self, channel: UserId, seconds: u32) -> CommandResult {
        let id = channel.clone();
        let said = self
            .ask_helix(|helix| async move { helix.start_commercial(id.as_str(), seconds).await })
            .await?;
        if !said.is_empty() {
            // Twitch's own wording about what it managed to serve.
            self.notice(&channel, NoticeLevel::Info, "", &[], said)
                .await;
        }
        Ok(())
    }

    /// Sets the title, the category, or both. A category is given by name,
    /// as it is typed, so it has to be looked up first.
    #[allow(clippy::too_many_arguments)]
    async fn set_channel_info(
        &mut self,
        channel: UserId,
        title: Option<String>,
        game: Option<String>,
        language: Option<String>,
        tags: Option<Vec<String>>,
        delay: Option<u32>,
        labels: Option<Vec<String>>,
    ) -> CommandResult {
        let name = self.channel_name(&channel);
        let mut change = serde_json::Map::new();
        if let Some(title) = &title {
            change.insert("title".to_owned(), serde_json::Value::from(title.clone()));
        }
        let mut found: Option<String> = None;
        if let Some(game) = &game {
            let wanted = game.clone();
            let found_game = self
                .ask_helix(|helix| async move { helix.game_by_name(&wanted).await })
                .await?;
            let Some(found_game) = found_game else {
                return Err(error(
                    ErrorKind::InvalidArgument,
                    "game.unknown",
                    &[("game", game)],
                    format!("Twitch has no category called {game}"),
                ));
            };
            let game = found_game;
            change.insert("game_id".to_owned(), serde_json::Value::from(game.id));
            found = Some(game.name);
        }
        if let Some(language) = &language {
            change.insert(
                "broadcaster_language".to_owned(),
                serde_json::Value::from(language.clone()),
            );
        }
        if let Some(tags) = &tags {
            change.insert("tags".to_owned(), serde_json::Value::from(tags.clone()));
        }
        if let Some(delay) = delay {
            change.insert("delay".to_owned(), serde_json::Value::from(delay));
        }
        // Twitch takes the labels as a set of switches rather than a
        // list, and clearing them means saying "off" to every one.
        if let Some(labels) = &labels {
            let all = [
                "DebatedSocialIssuesAndPolitics",
                "DrugsIntoxication",
                "SexualThemes",
                "ViolentGraphic",
                "Gambling",
                "ProfanityVulgarity",
            ];
            let switches: Vec<serde_json::Value> = all
                .iter()
                .map(|name| {
                    serde_json::json!({
                        "id": name,
                        "is_enabled": labels.iter().any(|set| set == name),
                    })
                })
                .collect();
            change.insert(
                "content_classification_labels".to_owned(),
                serde_json::Value::from(switches),
            );
        }
        let change = serde_json::Value::Object(change);
        let id = channel.clone();
        self.with_helix(|helix| async move { helix.update_channel(id.as_str(), &change).await })
            .await?;
        // Nothing comes back to say it worked, so the channel is told.
        if let Some(title) = &title {
            self.notice(
                &channel,
                NoticeLevel::Info,
                "channel.title_set",
                &[("channel", &name), ("title", title)],
                format!("Title of #{name} is now: {title}"),
            )
            .await;
        }
        if let Some(game) = &found {
            self.notice(
                &channel,
                NoticeLevel::Info,
                "channel.game_set",
                &[("channel", &name), ("game", game)],
                format!("#{name} is now streaming {game}"),
            )
            .await;
        }
        Ok(())
    }

    /// Starts the device code flow, unless a valid token is already in hand.
    async fn login(&mut self, token: u64) -> Outcome {
        if self.account.is_some() {
            return Outcome::Done(Ok(()));
        }
        if self.pending_login.is_some() {
            return Outcome::Done(Err(error(
                ErrorKind::InvalidArgument,
                "login.in_progress",
                &[],
                "A login is already in progress",
            )));
        }
        self.pending_login = Some(token);
        let scopes: Vec<&str> = self.config.scopes.iter().map(String::as_str).collect();
        let code: DeviceCode = match self.auth.start_device_flow(&scopes).await {
            Ok(code) => code,
            Err(e) => {
                self.pending_login = None;
                return Outcome::Done(Err(from_twitch(e)));
            }
        };
        self.emit(Event::LoginPending {
            uri: code.verification_uri.clone(),
            code: code.user_code.clone(),
            expires_in_secs: code.expires_in,
        })
        .await;

        // Polling can take minutes; the engine must not wait for it, so the
        // result comes back through the login channel.
        let auth = self.auth.clone();
        let login_out = self.login_out.clone();
        tokio::spawn(async move {
            let result = auth.poll_for_token(&code).await;
            let _ = login_out.send(LoginOutcome { token, result }).await;
        });
        Outcome::Deferred
    }

    async fn logout(&mut self) {
        // Twitch is told first, while the token is still at hand.
        // Deleting the file alone would leave it working until it
        // expired, with everything this client subscribed to still
        // running on it.
        if let Ok(Some(tokens)) = liveside_twitch::auth::load_tokens(&self.config.token_path)
            && let Err(e) = self.auth.revoke(&tokens.access_token).await
        {
            self.log(
                NoticeLevel::Warning,
                "logout.not_revoked",
                &[("error", &e.to_string())],
                format!("Twitch would not forget this login: {e}"),
            )
            .await;
        }
        // Closed rather than forgotten: Twitch keeps a socket's
        // subscriptions enabled after the token behind them is revoked,
        // and the next login's socket would run beside it.
        self.close_sockets();
        self.helix = None;
        self.account = None;
        // Its login was this account's; the next one opens its own.
        self.speaker = Speaker {
            events: self.speaker.events.take(),
            ..Speaker::default()
        };
        self.followed_live = None;
        self.session_id = None;
        self.channels.clear();
        self.reconnect_at = None;
        self.account_subscriptions.clear();
        let _ = liveside_twitch::auth::delete_tokens(&self.config.token_path);
        self.emit(Event::LoggedOut).await;
        self.emit(Event::Connection {
            state: ConnectionState::Disconnected,
        })
        .await;
    }

    async fn on_login(&mut self, outcome: LoginOutcome) {
        let result = match outcome.result {
            Ok(tokens) => {
                let _ = liveside_twitch::auth::save_tokens(&self.config.token_path, &tokens);
                match self.use_tokens(tokens).await {
                    Ok(()) => Ok(()),
                    Err(e) => {
                        self.emit(Event::LoginFailed {
                            reason: e.to_string(),
                        })
                        .await;
                        Err(from_twitch(e))
                    }
                }
            }
            Err(e) => {
                self.emit(Event::LoginFailed {
                    reason: e.to_string(),
                })
                .await;
                Err(from_twitch(e))
            }
        };
        self.pending_login = None;
        let _ = self.done(outcome.token, result).await;
    }

    /// Joins one channel: everything is asked for first, then applied.
    async fn join(&mut self, login: &Login) -> CommandResult {
        // Already in: asking Twitch again would only be refused, since
        // the session has every subscription the channel needs.
        if self.channels.values().any(|joined| &joined.login == login) {
            return Ok(());
        }
        let helix = self.require_helix()?.clone();
        let session_id = self.session_id.clone().ok_or_else(|| {
            error(
                ErrorKind::Network,
                "twitch.not_connected",
                &[],
                "Not connected to Twitch yet",
            )
        })?;
        let account = self.require_account()?.clone();
        let prepared = prepare_join(&helix, &session_id, &account, login).await?;
        self.apply_join(prepared).await
    }

    /// Writes down a channel that has been asked about and tells the
    /// frontends everything that was learned.
    async fn apply_join(&mut self, prepared: Prepared) -> CommandResult {
        let Prepared {
            channel,
            subscriptions,
            settings,
            requests,
            shared,
            shield,
            stream,
            badges,
            polls,
            predictions,
            rewards,
            hype,
        } = prepared;
        let helix = self.require_helix()?.clone();
        // Two names for one channel, or a second join while the first was
        // still being asked about: drop what the second one made.
        if self.channels.contains_key(&channel.id) {
            for id in &subscriptions {
                let _ = helix.unsubscribe(id).await;
            }
            return Ok(());
        }
        if self.config.third_party_emotes {
            self.load_emotes(Some((channel.id.clone(), channel.login.clone())));
        }
        // Chat settings are nice to have; a failure must not block the join.
        let modes = match settings {
            Ok(modes) => modes,
            Err(e) => {
                self.log(
                    NoticeLevel::Warning,
                    "settings.unreadable",
                    &[
                        ("channel", channel.login.as_str()),
                        ("error", &e.to_string()),
                    ],
                    format!("Cannot read chat settings of #{}: {e}", channel.login),
                )
                .await;
                RoomModes::default()
            }
        };
        self.emit(Event::ChannelJoined {
            channel: channel.clone(),
            modes,
        })
        .await;
        // What is already waiting to be answered. Refused for a channel
        // we do not moderate, which is the ordinary case.
        if let Ok(requests) = requests {
            for request in requests {
                self.emit(Event::UnbanRequested {
                    channel: channel.id.clone(),
                    request: liveside_proto::event::UnbanRequest {
                        id: request.id,
                        user: liveside_proto::model::User {
                            id: request.user_id.into(),
                            login: request.user_login.into(),
                            display_name: request.user_name,
                        },
                        text: request.text,
                        time: Timestamp::now(),
                    },
                })
                .await;
            }
        }
        // A poll or a prediction already running: EventSub says nothing
        // until the next vote, and until then a client that just started
        // would show an empty screen over a channel that is mid-vote.
        for poll in polls.unwrap_or_default() {
            let poll = from_helix_poll(poll);
            if poll.status == liveside_proto::model::PollStatus::Active {
                self.polls.insert(channel.id.clone(), poll.id.clone());
                self.emit(Event::PollUpdate {
                    channel: channel.id.clone(),
                    poll,
                })
                .await;
            }
        }
        for prediction in predictions.unwrap_or_default() {
            use liveside_proto::model::PredictionStatus;
            let prediction = from_helix_prediction(prediction);
            // A locked one matters most: it is waiting to be paid
            // out, and Twitch cancels it after a day.
            if matches!(
                prediction.status,
                PredictionStatus::Active | PredictionStatus::Locked
            ) {
                self.predictions
                    .insert(channel.id.clone(), prediction.id.clone());
                self.emit(Event::PredictionUpdate {
                    channel: channel.id.clone(),
                    prediction,
                })
                .await;
            }
        }
        // The rewards this client created here, and whatever is already
        // waiting on them: a queue that starts empty after a restart
        // would be worse than none at all.
        if let Some(ours) = rewards {
            let ids: std::collections::HashSet<String> =
                ours.iter().map(|reward| reward.id.clone()).collect();
            self.manageable_rewards.insert(channel.id.clone(), ids);
            for reward in ours {
                let Ok(waiting) = helix.redemptions(channel.id.as_str(), &reward.id).await else {
                    continue;
                };
                for redemption in waiting {
                    self.emit(Event::RedemptionUpdate {
                        channel: channel.id.clone(),
                        redemption: from_helix_redemption(redemption),
                    })
                    .await;
                }
            }
        }
        // A Shared Chat session already running, and a Hype Train in
        // progress: both events only say when something changes, so a
        // client that starts in the middle of one would say nothing.
        if let Ok(participants) = shared
            && participants.len() > 1
        {
            self.notice(
                &channel.id,
                NoticeLevel::Info,
                "shared.running",
                &[("count", &(participants.len() - 1).to_string())],
                format!(
                    "Chat is shared with {} other channels",
                    participants.len() - 1
                ),
            )
            .await;
        }
        if let Some(level) = hype {
            self.notice(
                &channel.id,
                NoticeLevel::Info,
                "money.hype_running",
                &[("level", &level.to_string())],
                format!("A Hype Train is running (level {level})"),
            )
            .await;
        }
        // Whether Shield Mode is on. The two events say when it changes;
        // without asking, a client that started mid-attack would show the
        // channel as calm.
        if let Ok(on) = shield {
            self.emit(Event::ShieldMode {
                channel: channel.id.clone(),
                on,
            })
            .await;
            if on {
                self.notice(
                    &channel.id,
                    NoticeLevel::Warning,
                    "event.shield_on",
                    &[],
                    "Shield Mode is on".to_owned(),
                )
                .await;
            }
        }
        // Whether it is streaming, so the frontends can say so beside the
        // name.
        let live_now = !stream.is_empty();
        self.emit(Event::Stream {
            channel: channel.id.clone(),
            live: live_now,
        })
        .await;
        self.irc_join(channel.login.as_str()).await;
        self.channels.insert(
            channel.id.clone(),
            Joined {
                login: channel.login.clone(),
                subscriptions,
                // Filled in when the background fetch finishes.
                emotes: EmoteSet::default(),
                badges,
                live: live_now,
            },
        );
        // And how many watch: the minute's Get Streams would say it too,
        // but only a minute from now.
        self.say_stream_stats(&stream).await;
        Ok(())
    }

    /// Joins the channels asked for that we are not in. Run once the
    /// session is up, which is also the moment a login finishes: the
    /// channels from the settings file were asked for at startup, when
    /// there was no account to join them with.
    async fn join_what_is_wanted(&mut self) {
        let wanted: Vec<(Option<u64>, Login)> = self
            .wanted
            .iter()
            .cloned()
            .map(|login| (None, login))
            .collect();
        self.join_many(wanted).await;
    }

    /// Joins several channels, asking Twitch about all of them at once.
    /// A channel that was asked for by a command carries that command's
    /// token, which is answered when the channel is in or has failed;
    /// the ones from the settings file carry none and only say so in the
    /// status buffer when they fail.
    async fn join_many(&mut self, wanted: Vec<(Option<u64>, Login)>) {
        let joined: Vec<Login> = self
            .channels
            .values()
            .map(|joined| joined.login.clone())
            .collect();
        // Each channel once, where it was first asked for, with everyone
        // who asked: the saved channels and the settings file name the
        // same ones, and two answers for one channel ran at once and
        // fought over its subscriptions, which Twitch has only once a
        // session, and lost it at random.
        let mut todo: Vec<(Vec<u64>, Login)> = Vec::new();
        for (token, login) in wanted {
            if joined.contains(&login) {
                if let Some(token) = token {
                    let _ = self.done(token, Ok(())).await;
                }
                continue;
            }
            match todo.iter_mut().find(|(_, asked)| asked == &login) {
                Some((tokens, _)) => tokens.extend(token),
                None => todo.push((token.into_iter().collect(), login)),
            }
        }
        if todo.is_empty() {
            return;
        }
        let (Some(helix), Some(session_id), Some(account)) = (
            self.helix.clone(),
            self.session_id.clone(),
            self.account.clone(),
        ) else {
            return;
        };
        // All of them asked about at once. Each channel is a dozen round
        // trips to Twitch, and doing them channel after channel left the
        // list filling up one name every six seconds.
        let mut asking = tokio::task::JoinSet::new();
        let count = todo.len();
        for (place, (tokens, login)) in todo.into_iter().enumerate() {
            let helix = helix.clone();
            let session_id = session_id.clone();
            let account = account.clone();
            asking.spawn(async move {
                let prepared = prepare_join(&helix, &session_id, &account, &login).await;
                (place, tokens, login, prepared)
            });
        }
        // Applied one at a time, and in the order they were asked for, so
        // the list comes up the same at every start: an answer that comes
        // early waits for the ones before it. The state behind them is this
        // task's alone.
        let mut answered: Vec<Option<JoinAnswer>> = (0..count).map(|_| None).collect();
        let mut next = 0;
        while let Some(done) = asking.join_next().await {
            let Ok((place, tokens, login, prepared)) = done else {
                continue;
            };
            answered[place] = Some((tokens, login, prepared));
            while let Some(Some((tokens, login, prepared))) =
                answered.get_mut(next).map(Option::take)
            {
                next += 1;
                self.finish_join(tokens, login, prepared).await;
            }
        }
        // A task that panicked leaves a gap; what came after it still
        // counts.
        for (tokens, login, prepared) in answered.into_iter().skip(next).flatten() {
            self.finish_join(tokens, login, prepared).await;
        }
    }

    /// Applies one answer of `join_many` and says how it went: to every
    /// command that asked, or in the log for a join nobody waits for.
    async fn finish_join(
        &mut self,
        tokens: Vec<u64>,
        login: Login,
        prepared: Result<Prepared, CommandError>,
    ) {
        let outcome = match prepared {
            Ok(prepared) => self.apply_join(prepared).await,
            Err(e) => Err(e),
        };
        if !tokens.is_empty() {
            for token in tokens {
                let _ = self.done(token, outcome.clone()).await;
            }
            return;
        }
        if let Err(e) = outcome {
            // A name that will never work is dropped, so a typo in the
            // settings file does not complain again at every reconnect.
            if matches!(e.code.as_str(), "channel.unknown" | "channel.invalid_name") {
                self.wanted.retain(|wanted| wanted != &login);
            }
            self.log(
                NoticeLevel::Warning,
                &e.code,
                &e.args
                    .iter()
                    .map(|(name, value)| (name.as_str(), value.as_str()))
                    .collect::<Vec<_>>(),
                e.message.clone(),
            )
            .await;
        }
    }

    async fn part(&mut self, channel: &UserId) -> CommandResult {
        if let Some(joined) = self.channels.get(channel) {
            let login = joined.login.clone();
            self.wanted.retain(|wanted| wanted != &login);
        }
        let Some(joined) = self.channels.remove(channel) else {
            return Err(error(
                ErrorKind::NotJoined,
                "channel.not_joined",
                &[("channel", channel.as_str())],
                format!("Not in channel {channel}"),
            ));
        };
        if let Some(helix) = &self.helix {
            for id in &joined.subscriptions {
                // Leaving locally matters more than tidying up at Twitch.
                let _ = helix.unsubscribe(id).await;
            }
        }
        if let Some(irc) = &self.irc_out {
            let _ = irc
                .send(liveside_twitch::irc::Command::Part(
                    joined.login.to_string(),
                ))
                .await;
        }
        self.emit(Event::ChannelParted {
            channel: channel.clone(),
        })
        .await;
        Ok(())
    }

    async fn ban(
        &mut self,
        channel: UserId,
        user: UserTarget,
        seconds: Option<u32>,
        reason: Option<String>,
    ) -> CommandResult {
        let moderator = self.require_account()?.user.id.to_string();
        let user_id = self.resolve(&user).await?;
        let helix = self.require_helix()?.clone();
        helix
            .ban(
                channel.as_str(),
                &moderator,
                &user_id,
                seconds,
                reason.as_deref(),
            )
            .await
            .map_err(from_twitch)
    }

    /// Turns a login into a user id; ids pass through.
    async fn resolve(&self, user: &UserTarget) -> Result<String, CommandError> {
        match user {
            UserTarget::Id(id) => Ok(id.to_string()),
            UserTarget::Login(login) => {
                let helix = self.require_helix()?;
                let users = helix
                    .users_by_login(&[login.as_str()])
                    .await
                    .map_err(from_twitch)?;
                users
                    .into_iter()
                    .next()
                    .map(|user| user.id.to_string())
                    .ok_or_else(|| {
                        error(
                            ErrorKind::InvalidArgument,
                            "user.unknown",
                            &[("user", login.as_str())],
                            format!("No such user {login}"),
                        )
                    })
            }
        }
    }

    fn require_account(&self) -> Result<&Account, CommandError> {
        self.account.as_ref().ok_or_else(|| {
            error(
                ErrorKind::NotLoggedIn,
                "account.required",
                &[],
                "Log in first (/login)",
            )
        })
    }

    fn require_helix(&self) -> Result<&Helix, CommandError> {
        self.helix.as_ref().ok_or_else(|| {
            error(
                ErrorKind::NotLoggedIn,
                "account.required",
                &[],
                "Log in first (/login)",
            )
        })
    }

    /// Runs a Helix call, mapping its error.
    async fn with_helix<F, Fut>(&self, call: F) -> CommandResult
    where
        F: FnOnce(Helix) -> Fut,
        Fut: std::future::Future<Output = liveside_twitch::Result<()>>,
    {
        let helix = self.require_helix()?.clone();
        call(helix).await.map_err(from_twitch)
    }

    /// The same, for a call that answers with something.
    async fn ask_helix<T, F, Fut>(&self, call: F) -> Result<T, liveside_proto::CommandError>
    where
        F: FnOnce(Helix) -> Fut,
        Fut: std::future::Future<Output = liveside_twitch::Result<T>>,
    {
        let helix = self.require_helix()?.clone();
        call(helix).await.map_err(from_twitch)
    }

    async fn on_socket(&mut self, from: u64, message: eventsub::Message) {
        let current = self.socket.as_ref().map(|(number, _)| *number);
        let next = self.socket_next.as_ref().map(|(number, _)| *number);
        // A socket replaced or closed may have said something on its way
        // out; none of it is about the session in use.
        if Some(from) != current && Some(from) != next {
            return;
        }
        match message {
            eventsub::Message::Welcome {
                session_id,
                resumed,
            } => {
                // The new socket takes over, and the old one is closed:
                // left open, it would go on delivering every event a
                // second time.
                if Some(from) == next {
                    if let Some((_, old)) = self.socket.take() {
                        old.abort();
                    }
                    self.socket = self.socket_next.take();
                }
                self.session_id = Some(session_id.clone());
                // Failed tries before this one mean the network was gone,
                // and with it, perhaps, emotes being loaded.
                let after_outage = self.attempt > 0;
                self.attempt = 0;
                self.emit(Event::Connection {
                    state: ConnectionState::Connected,
                })
                .await;
                // A session Twitch moved us to already has everything the
                // last one had. Subscribing again would make a second set
                // of every one of them, which is how four channels came
                // to cost more than a token is allowed to spend.
                if resumed {
                    self.said_broken.clear();
                    self.refresh_live().await;
                } else {
                    self.resubscribe(&session_id).await;
                }
                let pending: Vec<(Option<u64>, Login)> = std::mem::take(&mut self.pending_joins)
                    .into_iter()
                    .map(|(token, channel)| (Some(token), channel))
                    .collect();
                self.join_many(pending).await;
                self.join_what_is_wanted().await;
                if after_outage && self.config.third_party_emotes {
                    for scope in self.emotes_to_retry() {
                        self.load_emotes(scope);
                    }
                }
            }
            eventsub::Message::Notification { kind, event } => {
                self.on_notification(&kind, event).await;
            }
            eventsub::Message::Revoked { kind, status } => {
                // Twitch revokes a subscription when the token that made
                // it goes; opening the socket again makes every one of
                // them afresh, which is what "try rejoining" used to ask
                // the reader to do by hand.
                self.log(
                    NoticeLevel::Warning,
                    "eventsub.revoked",
                    &[("kind", &kind), ("status", &status)],
                    format!("Twitch revoked {kind} ({status}); subscribing again"),
                )
                .await;
                self.connect_socket();
            }
            eventsub::Message::Closed {
                reason,
                reconnect_to,
            } => {
                if Some(from) == current {
                    self.socket = None;
                    // Its replacement is on the way; that one says how
                    // it went.
                    if self.socket_next.is_some() {
                        self.session_id = None;
                        return;
                    }
                    self.session_id = None;
                } else {
                    // The replacement could not be opened; the socket in
                    // use, if any, goes on, and another try is scheduled.
                    self.socket_next = None;
                }
                if self.account.is_none() {
                    return; // Logged out on purpose.
                }
                // Twitch moving us is not a failure: it offers the new
                // URL half a minute ahead, so go there at once and do not
                // count it against the backoff.
                if let Some(url) = reconnect_to {
                    self.reconnect_to = Some(url);
                    self.reconnect_at = Some(Instant::now());
                    self.emit(Event::Connection {
                        state: ConnectionState::Connecting,
                    })
                    .await;
                    return;
                }
                self.attempt += 1;
                let delay = Duration::from_secs(2u64.pow(self.attempt.min(4)));
                self.reconnect_at = Some(Instant::now() + delay);
                self.emit(Event::Connection {
                    state: ConnectionState::Reconnecting {
                        attempt: self.attempt,
                        reason,
                    },
                })
                .await;
            }
        }
    }

    /// After a reconnect the old session is gone, so every joined channel
    /// needs its subscriptions again.
    async fn resubscribe(&mut self, session_id: &str) {
        let Some(helix) = self.helix.clone() else {
            return;
        };
        // Whatever the last socket made is no use to this one, and
        // Twitch does not always drop it when the socket goes: leftovers
        // stay enabled, count against the ten points a token may spend
        // and fill the three hundred a connection may hold. So they go
        // first, all at once, and a failure is ignored — a subscription
        // Twitch has already forgotten is exactly what we wanted.
        let stale: Vec<String> = self
            .channels
            .values_mut()
            .flat_map(|joined| std::mem::take(&mut joined.subscriptions))
            .chain(std::mem::take(&mut self.account_subscriptions))
            .collect();
        if !stale.is_empty() {
            let mut dropping = tokio::task::JoinSet::new();
            for id in stale {
                let helix = helix.clone();
                dropping.spawn(async move { helix.unsubscribe(&id).await });
            }
            while dropping.join_next().await.is_some() {}
        }
        // About this account rather than about a channel: a whisper
        // reaches us wherever we are, so it is subscribed once per
        // session rather than once per channel.
        if let Some(account) = self.account.clone() {
            let condition = serde_json::json!({ "user_id": account.user.id.as_str() });
            for (kind, version) in eventsub::ACCOUNT_SUBSCRIPTIONS {
                if let Ok(subscription) = helix
                    .subscribe(kind, version, condition.clone(), session_id)
                    .await
                {
                    self.account_subscriptions.push(subscription.id);
                }
            }
        }
        let channels: Vec<UserId> = self.channels.keys().cloned().collect();
        // Who is streaming, asked again now rather than trusted from
        // before: while this client was away, stream.offline came and
        // went, and the green dot beside a name would stay on for days.
        let ids: Vec<String> = channels.iter().map(|c| c.to_string()).collect();
        if let Ok(live) = helix.live_among(&ids).await {
            self.say_stream_stats(&live).await;
            for channel in &channels {
                let now = live.iter().any(|stream| stream.user_id == channel.as_str());
                if let Some(joined) = self.channels.get_mut(channel) {
                    joined.live = now;
                }
                self.emit(Event::Stream {
                    channel: channel.clone(),
                    live: now,
                })
                .await;
            }
        }
        // Every channel subscribed again at the same time: each one is
        // forty-odd round trips, and after a token renewal with four
        // channels open, one at a time left chat silent for a minute.
        let Some(account) = self.account.clone() else {
            return;
        };
        let mut asking = tokio::task::JoinSet::new();
        for channel in channels {
            let helix = helix.clone();
            let account = account.clone();
            let session_id = session_id.to_owned();
            asking.spawn(async move {
                let made = subscribe(&helix, &account, &channel, &session_id).await;
                (channel, made)
            });
        }
        while let Some(done) = asking.join_next().await {
            let Ok((channel, made)) = done else {
                continue;
            };
            match made {
                Ok(ids) => {
                    // Receiving again: the next failure is news.
                    self.said_broken.remove(&channel);
                    if let Some(joined) = self.channels.get_mut(&channel) {
                        joined.subscriptions = ids;
                    }
                }
                Err(e) => {
                    // The reconnect loop tries every few seconds; saying
                    // the same thing each time buries the chat under it.
                    let again = self
                        .said_broken
                        .get(&channel)
                        .is_some_and(|said| said == &e.message);
                    if !again {
                        self.said_broken.insert(channel.clone(), e.message.clone());
                        // An error of ours already has a code the frontends
                        // can say in the reader's language; wrapping it in
                        // another one would strand it in English.
                        if e.code.is_empty() {
                            self.notice(
                                &channel,
                                NoticeLevel::Error,
                                "channel.broken",
                                &[("error", &e.message)],
                                format!("Cannot receive this channel: {}", e.message),
                            )
                            .await;
                        } else {
                            let args: Vec<(&str, &str)> = e
                                .args
                                .iter()
                                .map(|(name, value)| (name.as_str(), value.as_str()))
                                .collect();
                            self.notice(
                                &channel,
                                NoticeLevel::Error,
                                &e.code,
                                &args,
                                e.message.clone(),
                            )
                            .await;
                        }
                    }
                }
            }
        }
    }

    /// Writes one of the channel's own events into its chat. `say` turns
    /// the payload into a code and its values; the frontend does the
    /// wording. An event for a channel we are not in is dropped, as a
    /// stale subscription is Twitch's to forget, not ours to show.
    async fn channel_event(
        &self,
        event: serde_json::Value,
        level: NoticeLevel,
        say: impl FnOnce(&mapping::ChannelEvent) -> (&'static str, Vec<(&'static str, String)>),
    ) {
        let Ok(payload) = serde_json::from_value::<mapping::ChannelEvent>(event) else {
            return;
        };
        let channel = payload.channel();
        if !self.channels.contains_key(&channel) {
            return;
        }
        let (code, args) = say(&payload);
        let args: Vec<(&str, &str)> = args
            .iter()
            .map(|(name, value)| (*name, value.as_str()))
            .collect();
        // The English here is what a frontend shows when it does not know
        // the code; the codes themselves are translated in `liveside_client`.
        let text = crate::twitch::english_event(code, &args);
        self.notice(&channel, level, code, &args, text).await;
    }

    async fn on_notification(&mut self, kind: &str, event: serde_json::Value) {
        let own_id = self.account.as_ref().map(|a| a.user.id.to_string());
        let now = Timestamp::now();
        match kind {
            "channel.chat.message" => {
                let Ok(payload) = serde_json::from_value::<mapping::ChatMessageEvent>(event) else {
                    return;
                };
                let channel = payload.channel();
                let Some(joined) = self.channels.get(&channel) else {
                    return;
                };
                let mut line = payload.to_line(now, own_id.as_deref());
                // Twitch knows nothing about 7TV, BTTV or FFZ, so their
                // emotes arrive as plain text and are recognized here.
                if let LineContent::Message(message) = &mut line.content {
                    // A badge arrives as two ids; the pictures were looked
                    // up when joining, and a window wants the picture.
                    for badge in &mut message.badges {
                        let key = (badge.set_id.clone(), badge.id.clone());
                        if let Some((image, title)) = joined
                            .badges
                            .get(&key)
                            .or_else(|| self.global_badges.get(&key))
                        {
                            badge.image = image.clone();
                            badge.title = title.clone();
                        }
                    }
                    let fragments = std::mem::take(&mut message.fragments);
                    message.fragments = liveside_twitch::emotes::apply(
                        fragments,
                        &[&joined.emotes, &self.global_emotes],
                    );
                }
                self.emit(Event::Line { channel, line }).await;
            }
            "channel.chat.notification" => {
                let Ok(payload) = serde_json::from_value::<mapping::NotificationEvent>(event)
                else {
                    return;
                };
                let channel = UserId(payload.broadcaster_user_id.clone());
                if !self.channels.contains_key(&channel) {
                    return;
                }
                self.emit(Event::Line {
                    channel,
                    line: payload.to_line(now),
                })
                .await;
            }
            // What happens in a channel besides chat. Each one becomes a
            // notice with a code, so both frontends say it in the reader's
            // own language rather than repeating Twitch's English.
            "channel.follow" => {
                self.channel_event(event, NoticeLevel::Info, |e| {
                    ("event.follow", vec![("user", e.user_name.clone())])
                })
                .await;
            }
            "stream.online" | "stream.offline" => {
                let live = kind.ends_with("online");
                if let Ok(payload) = serde_json::from_value::<mapping::ChannelEvent>(event.clone())
                    && self.channels.contains_key(&payload.channel())
                {
                    self.emit(Event::Stream {
                        channel: payload.channel(),
                        live,
                    })
                    .await;
                }
                self.channel_event(event, NoticeLevel::Info, move |_| {
                    let code = if live {
                        "event.stream_online"
                    } else {
                        "event.stream_offline"
                    };
                    (code, Vec::new())
                })
                .await;
            }
            "channel.update" => {
                self.channel_event(event, NoticeLevel::Info, |e| {
                    (
                        "event.channel_update",
                        vec![
                            ("title", e.title.clone()),
                            ("game", e.category_name.clone()),
                        ],
                    )
                })
                .await;
            }
            "channel.shield_mode.begin" | "channel.shield_mode.end" => {
                let on = kind.ends_with("begin");
                // Kept as well as said, so a menu can be a toggle that
                // knows rather than two items that guess.
                if let Ok(payload) = serde_json::from_value::<mapping::ChannelEvent>(event.clone())
                {
                    self.emit(Event::ShieldMode {
                        channel: payload.channel(),
                        on,
                    })
                    .await;
                }
                self.channel_event(event, NoticeLevel::Warning, move |e| {
                    let code = if on {
                        "event.shield_on"
                    } else {
                        "event.shield_off"
                    };
                    (code, vec![("user", e.moderator_user_name.clone())])
                })
                .await;
            }
            "channel.shoutout.create" => {
                self.channel_event(event, NoticeLevel::Info, |e| {
                    (
                        "event.shoutout_sent",
                        vec![("user", e.to_broadcaster_user_name.clone())],
                    )
                })
                .await;
            }
            "channel.shoutout.receive" => {
                self.channel_event(event, NoticeLevel::Info, |e| {
                    (
                        "event.shoutout_received",
                        vec![
                            ("user", e.from_broadcaster_user_name.clone()),
                            ("viewers", e.viewer_count.unwrap_or(0).to_string()),
                        ],
                    )
                })
                .await;
            }
            "channel.poll.begin" | "channel.poll.progress" | "channel.poll.end" => {
                let Ok(payload) = serde_json::from_value::<mapping::PollEvent>(event) else {
                    return;
                };
                let channel = UserId(payload.broadcaster_user_id.clone());
                if !self.channels.contains_key(&channel) {
                    return;
                }
                let finished = kind == "channel.poll.end";
                let poll = payload.poll(finished);
                match finished {
                    true => self.polls.remove(&channel),
                    false => self.polls.insert(channel.clone(), poll.id.clone()),
                };
                self.emit(Event::PollUpdate { channel, poll }).await;
            }
            "channel.prediction.begin"
            | "channel.prediction.progress"
            | "channel.prediction.lock"
            | "channel.prediction.end" => {
                let Ok(payload) = serde_json::from_value::<mapping::PredictionEvent>(event) else {
                    return;
                };
                let channel = UserId(payload.broadcaster_user_id.clone());
                if !self.channels.contains_key(&channel) {
                    return;
                }
                let finished = kind == "channel.prediction.end";
                let locked = kind == "channel.prediction.lock";
                let prediction = payload.prediction(locked, finished);
                match finished {
                    true => self.predictions.remove(&channel),
                    false => self
                        .predictions
                        .insert(channel.clone(), prediction.id.clone()),
                };
                self.emit(Event::PredictionUpdate {
                    channel,
                    prediction,
                })
                .await;
            }
            "channel.channel_points_custom_reward_redemption.add" => {
                let Ok(payload) = serde_json::from_value::<mapping::RedemptionEvent>(event) else {
                    return;
                };
                let channel = UserId(payload.broadcaster_user_id.clone());
                if !self.channels.contains_key(&channel) {
                    return;
                }
                // What they typed belongs in the line: a reward that asks
                // a question is answered in the redemption, not in chat.
                let (code, args): (&str, Vec<(&str, String)>) = match payload.user_input.is_empty()
                {
                    true => (
                        "event.redemption",
                        vec![
                            ("user", payload.user_name.clone()),
                            ("reward", payload.reward.title.clone()),
                            ("cost", payload.reward.cost.to_string()),
                        ],
                    ),
                    false => (
                        "event.redemption_text",
                        vec![
                            ("user", payload.user_name.clone()),
                            ("reward", payload.reward.title.clone()),
                            ("cost", payload.reward.cost.to_string()),
                            ("text", payload.user_input.clone()),
                        ],
                    ),
                };
                let args: Vec<(&str, &str)> = args.iter().map(|(k, v)| (*k, v.as_str())).collect();
                let text = english_event(code, &args);
                self.notice(&channel, NoticeLevel::Info, code, &args, text)
                    .await;
                // Into the queue as well, but only for a reward this
                // client created: Twitch lets nobody else answer one, so
                // a row for it would carry a button that cannot work.
                if self
                    .manageable_rewards
                    .get(&channel)
                    .is_some_and(|ours| ours.contains(&payload.reward.id))
                {
                    self.emit(Event::RedemptionUpdate {
                        channel,
                        redemption: redemption_of(&payload),
                    })
                    .await;
                }
            }
            "channel.channel_points_custom_reward_redemption.update" => {
                let Ok(payload) = serde_json::from_value::<mapping::RedemptionEvent>(event) else {
                    return;
                };
                let channel = UserId(payload.broadcaster_user_id.clone());
                if !self.channels.contains_key(&channel) {
                    return;
                }
                // Answered, here or in the dashboard: the queue drops it.
                self.emit(Event::RedemptionUpdate {
                    channel,
                    redemption: redemption_of(&payload),
                })
                .await;
            }
            // The money, all said the same way: a line in the channel.
            "channel.subscribe"
            | "channel.subscription.message"
            | "channel.subscription.gift"
            | "channel.cheer"
            | "channel.hype_train.begin"
            | "channel.hype_train.end"
            | "channel.goal.begin"
            | "channel.goal.end"
            | "channel.charity_campaign.donate"
            | "channel.charity_campaign.start"
            | "channel.charity_campaign.stop" => {
                let Ok(payload) = serde_json::from_value::<mapping::MoneyEvent>(event) else {
                    return;
                };
                let channel = payload.channel();
                if !self.channels.contains_key(&channel) {
                    return;
                }
                // Somebody who gave anonymously is not named, because
                // Twitch does not say who they were.
                let who = match payload.is_anonymous || payload.user_name.is_empty() {
                    true => "anonym".to_owned(),
                    false => payload.user_name.clone(),
                };
                let said = payload.said();
                let (code, args): (&str, Vec<(&str, String)>) =
                    match kind {
                        // A gift subscription arrives twice: once for the
                        // gift and once per person who got one. The second
                        // kind says so, and naming the receiver there would
                        // be a line per gift in a wave of a hundred.
                        "channel.subscribe" if payload.is_gift => return,
                        "channel.subscribe" => (
                            "money.subscribe",
                            vec![("user", who), ("tier", payload.tier_number())],
                        ),
                        "channel.subscription.message" => (
                            "money.resubscribe",
                            vec![
                                ("user", who),
                                ("tier", payload.tier_number()),
                                ("months", payload.cumulative_months.to_string()),
                                ("text", said),
                            ],
                        ),
                        "channel.subscription.gift" => (
                            "money.gift",
                            vec![
                                ("user", who),
                                ("tier", payload.tier_number()),
                                ("count", payload.total.to_string()),
                            ],
                        ),
                        "channel.cheer" => (
                            "money.cheer",
                            vec![
                                ("user", who),
                                ("bits", payload.bits.to_string()),
                                ("text", said),
                            ],
                        ),
                        "channel.hype_train.begin" => (
                            "money.hype_begin",
                            vec![("level", payload.level.to_string())],
                        ),
                        "channel.hype_train.end" => {
                            ("money.hype_end", vec![("level", payload.level.to_string())])
                        }
                        "channel.goal.begin" => (
                            "money.goal_begin",
                            vec![
                                ("goal", payload.description.clone()),
                                (
                                    "target",
                                    mapping::MoneyEvent::money(payload.target_amount.as_ref()),
                                ),
                            ],
                        ),
                        "channel.goal.end" => (
                            match payload.is_achieved {
                                true => "money.goal_reached",
                                false => "money.goal_ended",
                            },
                            vec![
                                ("goal", payload.description.clone()),
                                (
                                    "amount",
                                    mapping::MoneyEvent::money(payload.current_amount.as_ref()),
                                ),
                            ],
                        ),
                        _ => (
                            "money.charity",
                            vec![
                            ("user", who),
                            ("charity", payload.charity_name.clone()),
                            (
                                "amount",
                                mapping::MoneyEvent::money(
                                    payload.amount.as_ref().map(|_| {
                                        // Rebuilt as a value so one helper
                                        // reads both shapes.
                                        serde_json::json!({
                                            "value": payload.amount.as_ref().map(|a| a.value),
                                            "decimal_places": payload
                                                .amount
                                                .as_ref()
                                                .map(|a| a.decimal_places),
                                            "currency": payload
                                                .amount
                                                .as_ref()
                                                .map(|a| a.currency.clone()),
                                        })
                                    })
                                    .as_ref(),
                                ),
                            ),
                        ],
                        ),
                    };
                let args: Vec<(&str, &str)> = args.iter().map(|(k, v)| (*k, v.as_str())).collect();
                let text = english_event(code, &args);
                self.notice(&channel, NoticeLevel::Info, code, &args, text)
                    .await;
            }
            "channel.suspicious_user.message" => {
                let Ok(payload) = serde_json::from_value::<mapping::SuspiciousEvent>(event) else {
                    return;
                };
                let channel = payload.channel();
                if !self.channels.contains_key(&channel) {
                    return;
                }
                let Some(message) = payload.message.as_ref().map(|m| m.message_id.clone()) else {
                    return;
                };
                self.emit(Event::SuspiciousMessage {
                    channel,
                    message: message.into(),
                    user: liveside_proto::model::User {
                        id: payload.user_id.clone().into(),
                        login: payload.user_login.clone().into(),
                        display_name: payload.user_name.clone(),
                    },
                    suspicion: Box::new(payload.suspicion()),
                })
                .await;
            }
            "channel.suspicious_user.update" => {
                let Ok(payload) = serde_json::from_value::<mapping::SuspiciousEvent>(event) else {
                    return;
                };
                let channel = payload.channel();
                if !self.channels.contains_key(&channel) {
                    return;
                }
                use liveside_proto::model::SuspicionStatus;
                let (code, level) = match payload.suspicion().status {
                    SuspicionStatus::Monitored => ("mod.suspicion_monitored", NoticeLevel::Warning),
                    SuspicionStatus::Restricted => {
                        ("mod.suspicion_restricted", NoticeLevel::Warning)
                    }
                    // Twitch still thinks them suspicious; it is the
                    // moderators who have decided not to act.
                    SuspicionStatus::None => ("mod.suspicion_cleared", NoticeLevel::Info),
                };
                let args = [
                    ("moderator", payload.moderator_user_name.as_str()),
                    ("user", payload.user_name.as_str()),
                ];
                let text = english_event(code, &args);
                self.notice(&channel, level, code, &args, text).await;
            }
            "user.whisper.message" => {
                let Ok(payload) = serde_json::from_value::<mapping::WhisperEvent>(event) else {
                    return;
                };
                let from = liveside_proto::model::User {
                    id: payload.from_user_id.clone().into(),
                    login: payload.from_user_login.clone().into(),
                    display_name: payload.from_user_name.clone(),
                };
                let line = Line {
                    time: Timestamp::now(),
                    content: LineContent::Message(liveside_proto::model::ChatMessage {
                        id: payload.whisper_id.clone().into(),
                        sender: from.clone(),
                        color: None,
                        badges: Vec::new(),
                        fragments: vec![liveside_proto::model::Fragment::Text {
                            text: payload.whisper.map(|w| w.text).unwrap_or_default(),
                        }],
                        kind: liveside_proto::model::MessageKind::Text,
                        bits: None,
                        reply: None,
                        shared_source: None,
                    }),
                    flags: liveside_proto::model::LineFlags::default(),
                };
                self.emit(Event::Whisper { user: from, line }).await;
            }
            "channel.shared_chat.begin"
            | "channel.shared_chat.update"
            | "channel.shared_chat.end" => {
                let Ok(payload) = serde_json::from_value::<mapping::SharedChatEvent>(event) else {
                    return;
                };
                let channel = payload.channel();
                if !self.channels.contains_key(&channel) {
                    return;
                }
                let me = self.channel_name(&channel);
                let (code, args) = match kind {
                    "channel.shared_chat.begin" => {
                        ("shared.begin", vec![("channels", payload.others(&me))])
                    }
                    _ => ("shared.end", Vec::new()),
                };
                let args: Vec<(&str, &str)> = args.iter().map(|(k, v)| (*k, v.as_str())).collect();
                let text = english_event(code, &args);
                self.notice(&channel, NoticeLevel::Info, code, &args, text)
                    .await;
            }
            "channel.ad_break.begin" => {
                self.channel_event(event, NoticeLevel::Warning, |e| {
                    ("event.ad_break", vec![("seconds", e.seconds().to_string())])
                })
                .await;
            }
            "automod.message.hold" => {
                let Ok(payload) = serde_json::from_value::<mapping::HoldEvent>(event) else {
                    return;
                };
                let channel = UserId(payload.broadcaster_user_id.clone());
                if !self.channels.contains_key(&channel) {
                    return;
                }
                self.emit(Event::MessageHeld {
                    channel,
                    held: payload.to_held(now),
                })
                .await;
            }
            "automod.message.update" => {
                let Ok(payload) = serde_json::from_value::<mapping::HoldEvent>(event) else {
                    return;
                };
                let channel = UserId(payload.broadcaster_user_id.clone());
                if !self.channels.contains_key(&channel) {
                    return;
                }
                self.emit(Event::HeldResolved {
                    channel,
                    message: payload.message_id.clone().into(),
                    status: payload.status.clone(),
                    moderator: payload.moderator_user_name.clone(),
                })
                .await;
            }
            // Our own message, caught on its way into the chat. Nobody
            // else sees these, and without them a message simply vanishes.
            // The moderation log: what every moderator does here.
            "channel.moderate" => {
                let Ok(payload) = serde_json::from_value::<mapping::ModerateEvent>(event) else {
                    return;
                };
                let channel = payload.channel();
                if !self.channels.contains_key(&channel) {
                    return;
                }
                let (code, args) = payload.to_notice();
                let args: Vec<(&str, &str)> = args
                    .iter()
                    .map(|(name, value)| (*name, value.as_str()))
                    .collect();
                let text = english_event(code, &args);
                self.notice(&channel, NoticeLevel::Info, code, &args, text)
                    .await;
            }
            "channel.unban_request.create" => {
                let Ok(payload) = serde_json::from_value::<mapping::UnbanRequestEvent>(event)
                else {
                    return;
                };
                let channel = payload.channel();
                if !self.channels.contains_key(&channel) {
                    return;
                }
                self.emit(Event::UnbanRequested {
                    channel,
                    request: payload.to_request(now),
                })
                .await;
            }
            "channel.unban_request.resolve" => {
                let Ok(payload) = serde_json::from_value::<mapping::UnbanRequestEvent>(event)
                else {
                    return;
                };
                let channel = payload.channel();
                if !self.channels.contains_key(&channel) {
                    return;
                }
                self.emit(Event::UnbanResolved {
                    channel,
                    request: payload.id.clone(),
                    status: payload.status.clone(),
                    moderator: payload.moderator_user_name.clone(),
                })
                .await;
            }
            "channel.chat.user_message_hold" => {
                self.channel_event(event, NoticeLevel::Warning, |_| {
                    ("event.own_held", Vec::new())
                })
                .await;
            }
            "channel.chat.user_message_update" => {
                let Ok(payload) = serde_json::from_value::<mapping::HoldEvent>(event.clone())
                else {
                    return;
                };
                let code = match payload.status.as_str() {
                    "approved" => "event.own_allowed",
                    "denied" => "event.own_denied",
                    _ => "event.own_dropped",
                };
                self.channel_event(event, NoticeLevel::Info, move |_| (code, Vec::new()))
                    .await;
            }
            "channel.chat.message_delete" => {
                let Ok(payload) = serde_json::from_value::<mapping::MessageDeleteEvent>(event)
                else {
                    return;
                };
                self.emit(Event::MessageDeleted {
                    channel: UserId(payload.broadcaster_user_id),
                    message: payload.message_id.into(),
                })
                .await;
            }
            "channel.chat.clear_user_messages" => {
                let Ok(payload) = serde_json::from_value::<mapping::ClearUserMessagesEvent>(event)
                else {
                    return;
                };
                // Twitch does not say whether it was a timeout or a ban.
                self.emit(Event::UserCleared {
                    channel: UserId(payload.broadcaster_user_id),
                    user: UserId(payload.target_user_id),
                    seconds: None,
                })
                .await;
            }
            "channel.chat_settings.update" => {
                let Ok(payload) = serde_json::from_value::<mapping::ChatSettingsEvent>(event)
                else {
                    return;
                };
                let channel = UserId(payload.broadcaster_user_id.clone());
                if !self.channels.contains_key(&channel) {
                    return;
                }
                self.emit(Event::RoomModesChanged {
                    channel,
                    modes: payload.to_modes(),
                })
                .await;
            }
            "channel.chat.clear" => {
                let Ok(payload) = serde_json::from_value::<mapping::ChatClearEvent>(event) else {
                    return;
                };
                self.emit(Event::ChatCleared {
                    channel: UserId(payload.broadcaster_user_id),
                })
                .await;
            }
            _ => {}
        }
    }
}

/// An error this engine wrote, which a frontend can say in its own words.
/// The time zone Twitch is told a broadcast happens in. It wants an IANA
/// name; the machine's own is the only honest answer, and UTC is the
/// fallback when it will not say.
fn local_timezone() -> String {
    std::fs::read_link("/etc/localtime")
        .ok()
        .and_then(|path| {
            let path = path.to_string_lossy().to_string();
            path.split_once("/zoneinfo/")
                .map(|(_, zone)| zone.to_owned())
        })
        .unwrap_or_else(|| "UTC".to_owned())
}

/// A redemption as EventSub carries it.
fn redemption_of(payload: &mapping::RedemptionEvent) -> liveside_proto::model::Redemption {
    use liveside_proto::model::{Redemption, RedemptionStatus, User};
    Redemption {
        id: payload.id.clone(),
        reward_id: payload.reward.id.clone(),
        reward_title: payload.reward.title.clone(),
        cost: payload.reward.cost,
        user: User {
            id: payload.user_id.clone().into(),
            login: payload.user_login.clone().into(),
            display_name: payload.user_name.clone(),
        },
        text: payload.user_input.clone(),
        status: match payload.status.as_str() {
            "fulfilled" => RedemptionStatus::Fulfilled,
            "canceled" => RedemptionStatus::Canceled,
            _ => RedemptionStatus::Unfulfilled,
        },
        time: Timestamp::now(),
    }
}

/// A redemption as Helix answers it, in the protocol's own terms.
fn from_helix_redemption(
    redemption: liveside_twitch::helix::HelixRedemption,
) -> liveside_proto::model::Redemption {
    use liveside_proto::model::{Redemption, RedemptionStatus, User};
    liveside_proto::model::Redemption {
        id: redemption.id,
        reward_id: redemption.reward.id,
        reward_title: redemption.reward.title,
        cost: redemption.reward.cost,
        user: User {
            id: redemption.user_id.into(),
            login: redemption.user_login.into(),
            display_name: redemption.user_name,
        },
        text: redemption.user_input,
        status: match redemption.status.as_str() {
            "FULFILLED" => RedemptionStatus::Fulfilled,
            "CANCELED" => RedemptionStatus::Canceled,
            _ => RedemptionStatus::Unfulfilled,
        },
        time: Timestamp::now(),
    } as Redemption
}

/// A prediction as Helix answers it, in the protocol's own terms.
fn from_helix_prediction(
    prediction: liveside_twitch::helix::HelixPrediction,
) -> liveside_proto::model::Prediction {
    use liveside_proto::model::{Prediction, PredictionOutcome, PredictionStatus};
    Prediction {
        id: prediction.id,
        title: prediction.title,
        outcomes: prediction
            .outcomes
            .into_iter()
            .map(|outcome| PredictionOutcome {
                id: outcome.id,
                title: outcome.title,
                users: outcome.users,
                points: outcome.channel_points,
            })
            .collect(),
        status: match prediction.status.as_str() {
            "ACTIVE" => PredictionStatus::Active,
            "LOCKED" => PredictionStatus::Locked,
            "CANCELED" => PredictionStatus::Canceled,
            _ => PredictionStatus::Resolved,
        },
        locks_at: prediction.locked_at,
        winner: prediction.winning_outcome_id,
    }
}

/// When to renew a token that lives `expires_in` seconds: a quarter of
/// an hour before it runs out.
///
/// There used to be a ceiling of an hour here as well, which with
/// Twitch's four-hour token meant renewing four times as often as
/// needed — and a renewal rebuilt the EventSub socket, so the client
/// spent an hourly round of making forty-odd subscriptions per channel
/// afresh. That was what filled the subscription budget and took chat
/// down after a few hours.
fn renew_at(expires_in: u64) -> Instant {
    Instant::now() + Duration::from_secs(renew_in(expires_in))
}

/// The seconds of that, where it can be tested without waiting.
fn renew_in(expires_in: u64) -> u64 {
    let margin = 15 * 60;
    expires_in.saturating_sub(margin).max(60)
}

/// A poll as Helix answers it, in the protocol's own terms.
fn from_helix_poll(poll: liveside_twitch::helix::HelixPoll) -> liveside_proto::model::Poll {
    use liveside_proto::model::{Poll, PollChoice, PollStatus};
    Poll {
        id: poll.id,
        title: poll.title,
        choices: poll
            .choices
            .into_iter()
            .map(|choice| PollChoice {
                id: choice.id,
                title: choice.title,
                votes: choice.votes,
            })
            .collect(),
        status: match poll.status.as_str() {
            "ACTIVE" => PollStatus::Active,
            "ARCHIVED" => PollStatus::Archived,
            "TERMINATED" => PollStatus::Terminated,
            _ => PollStatus::Completed,
        },
        ends_at: poll.ends_at,
    }
}

fn error(
    kind: ErrorKind,
    code: &str,
    args: &[(&str, &str)],
    message: impl Into<String>,
) -> CommandError {
    CommandError::new(kind, code, args, message)
}

/// Named arguments, as the protocol carries them.
fn owned(args: &[(&str, &str)]) -> Vec<(String, String)> {
    args.iter()
        .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
        .collect()
}

/// The body for Update Chat Settings: only the field being changed, so
/// nothing else is disturbed.
fn room_mode_patch(mode: &RoomModeChange) -> serde_json::Value {
    use serde_json::json;
    match mode {
        RoomModeChange::Slow(Some(seconds)) => {
            json!({ "slow_mode": true, "slow_mode_wait_time": seconds })
        }
        RoomModeChange::Slow(None) => json!({ "slow_mode": false }),
        RoomModeChange::Followers(Some(minutes)) => {
            json!({ "follower_mode": true, "follower_mode_duration": minutes })
        }
        RoomModeChange::Followers(None) => json!({ "follower_mode": false }),
        RoomModeChange::Subscribers(on) => json!({ "subscriber_mode": on }),
        RoomModeChange::NonModeratorDelay(Some(seconds)) => {
            json!({ "non_moderator_chat_delay": true, "non_moderator_chat_delay_duration": seconds })
        }
        RoomModeChange::NonModeratorDelay(None) => json!({ "non_moderator_chat_delay": false }),
        RoomModeChange::EmoteOnly(on) => json!({ "emote_mode": on }),
        RoomModeChange::UniqueChat(on) => json!({ "unique_chat_mode": on }),
    }
}

/// A channel's badge pictures, or the global ones, keyed by set and
/// version. A failure means no pictures, which frontends already handle:
/// they fall back to the letters the terminal uses.
async fn badges_of(
    helix: &Helix,
    broadcaster_id: Option<&str>,
) -> HashMap<(String, String), (String, String)> {
    let mut out = HashMap::new();
    let Ok(sets) = helix.badges(broadcaster_id).await else {
        return out;
    };
    for set in sets {
        for version in set.versions {
            out.insert(
                (set.set_id.clone(), version.id),
                (version.image_url_2x, version.title),
            );
        }
    }
    out
}

/// What a channel event says when the frontend has no wording for it.
/// The engine sends this alongside the code, as it does for everything
/// else it reports.
fn english_event(code: &str, args: &[(&str, &str)]) -> String {
    let value = |name: &str| {
        args.iter()
            .find(|(key, _)| *key == name)
            .map(|(_, value)| *value)
            .unwrap_or("")
    };
    match code {
        "event.follow" => format!("{} is now following", value("user")),
        "event.stream_online" => "The stream has started".to_owned(),
        "event.stream_offline" => "The stream has ended".to_owned(),
        "event.channel_update" => {
            format!("Now: {} — {}", value("title"), value("game"))
        }
        "event.shield_on" => format!("{} turned Shield Mode on", value("user")),
        "event.shield_off" => format!("{} turned Shield Mode off", value("user")),
        "event.shoutout_sent" => format!("Shouted out {}", value("user")),
        "event.shoutout_received" => format!(
            "{} shouted out this channel to {} viewers",
            value("user"),
            value("viewers")
        ),
        "event.ad_break" => format!("An advertisement is running for {}s", value("seconds")),
        "mod.suspicion_monitored" => {
            format!("{} put {} under watch", value("moderator"), value("user"))
        }
        "mod.suspicion_restricted" => format!(
            "{} restricted {}: their messages are held",
            value("moderator"),
            value("user")
        ),
        "mod.suspicion_cleared" => {
            format!("{} took {} off watch", value("moderator"), value("user"))
        }
        "shared.begin" => format!("Chat is now shared with {}", value("channels")),
        "shared.update" => format!("Chat is now shared with {}", value("channels")),
        "money.charity_start" => format!(
            "A charity campaign has started for {} ({})",
            value("charity"),
            value("target")
        ),
        "money.charity_stop" => format!(
            "The charity campaign for {} is over: {}",
            value("charity"),
            value("amount")
        ),
        "shared.end" => "Chat is no longer shared".to_owned(),
        "ads.none" => "No advertisement break is scheduled".to_owned(),
        "ads.next" => format!(
            "Next advertisement break at {}, {}s, snoozes left: {}",
            value("at"),
            value("seconds"),
            value("snoozes")
        ),
        "money.subscribe" => format!("{} subscribed (tier {})", value("user"), value("tier")),
        "money.resubscribe" => format!(
            "{} resubscribed, {} months (tier {}){}",
            value("user"),
            value("months"),
            value("tier"),
            match value("text").is_empty() {
                true => String::new(),
                false => format!(": {}", value("text")),
            }
        ),
        "money.gift" => format!(
            "{} gave {} subscriptions (tier {})",
            value("user"),
            value("count"),
            value("tier")
        ),
        "money.cheer" => format!(
            "{} cheered {} bits{}",
            value("user"),
            value("bits"),
            match value("text").is_empty() {
                true => String::new(),
                false => format!(": {}", value("text")),
            }
        ),
        "money.hype_begin" => format!("A Hype Train has started (level {})", value("level")),
        "money.hype_running" => format!("A Hype Train is running (level {})", value("level")),
        "shared.running" => format!("Chat is shared with {} other channels", value("count")),
        "money.hype_end" => format!("The Hype Train is over at level {}", value("level")),
        "money.goal_begin" => format!("New goal: {} ({})", value("goal"), value("target")),
        "money.goal_reached" => format!("Goal reached: {} ({})", value("goal"), value("amount")),
        "money.goal_ended" => format!("Goal ended: {} ({})", value("goal"), value("amount")),
        "money.charity" => format!(
            "{} gave {} to {}",
            value("user"),
            value("amount"),
            value("charity")
        ),
        "event.redemption" => format!(
            "{} redeemed {} ({} points)",
            value("user"),
            value("reward"),
            value("cost")
        ),
        "event.redemption_text" => format!(
            "{} redeemed {} ({} points): {}",
            value("user"),
            value("reward"),
            value("cost"),
            value("text")
        ),
        "mod.ban" => format!(
            "{} banned {}{}",
            value("moderator"),
            value("user"),
            value("detail")
        ),
        "mod.unban" => format!("{} unbanned {}", value("moderator"), value("user")),
        "mod.timeout" => format!(
            "{} timed {} out{}",
            value("moderator"),
            value("user"),
            value("detail")
        ),
        "mod.untimeout" => format!(
            "{} lifted the timeout on {}",
            value("moderator"),
            value("user")
        ),
        "mod.delete" => format!(
            "{} deleted a message from {}: {}",
            value("moderator"),
            value("user"),
            value("text")
        ),
        "mod.warn" => format!(
            "{} warned {}{}",
            value("moderator"),
            value("user"),
            value("detail")
        ),
        "mod.mod" => format!("{} made {} a moderator", value("moderator"), value("user")),
        "mod.unmod" => format!(
            "{} took moderator from {}",
            value("moderator"),
            value("user")
        ),
        "mod.vip" => format!("{} made {} a VIP", value("moderator"), value("user")),
        "mod.unvip" => format!("{} took VIP from {}", value("moderator"), value("user")),
        "mod.raid" => format!("{} started a raid on {}", value("moderator"), value("user")),
        "mod.emoteonly" => format!("{} turned emote-only chat on", value("moderator")),
        "mod.emoteonlyoff" => format!("{} turned emote-only chat off", value("moderator")),
        "mod.subscribers" => format!("{} turned subscriber-only chat on", value("moderator")),
        "mod.subscribersoff" => format!("{} turned subscriber-only chat off", value("moderator")),
        "mod.uniquechat" => format!("{} turned unique chat on", value("moderator")),
        "mod.uniquechatoff" => format!("{} turned unique chat off", value("moderator")),
        "mod.followers" => format!(
            "{} turned follower-only chat on{}",
            value("moderator"),
            value("detail")
        ),
        "mod.followersoff" => format!("{} turned follower-only chat off", value("moderator")),
        "mod.slow" => format!(
            "{} turned slow mode on{}",
            value("moderator"),
            value("detail")
        ),
        "mod.slowoff" => format!("{} turned slow mode off", value("moderator")),
        "mod.clear" => format!("{} cleared the chat", value("moderator")),
        "mod.other" => format!("{}: {}", value("moderator"), value("action")),
        "event.own_held" => "Your message is being checked before it appears".to_owned(),
        "event.own_allowed" => "Your message was let through".to_owned(),
        "event.own_denied" => "Your message was refused".to_owned(),
        "event.own_dropped" => "Your message will not appear".to_owned(),
        _ => String::new(),
    }
}

/// Maps a Twitch error to what the user should be told.
/// How many EventSub subscriptions are asked for at once. A channel wants
/// more than forty of them and Twitch answers each in its own round trip,
/// so asking one after another made a join take some ten seconds. A
/// handful in flight keeps it short without leaning on the rate limit.
const SUBSCRIBE_AT_ONCE: usize = 8;

/// Creates subscriptions in waves. Gives back the ids of those that were
/// made and the first failure, if there was one: whether a failure matters
/// is the caller's to decide, since the extras are allowed to be refused.
async fn create_subscriptions(
    helix: &Helix,
    session_id: &str,
    wanted: Vec<(&'static str, &'static str, serde_json::Value)>,
) -> (Vec<String>, Option<CommandError>) {
    let mut ids = Vec::new();
    let mut failed: Option<CommandError> = None;
    for wave in wanted.chunks(SUBSCRIBE_AT_ONCE) {
        let mut set = tokio::task::JoinSet::new();
        for (kind, version, condition) in wave.iter().cloned() {
            let helix = helix.clone();
            let session_id = session_id.to_owned();
            set.spawn(async move { helix.subscribe(kind, version, condition, &session_id).await });
        }
        while let Some(done) = set.join_next().await {
            let outcome = match done {
                Ok(outcome) => outcome.map_err(from_twitch),
                // Only a panic in the task above gets here.
                Err(e) => Err(error(
                    ErrorKind::Network,
                    "twitch.subscribe_failed",
                    &[("error", &e.to_string())],
                    format!("Could not subscribe to the channel's events: {e}"),
                )),
            };
            match outcome {
                Ok(subscription) => ids.push(subscription.id),
                Err(e) => failed = failed.or(Some(e)),
            }
        }
    }
    (ids, failed)
}

/// Everything a join needs from Twitch, gathered before any of it is
/// applied. Nothing here touches the backend's own state, so several
/// channels can be asked about side by side.
struct Prepared {
    channel: Channel,
    subscriptions: Vec<String>,
    settings: liveside_twitch::Result<RoomModes>,
    requests: liveside_twitch::Result<Vec<liveside_twitch::helix::UnbanRequestData>>,
    shared: liveside_twitch::Result<Vec<String>>,
    shield: liveside_twitch::Result<bool>,
    /// The stream, while one runs: whether it does, and how it stands.
    stream: Vec<liveside_twitch::helix::LiveStream>,
    badges: HashMap<(String, String), (String, String)>,
    polls: Option<Vec<liveside_twitch::helix::HelixPoll>>,
    predictions: Option<Vec<liveside_twitch::helix::HelixPrediction>>,
    rewards: Option<Vec<liveside_twitch::helix::HelixReward>>,
    hype: Option<u32>,
}

/// What `join_many` learned about one channel: the commands that asked
/// for it, the channel, and what Twitch said.
type JoinAnswer = (Vec<u64>, Login, Result<Prepared, CommandError>);

/// Asks Twitch everything a join needs. A join is more than a dozen
/// round trips, so they are made as few waves as the answers allow and
/// the whole of it can run for several channels at once.
async fn prepare_join(
    helix: &Helix,
    session_id: &str,
    account: &Account,
    login: &Login,
) -> Result<Prepared, CommandError> {
    let users = helix
        .users_by_login(&[login.as_str()])
        .await
        .map_err(from_twitch)?;
    let user = users.into_iter().next().ok_or_else(|| {
        error(
            ErrorKind::InvalidArgument,
            "channel.unknown",
            &[("channel", login.as_str())],
            format!("No channel #{login}"),
        )
    })?;
    let channel = Channel {
        id: user.id.clone(),
        login: user.login,
        display_name: user.display_name,
    };
    let subscriptions = subscribe(helix, account, &channel.id, session_id).await?;
    // Everything a channel has to be asked about on joining, asked for
    // at once. None of these answers depends on another and Twitch takes
    // a round trip for each, so waiting for them one after another was
    // most of the time a join took.
    let me = account.user.id.to_string();
    let own = account.user.id == channel.id;
    let id = channel.id.clone();
    let (settings, requests, shared, shield, stream, badges, polls, predictions, rewards, hype) = tokio::join!(
        helix.chat_settings(id.as_str()),
        helix.unban_requests(id.as_str(), &me),
        helix.shared_chat(id.as_str()),
        helix.shield_mode(id.as_str(), &me),
        // A failure here is not worth failing a join for; the next
        // Get Streams says it anyway.
        async {
            helix
                .live_among(&[id.to_string()])
                .await
                .unwrap_or_default()
        },
        badges_of(helix, Some(id.as_str())),
        // Twitch answers these only for the channel's own login, so
        // anywhere else they are not even asked.
        async {
            if own {
                helix.polls(id.as_str()).await.ok()
            } else {
                None
            }
        },
        async {
            if own {
                helix.predictions(id.as_str()).await.ok()
            } else {
                None
            }
        },
        async {
            if own {
                helix.custom_rewards(id.as_str(), true).await.ok()
            } else {
                None
            }
        },
        async {
            if own {
                helix.hype_train(id.as_str()).await.ok().flatten()
            } else {
                None
            }
        },
    );
    Ok(Prepared {
        channel,
        subscriptions,
        settings,
        requests,
        shared,
        shield,
        stream,
        badges,
        polls,
        predictions,
        rewards,
        hype,
    })
}

/// Creates this client's subscriptions for one channel. Free of the
/// backend's state, so it can run for several channels at once.
async fn subscribe(
    helix: &Helix,
    account: &Account,
    channel: &UserId,
    session_id: &str,
) -> Result<Vec<String>, CommandError> {
    let condition = serde_json::json!({
        "broadcaster_user_id": channel.as_str(),
        "user_id": account.user.id.as_str(),
    });
    let wanted: Vec<_> = CHANNEL_SUBSCRIPTIONS
        .iter()
        .map(|(kind, version)| (*kind, *version, condition.clone()))
        .collect();
    let (mut ids, failed) = create_subscriptions(helix, session_id, wanted).await;
    if let Some(e) = failed {
        // Roll back, so a half-joined channel cannot linger.
        for id in &ids {
            let _ = helix.unsubscribe(id).await;
        }
        return Err(e);
    }
    // Everything else a channel can report. Twitch refuses these
    // unless the account moderates or owns the channel, which is the
    // ordinary case for a channel one merely reads, so a refusal is
    // passed over in silence rather than kept from the reader as a
    // failed join.
    // Only our own channel can afford the ones that cost: see
    // eventsub::costs_budget.
    let own = account.user.id == *channel;
    let wanted: Vec<_> = EXTRA_SUBSCRIPTIONS
        .iter()
        .filter(|extra| own || !eventsub::costs_budget(extra.kind))
        .map(|extra| {
            let condition = match extra.about {
                eventsub::About::Channel => {
                    serde_json::json!({ "broadcaster_user_id": channel.as_str() })
                }
                eventsub::About::Moderator => serde_json::json!({
                    "broadcaster_user_id": channel.as_str(),
                    "moderator_user_id": account.user.id.as_str(),
                }),
                eventsub::About::Viewer => serde_json::json!({
                    "broadcaster_user_id": channel.as_str(),
                    "user_id": account.user.id.as_str(),
                }),
            };
            (extra.kind, extra.version, condition)
        })
        .collect();
    let (extras, _) = create_subscriptions(helix, session_id, wanted).await;
    ids.extend(extras);
    Ok(ids)
}

/// What reading the account's follows needs.
const FOLLOWS_SCOPE: &str = "user:read:follows";

/// The followed channels and the streams among them, as one list: the
/// live ones first, in Twitch's order (the most watched first), then the
/// rest by name. A stream whose channel the other list missed — the two
/// are separate requests, and someone may follow in between — is kept.
fn followed_list(
    channels: Vec<liveside_twitch::helix::FollowedChannel>,
    streams: Vec<liveside_twitch::helix::FollowedStream>,
) -> Vec<liveside_proto::model::FollowedChannel> {
    use liveside_proto::model::{FollowedChannel, LiveStream, StreamStats};
    let mut list: Vec<FollowedChannel> = Vec::new();
    for stream in streams {
        // Paging through a list that changes as it is read can give a
        // stream twice.
        if list.iter().any(|f| f.channel.id.as_str() == stream.user_id) {
            continue;
        }
        // A time that cannot be read reads as just started, not as fifty
        // years of streaming.
        let started = liveside_proto::Timestamp::parse_utc(&stream.started_at)
            .unwrap_or_else(liveside_proto::Timestamp::now);
        list.push(FollowedChannel {
            channel: Channel {
                id: stream.user_id.into(),
                login: stream.user_login.into(),
                display_name: stream.user_name,
            },
            live: Some(LiveStream {
                title: stream.title,
                category: stream.game_name,
                stats: StreamStats {
                    viewers: stream.viewer_count,
                    started,
                },
            }),
        });
    }
    let live = list.len();
    for channel in channels {
        if list
            .iter()
            .any(|f| f.channel.id.as_str() == channel.broadcaster_id)
        {
            continue;
        }
        list.push(FollowedChannel {
            channel: Channel {
                id: channel.broadcaster_id.into(),
                login: channel.broadcaster_login.into(),
                display_name: channel.broadcaster_name,
            },
            live: None,
        });
    }
    list[live..].sort_by(|a, b| a.channel.login.as_str().cmp(b.channel.login.as_str()));
    list
}

/// The followed channels in `live` that were not streaming `before`, in
/// `live`'s order, leaving out the joined ones: those say it themselves.
fn started_streaming<T>(
    before: &HashSet<UserId>,
    live: &[liveside_proto::model::FollowedChannel],
    joined: &HashMap<UserId, T>,
) -> Vec<UserId> {
    live.iter()
        .map(|f| &f.channel.id)
        .filter(|id| !before.contains(*id) && !joined.contains_key(*id))
        .cloned()
        .collect()
}

/// The refusal for a token that lacks `scope`, in Twitch's own words for
/// it and with the cure.
fn missing_scope(scope: &str) -> CommandError {
    let detail = format!("Missing scope: {scope}");
    error(
        ErrorKind::MissingScope,
        "twitch.missing_scope",
        &[("detail", &detail)],
        format!("{detail}. Log out and log in again to grant the new permission."),
    )
}

fn from_twitch(e: liveside_twitch::Error) -> CommandError {
    use liveside_twitch::Error as E;
    let text = e.to_string();
    // A token from before a feature existed lacks its permission, and the
    // only cure is logging in again; Twitch's own wording does not say so.
    if text.to_lowercase().contains("scope") {
        // Twitch's own wording sometimes ends in a full stop, sometimes not.
        let text = text.trim_end_matches(['.', ' ']);
        return error(
            ErrorKind::MissingScope,
            "twitch.missing_scope",
            &[("detail", text)],
            format!("{text}. Log out and log in again to grant the new permission."),
        );
    }
    // The socket's ten subscription points are spent. Twitch calls this
    // a rate limit, which sends the reader looking for a wait that will
    // never help.
    if text.contains("total cost exceeded") {
        return error(
            ErrorKind::Twitch,
            "twitch.budget_full",
            &[],
            "Twitch allows this client only ten subscriptions about \
             channels that have not authorized it, and they are all \
             taken; join fewer channels."
                .to_owned(),
        );
    }
    let kind = match &e {
        E::Auth(_) => ErrorKind::NotLoggedIn,
        E::Unauthorized(_) => ErrorKind::NotLoggedIn,
        E::Forbidden(_) => ErrorKind::MissingScope,
        E::Api(_) => ErrorKind::Twitch,
        E::Network(_) => ErrorKind::Network,
        E::Storage(_) => ErrorKind::Internal,
    };
    CommandError::passthrough(kind, e.to_string())
}

/// What [`Twitch::use_tokens`] says of a token granted to another
/// Client ID.
const OTHER_CLIENT: &str = "token granted to another client";

fn is_other_client(e: &liveside_twitch::Error) -> bool {
    matches!(e, liveside_twitch::Error::Unauthorized(m) if m == OTHER_CLIENT)
}

#[cfg(test)]
mod event_tests {
    use super::english_event;

    #[test]
    fn every_event_this_client_reports_has_words() {
        // A code with no English here would reach a frontend that does not
        // know it as an empty line.
        for code in [
            "mod.ban",
            "mod.unban",
            "mod.timeout",
            "mod.untimeout",
            "mod.delete",
            "mod.warn",
            "mod.mod",
            "mod.unmod",
            "mod.vip",
            "mod.unvip",
            "mod.raid",
            "mod.emoteonly",
            "mod.emoteonlyoff",
            "mod.subscribers",
            "mod.subscribersoff",
            "mod.uniquechat",
            "mod.uniquechatoff",
            "mod.followers",
            "mod.followersoff",
            "mod.slow",
            "mod.slowoff",
            "mod.clear",
            "mod.other",
            "event.own_held",
            "event.own_allowed",
            "event.own_denied",
            "event.own_dropped",
            "event.follow",
            "event.stream_online",
            "event.stream_offline",
            "event.channel_update",
            "event.shield_on",
            "event.shield_off",
            "event.shoutout_sent",
            "event.shoutout_received",
            "event.ad_break",
        ] {
            let said = english_event(code, &[("user", "Someone"), ("seconds", "60")]);
            assert!(!said.is_empty(), "{code}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::renew_in;

    mod followed {
        use super::super::*;
        use liveside_twitch::helix::{FollowedChannel, FollowedStream};

        fn channel(id: &str, login: &str) -> FollowedChannel {
            FollowedChannel {
                broadcaster_id: id.into(),
                broadcaster_login: login.into(),
                broadcaster_name: login.to_uppercase(),
            }
        }

        fn stream(id: &str, login: &str, viewers: u32) -> FollowedStream {
            FollowedStream {
                user_id: id.into(),
                user_login: login.into(),
                user_name: login.into(),
                game_name: "Art".into(),
                title: "t".into(),
                viewer_count: viewers,
                started_at: "2026-10-16T18:00:00Z".into(),
            }
        }

        fn logins(list: &[liveside_proto::model::FollowedChannel]) -> Vec<(&str, bool)> {
            list.iter()
                .map(|f| (f.channel.login.as_str(), f.live.is_some()))
                .collect()
        }

        #[test]
        fn live_ones_come_first_as_twitch_ordered_them_and_the_rest_by_name() {
            let list = followed_list(
                vec![
                    channel("1", "zeta"),
                    channel("2", "big"),
                    channel("3", "alpha"),
                    channel("4", "small"),
                ],
                vec![stream("2", "big", 900), stream("4", "small", 5)],
            );
            assert_eq!(
                logins(&list),
                [
                    ("big", true),
                    ("small", true),
                    ("alpha", false),
                    ("zeta", false)
                ]
            );
            let live = list[0].live.as_ref().unwrap();
            assert_eq!(live.stats.viewers, 900);
            assert_eq!(live.category, "Art");
            assert_eq!(
                live.stats.started,
                liveside_proto::Timestamp::parse_utc("2026-10-16T18:00:00Z").unwrap()
            );
            // The channel list's names are kept for the offline ones.
            assert_eq!(list[2].channel.display_name, "ALPHA");
        }

        #[test]
        fn a_stream_the_channel_list_missed_or_paged_twice_is_there_once() {
            let list = followed_list(
                vec![channel("1", "old")],
                vec![stream("7", "new", 10), stream("7", "new", 10)],
            );
            assert_eq!(logins(&list), [("new", true), ("old", false)]);
        }

        #[test]
        fn news_is_a_start_since_the_last_look_in_a_channel_not_joined() {
            let live = followed_list(
                Vec::new(),
                vec![
                    stream("1", "was", 10),
                    stream("2", "began", 20),
                    stream("3", "joined", 30),
                ],
            );
            let before: HashSet<UserId> = ["1".into()].into_iter().collect();
            let joined: HashMap<UserId, ()> = [("3".into(), ())].into_iter().collect();
            assert_eq!(
                started_streaming(&before, &live, &joined),
                [UserId::from("2")]
            );
        }

        #[tokio::test]
        async fn me_goes_out_over_irc_once_joined_and_only_with_the_permission() {
            let (events, _seen) = mpsc::channel(16);
            let config = TwitchConfig {
                token_path: std::env::temp_dir().join(format!(
                    "liveside-test-token-me-{}.json",
                    std::process::id()
                )),
                ..TwitchConfig::default()
            };
            assert!(!config.token_path.exists());
            let (mut twitch, _) = Twitch::new(config, reqwest::Client::new(), events);
            let account = |scopes: &[&str]| Account {
                user: liveside_proto::model::User {
                    id: "5".into(),
                    login: "me".into(),
                    display_name: "me".into(),
                },
                scopes: scopes.iter().map(|s| (*s).to_owned()).collect(),
            };
            twitch.account = Some(account(&["user:write:chat"]));
            let error = twitch
                .send_action("1".into(), "waves".into())
                .await
                .unwrap_err();
            assert_eq!(
                (error.kind, error.code.as_str()),
                (ErrorKind::MissingScope, "me.needs_login")
            );

            // A connection standing in for IRC, already logged in.
            twitch.account = Some(account(&["chat:read", "chat:edit"]));
            let (out, mut sent) = mpsc::channel(8);
            twitch.speaker.out = Some(out);
            twitch.speaker.ready = true;
            twitch
                .send_action("1".into(), "waves".into())
                .await
                .unwrap();
            twitch
                .send_action("1".into(), "again".into())
                .await
                .unwrap();
            let mut lines = Vec::new();
            while let Ok(command) = sent.try_recv() {
                lines.push(format!("{command:?}"));
            }
            assert_eq!(
                lines,
                [
                    "Join(\"1\")",
                    "Say { channel: \"1\", text: \"/me waves\" }",
                    "Say { channel: \"1\", text: \"/me again\" }",
                ],
                "joined once, then said"
            );
            // Asked for before it is ready, it waits and goes once it is.
            twitch.speaker.ready = false;
            twitch
                .send_action("1".into(), "later".into())
                .await
                .unwrap();
            assert!(sent.try_recv().is_err());
            twitch.on_speaker(liveside_twitch::irc::Event::Ready).await;
            assert_eq!(
                format!("{:?}", sent.try_recv().unwrap()),
                "Say { channel: \"1\", text: \"/me later\" }"
            );
        }

        #[tokio::test]
        async fn a_token_without_the_permission_is_told_to_log_in_again() {
            let (events, _seen) = mpsc::channel(16);
            let config = TwitchConfig {
                token_path: std::env::temp_dir().join(format!(
                    "liveside-test-token-followed-{}.json",
                    std::process::id()
                )),
                ..TwitchConfig::default()
            };
            assert!(!config.token_path.exists());
            let (mut twitch, _) = Twitch::new(config, reqwest::Client::new(), events);
            twitch.account = Some(Account {
                user: liveside_proto::model::User {
                    id: "5".into(),
                    login: "me".into(),
                    display_name: "me".into(),
                },
                scopes: vec!["user:read:chat".into()],
            });
            let error = twitch.show_followed_channels().await.unwrap_err();
            assert_eq!(error.kind, ErrorKind::MissingScope);
            assert_eq!(error.code, "twitch.missing_scope");
            assert!(
                error.message.contains("user:read:follows"),
                "{}",
                error.message
            );
        }
    }

    mod sockets {
        use super::super::*;

        /// The backend with nothing behind it. Its token file is one of
        /// its own that does not exist: the default is the reader's real
        /// one, and logging out revokes and deletes whatever is there —
        /// which an earlier version of these tests did.
        fn backend() -> (Twitch, mpsc::Receiver<BackendMessage>) {
            let (events, seen) = mpsc::channel(1024);
            static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
            let nth = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let config = TwitchConfig {
                token_path: std::env::temp_dir().join(format!(
                    "liveside-test-token-{}-{nth}.json",
                    std::process::id()
                )),
                ..TwitchConfig::default()
            };
            assert!(!config.token_path.exists());
            let (twitch, _) = Twitch::new(config, reqwest::Client::new(), events);
            (twitch, seen)
        }

        fn socket(number: u64) -> ((u64, tokio::task::JoinHandle<()>), tokio::task::AbortHandle) {
            let task = tokio::spawn(std::future::pending::<()>());
            let abort = task.abort_handle();
            ((number, task), abort)
        }

        fn welcome(session: &str) -> eventsub::Message {
            eventsub::Message::Welcome {
                session_id: session.to_owned(),
                resumed: true,
            }
        }

        async fn settle() {
            for _ in 0..5 {
                tokio::task::yield_now().await;
            }
        }

        #[tokio::test]
        async fn a_welcomed_socket_closes_the_old_one_and_the_old_one_is_not_heard() {
            let (mut twitch, _seen) = backend();
            let (old, old_abort) = socket(1);
            let (new, new_abort) = socket(2);
            twitch.socket = Some(old);
            twitch.socket_next = Some(new);

            twitch.on_socket(2, welcome("new")).await;
            settle().await;
            assert!(old_abort.is_finished(), "the old socket was closed");
            assert!(!new_abort.is_finished());
            assert_eq!(twitch.socket.as_ref().map(|(n, _)| *n), Some(2));
            assert!(twitch.socket_next.is_none());
            assert_eq!(twitch.session_id.as_deref(), Some("new"));

            // Whatever the old one still had queued is not the new one's.
            twitch.on_socket(1, welcome("old")).await;
            assert_eq!(twitch.session_id.as_deref(), Some("new"));
        }

        #[tokio::test]
        async fn opening_again_gives_up_the_socket_still_being_opened() {
            let (mut twitch, _seen) = backend();
            let (opening, opening_abort) = socket(7);
            twitch.socket_count = 7;
            twitch.socket_next = Some(opening);
            // Never reaches Twitch: the url is refused before any network.
            twitch.config.eventsub_url = "ws://127.0.0.1:9/".to_owned();
            twitch.connect_socket();
            settle().await;
            assert!(opening_abort.is_finished());
            assert_eq!(twitch.socket_next.as_ref().map(|(n, _)| *n), Some(8));
            twitch.close_sockets();
        }

        #[tokio::test]
        async fn logging_out_closes_every_socket() {
            let (mut twitch, _seen) = backend();
            let (current, current_abort) = socket(1);
            let (opening, opening_abort) = socket(2);
            twitch.socket = Some(current);
            twitch.socket_next = Some(opening);
            twitch.logout().await;
            settle().await;
            assert!(current_abort.is_finished() && opening_abort.is_finished());
            assert!(twitch.socket.is_none() && twitch.socket_next.is_none());
        }

        #[tokio::test]
        async fn emotes_a_provider_did_not_give_are_asked_for_again() {
            let (mut twitch, _seen) = backend();
            let joined = |login: &str| Joined {
                login: login.into(),
                subscriptions: Vec::new(),
                emotes: EmoteSet::default(),
                badges: HashMap::new(),
                live: false,
            };
            twitch.channels.insert("1".into(), joined("one"));
            twitch.channels.insert("2".into(), joined("two"));
            let loaded = |channel: Option<&str>, failed: bool| EmotesLoaded {
                channel: channel.map(Into::into),
                set: EmoteSet::default(),
                problems: if failed {
                    vec![liveside_twitch::emotes::Problem {
                        provider: "7TV",
                        error: "no network".to_owned(),
                    }]
                } else {
                    Vec::new()
                },
            };
            twitch.emotes_loaded(loaded(None, true)).await;
            twitch.emotes_loaded(loaded(Some("1"), true)).await;
            twitch.emotes_loaded(loaded(Some("2"), false)).await;
            // A channel left while its emotes loaded is not asked about.
            twitch.emotes_loaded(loaded(Some("3"), true)).await;
            // One that failed and then loaded is no longer missing.
            twitch.channels.insert("4".into(), joined("four"));
            twitch.emotes_loaded(loaded(Some("4"), true)).await;
            twitch.emotes_loaded(loaded(Some("4"), false)).await;
            // Nor is one left since.
            twitch.channels.insert("5".into(), joined("five"));
            twitch.emotes_loaded(loaded(Some("5"), true)).await;
            twitch.channels.remove(&UserId::from("5"));

            let mut retry = twitch.emotes_to_retry();
            retry.sort();
            assert_eq!(retry, [None, Some(("1".into(), "one".into()))]);
            // Asked for again, they wait for that answer.
            assert!(twitch.emotes_to_retry().is_empty());
        }
    }

    #[test]
    fn a_token_is_renewed_before_it_runs_out() {
        // Twitch's usual four hours: a quarter of an hour before the end
        // of it, which is once, not the four times an hourly ceiling
        // used to make of it.
        assert_eq!(renew_in(4 * 3600), 4 * 3600 - 900);
        assert_eq!(renew_in(70 * 60), 70 * 60 - 900);
        // A token already inside the margin is renewed at once rather
        // than at some time in the past: this is where a subtraction
        // would have wrapped around to a hundred thousand years.
        assert_eq!(renew_in(60), 60);
        assert_eq!(renew_in(0), 60);
        // And one Twitch says lives for days is left alone until then:
        // renewing early buys nothing and used to cost a reconnect.
        assert_eq!(renew_in(7 * 24 * 3600), 7 * 24 * 3600 - 900);
    }
}
