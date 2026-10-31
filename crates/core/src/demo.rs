//! Demo backend: a fake Twitch with generated chatter.
//!
//! Lets frontends, the daemon and tests run without a connection to Twitch
//! or a Twitch account. Everything is deterministic except timing. Its
//! emotes and badges are real, so a window showing them fetches their
//! images from Twitch's CDN.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use liveside_proto::model::{
    Badge, Channel, ChatMessage, EmoteProvider, EmoteRef, Fragment, Line, LineContent, LineFlags,
    MessageKind, NoticeLevel, Poll, PollChoice, PollStatus, Prediction, PredictionOutcome,
    PredictionStatus, Redemption, RedemptionStatus, Reward, RoomModes, User,
};
use liveside_proto::{
    Account, CommandError, CommandResult, ConnectionState, ErrorKind, Event, Login, Rgb, Timestamp,
    UserId,
};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::backend::{BackendIo, BackendMessage, BackendRequest, RoomModeChange, UserTarget};

#[derive(Debug, Clone)]
pub struct DemoConfig {
    /// Pause between generated messages per channel; `None` generates none
    /// (useful for tests).
    pub chatter_interval: Option<Duration>,
    /// Whether to invent the membership of a channel, as the Twitch
    /// backend reports it from IRC. Off unless the user list is set to
    /// `irc`, so the demo shows what that setting actually changes.
    pub membership: bool,
}

impl Default for DemoConfig {
    fn default() -> Self {
        Self {
            chatter_interval: Some(Duration::from_millis(700)),
            membership: false,
        }
    }
}

/// Returns a function suitable for [`crate::start`] that spawns the demo
/// backend.
pub fn spawn(config: DemoConfig) -> impl FnOnce(BackendIo) {
    move |io| {
        tokio::spawn(run(config, io));
    }
}

/// Rewards the demo channel offers, with what they cost.
const REWARDS: &[(&str, &str)] = &[
    ("Highlight my message", "100"),
    ("Pick the next game", "5000"),
    ("Hydrate!", "250"),
];

const CHATTERS: &[&str] = &[
    "alice",
    "bob_the_viewer",
    "carol",
    "dave_42",
    "eve",
    "Frank",
];

/// People who are in the channel without ever saying anything. Only the
/// `irc` user list knows such people exist; the demo invents a few so the
/// setting can be seen doing something without a Twitch account.
fn lurkers(channel: &Channel) -> Vec<Login> {
    let mut n = hash(channel.login.as_str());
    (0..40)
        .map(|_| {
            n = mix(n);
            Login(format!("lurker_{:03}", n % 1000))
        })
        .collect()
}

const PHRASES: &[&str] = &[
    "hello chat",
    "Kappa",
    "that was close LUL",
    "@{streamer} what game is next?",
    "GG SeemsGood",
    "first time here, nice stream",
    "BigSad noooo",
    "can you explain that again?",
    "4Head 4Head 4Head",
    // Links of every kind the Security settings deal with, so each check
    // has something to show: a page that stays put, a shortener, a plain
    // redirect, an address in a local network, and a look-alike host with
    // a Cyrillic "а" — the phishing case.
    "check https://example.com/ for the schedule",
    "the clip: https://youtu.be/dQw4w9WgXcQ",
    "old link http://github.com/ still works",
    "my router is at http://192.168.1.1/ lol",
    "free subs at https://p\u{430}ypal.com/login",
];

/// Real Twitch global emote IDs, so image URLs work once a GUI renders them.
const EMOTES: &[(&str, &str)] = &[
    ("Kappa", "25"),
    ("4Head", "354"),
    // BigSad took the place of BibleThump (86), which Twitch retired.
    ("BigSad", "emotesv2_db3cd788399347a8b2ebfb8a85f5badb"),
    ("SeemsGood", "64138"),
    ("LUL", "425618"),
];

/// Joined channels and their chatter generator task, if any.
type Joined = Option<JoinHandle<()>>;

struct Demo {
    config: DemoConfig,
    events: mpsc::Sender<BackendMessage>,
    account: Option<Account>,
    /// Whether the one followed channel that comes and goes streams now,
    /// and the task that switches it while somebody is logged in.
    quiet_corner_live: Arc<AtomicBool>,
    /// Whether it is joined: a joined channel starting is said by the
    /// channel itself, not among the followed ones, as on Twitch.
    quiet_corner_joined: Arc<AtomicBool>,
    followed_watch: Option<JoinHandle<()>>,
    joined: HashMap<UserId, Joined>,
    /// What each channel's modes are set to, so that asking for one
    /// changes something a reader can see.
    modes: HashMap<UserId, RoomModes>,
    /// The poll running in a channel, if one is: the task voting for it
    /// every couple of seconds, and the count it has reached. Twitch
    /// answers an ended poll with its votes, so the demo has to know
    /// them too.
    polls: HashMap<UserId, (JoinHandle<()>, Arc<Mutex<Poll>>)>,
    /// The same for a prediction, which differs in that it does not end
    /// on its own: betting closes, and then it waits to be paid out.
    predictions: HashMap<UserId, (JoinHandle<()>, Arc<Mutex<Prediction>>)>,
    /// The rewards each channel offers here, and what is waiting to be
    /// answered: the demo pretends this client made them all, which is
    /// the only case where answering works at all.
    rewards: HashMap<UserId, Vec<Reward>>,
    /// What each channel says about itself here.
    info: HashMap<UserId, liveside_proto::model::ChannelInfo>,
    /// And what it says it will be streaming.
    schedule: HashMap<UserId, liveside_proto::model::Schedule>,
    /// What each channel blocks, so blocking and unblocking show in the
    /// list as they would on Twitch.
    blocked: HashMap<UserId, Vec<liveside_proto::model::BlockedTerm>>,
    /// What has been redeemed and not answered, shared with the task that
    /// generates it: answering one has to say what it was, as Twitch's
    /// own answer does.
    waiting: Arc<Mutex<Vec<Redemption>>>,
    next_message: Arc<AtomicU64>,
}

async fn run(config: DemoConfig, mut io: BackendIo) {
    let mut demo = Demo {
        config,
        events: io.events,
        account: None,
        quiet_corner_live: Arc::new(AtomicBool::new(false)),
        quiet_corner_joined: Arc::new(AtomicBool::new(false)),
        followed_watch: None,
        joined: HashMap::new(),
        modes: HashMap::new(),
        polls: HashMap::new(),
        predictions: HashMap::new(),
        rewards: HashMap::new(),
        info: HashMap::new(),
        schedule: HashMap::new(),
        blocked: HashMap::new(),
        waiting: Arc::new(Mutex::new(Vec::new())),
        next_message: Arc::new(AtomicU64::new(1)),
    };
    demo.emit(Event::Backend {
        name: "demo".to_owned(),
    })
    .await;
    demo.emit(Event::DataFlows {
        flows: vec![liveside_proto::event::DataFlow {
            party: "Nobody".to_owned(),
            party_code: "flow.party.nobody".to_owned(),
            kind: liveside_proto::event::DataFlowKind::Network,
            detail: "The demo backend invents its chat locally; nothing is \
                     sent anywhere and no account is used"
                .to_owned(),
            code: "flow.demo".to_owned(),
            args: Vec::new(),
            enabled: false,
            switch: String::new(),
        }],
    })
    .await;
    demo.emit(Event::Connection {
        state: ConnectionState::Connected,
    })
    .await;

    while let Some(request) = io.requests.recv().await {
        let result = demo.handle(request.request).await;
        let done = BackendMessage::Done {
            token: request.token,
            result,
        };
        if demo.events.send(done).await.is_err() {
            break;
        }
    }
    for task in demo.joined.values().flatten() {
        task.abort();
    }
}

impl Demo {
    /// What a channel blocks, starting with two terms the first time.
    fn blocked_terms(&mut self, channel: &UserId) -> &mut Vec<liveside_proto::model::BlockedTerm> {
        self.blocked.entry(channel.clone()).or_insert_with(|| {
            let term =
                |id: &str, text: &str, expires: Option<&str>| liveside_proto::model::BlockedTerm {
                    id: id.to_owned(),
                    text: text.to_owned(),
                    created_at: "2026-10-01T18:30:00Z".to_owned(),
                    expires_at: expires.map(str::to_owned),
                };
            vec![
                term("term-a", "spoiler", None),
                term("term-b", "*spam*", Some("2026-10-20T18:30:00Z")),
            ]
        })
    }

    async fn emit(&self, event: Event) {
        // The engine is gone: nothing left to do, requests will stop too.
        let _ = self.events.send(BackendMessage::Event(event)).await;
    }

    /// What the channel has planned here, made up on first asking.
    fn schedule_of(&mut self, channel: &UserId) -> &mut liveside_proto::model::Schedule {
        self.schedule
            .entry(channel.clone())
            .or_insert_with(|| liveside_proto::model::Schedule {
                segments: vec![
                    liveside_proto::model::ScheduleSegment {
                        id: "seg-1".to_owned(),
                        start: "2026-10-07T18:00:00Z".to_owned(),
                        end: "2026-10-07T21:00:00Z".to_owned(),
                        title: "Rust a kafe".to_owned(),
                        category: "Software and Game Development".to_owned(),
                        weekly: true,
                        canceled: false,
                    },
                    liveside_proto::model::ScheduleSegment {
                        id: "seg-2".to_owned(),
                        start: "2026-10-10T17:00:00Z".to_owned(),
                        end: "2026-10-10T20:00:00Z".to_owned(),
                        title: "Hraní".to_owned(),
                        category: "Just Chatting".to_owned(),
                        weekly: false,
                        canceled: false,
                    },
                ],
                vacation: None,
            })
    }

    /// What the channel offers here, made up on first asking. One place,
    /// so that changing a reward cannot leave an empty list behind that
    /// the defaults would then never fill.
    fn rewards_of(&mut self, channel: &UserId) -> &mut Vec<Reward> {
        self.rewards.entry(channel.clone()).or_insert_with(|| {
            REWARDS
                .iter()
                .enumerate()
                .map(|(i, (title, cost))| Reward {
                    id: format!("r{i}"),
                    title: (*title).to_owned(),
                    cost: cost.parse().unwrap_or(0),
                    prompt: String::new(),
                    enabled: true,
                    paused: false,
                    ours: true,
                })
                .collect()
        })
    }

    /// A notice the frontends can say in their own language: the few demo
    /// answers that mirror something the real backend reports, and the
    /// line every join starts with, the first thing a demo shows.
    async fn notice_code(&self, channel: &UserId, code: &str, args: &[(&str, &str)], text: String) {
        self.emit(Event::Line {
            channel: channel.clone(),
            line: Line {
                time: Timestamp::now(),
                content: LineContent::Notice {
                    level: NoticeLevel::Info,
                    text,
                    code: code.to_owned(),
                    args: args
                        .iter()
                        .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
                        .collect(),
                },
                flags: LineFlags::default(),
            },
        })
        .await;
    }

    async fn notice(&self, channel: &UserId, text: String) {
        self.emit(Event::Line {
            channel: channel.clone(),
            line: Line {
                time: Timestamp::now(),
                content: LineContent::Notice {
                    level: NoticeLevel::Info,
                    text,
                    // The demo's own chatter is not worth translating.
                    code: String::new(),
                    args: Vec::new(),
                },
                flags: LineFlags::default(),
            },
        })
        .await;
    }

    /// Who the demo says did the moderating: whoever is logged in.
    fn moderator_name(&self) -> String {
        self.account
            .as_ref()
            .map(|a| a.user.display_name.clone())
            .unwrap_or_else(|| "someone".to_owned())
    }

    /// A channel event with a code, as the Twitch backend sends them, so
    /// the frontends' wording of follows and the rest can be seen here.
    async fn channel_event(
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
                    args: args
                        .iter()
                        .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
                        .collect(),
                },
                flags: LineFlags::default(),
            },
        })
        .await;
    }

    fn joined(&self, channel: &UserId) -> Result<&Joined, CommandError> {
        self.joined.get(channel).ok_or_else(|| {
            CommandError::new(
                ErrorKind::NotJoined,
                "channel.not_joined",
                &[("channel", channel.as_str())],
                format!("Not in channel {channel}"),
            )
        })
    }

    async fn handle(&mut self, request: BackendRequest) -> CommandResult {
        match request {
            BackendRequest::Login => {
                self.emit(Event::LoginPending {
                    uri: "https://www.twitch.tv/activate".into(),
                    code: "DEMO-CODE".into(),
                    expires_in_secs: 1800,
                })
                .await;
                let account = Account {
                    user: user("demo_user"),
                    scopes: vec!["user:read:chat".into(), "user:write:chat".into()],
                };
                self.account = Some(account.clone());
                self.emit(Event::LoggedIn { account }).await;
                // Followed channels are watched as the Twitch backend
                // watches them, only faster, so a start can be seen.
                if self.config.chatter_interval.is_some() && self.followed_watch.is_none() {
                    self.followed_watch = Some(tokio::spawn(watch_followed(
                        self.events.clone(),
                        self.quiet_corner_live.clone(),
                        self.quiet_corner_joined.clone(),
                    )));
                }
            }
            BackendRequest::Logout => {
                if let Some(task) = self.followed_watch.take() {
                    task.abort();
                }
                self.account = None;
                self.emit(Event::LoggedOut).await;
            }
            BackendRequest::Join { channel: login } => {
                let channel = channel(&login);
                if self.joined.contains_key(&channel.id) {
                    return Ok(());
                }
                self.emit(Event::ChannelJoined {
                    channel: channel.clone(),
                    modes: RoomModes::default(),
                })
                .await;
                let history = (0..3)
                    .map(|i| {
                        let n = mix(hash(login.as_str()) ^ i);
                        let mut line = generated_line(&channel, n, &self.next_message);
                        line.flags.historical = true;
                        line
                    })
                    .collect();
                self.emit(Event::History {
                    channel: channel.id.clone(),
                    lines: history,
                })
                .await;
                self.notice_code(
                    &channel.id,
                    "demo.joined",
                    &[("channel", login.as_str())],
                    format!("Joined #{login} (demo backend)"),
                )
                .await;
                // An invented audience, running for an hour and a bit; the
                // chatter keeps telling how many, from the same start.
                let started =
                    liveside_proto::Timestamp(liveside_proto::Timestamp::now().0 - 75 * 60_000);
                // Part of the invented liveliness, so a test that asks
                // for a quiet demo gets a quiet one.
                if self.config.chatter_interval.is_some() {
                    self.emit(Event::Stream {
                        channel: channel.id.clone(),
                        live: true,
                    })
                    .await;
                    self.emit(Event::StreamStats {
                        channel: channel.id.clone(),
                        stats: liveside_proto::model::StreamStats {
                            viewers: 1234,
                            started,
                        },
                    })
                    .await;
                    self.channel_event(
                        &channel.id,
                        NoticeLevel::Info,
                        "event.stream_online",
                        &[],
                        "The stream has started".to_owned(),
                    )
                    .await;
                    self.channel_event(
                        &channel.id,
                        NoticeLevel::Info,
                        "event.channel_update",
                        &[
                            ("title", "Invented chat"),
                            ("game", "Software and Game Development"),
                        ],
                        "Now: Invented chat — Software and Game Development".to_owned(),
                    )
                    .await;
                }
                if self.config.membership {
                    // As IRC does: the whole list at once after joining.
                    self.emit(Event::Chatters {
                        channel: channel.id.clone(),
                        logins: lurkers(&channel),
                    })
                    .await;
                }
                let chatter = self.config.chatter_interval.map(|interval| {
                    tokio::spawn(chatter(
                        channel.clone(),
                        started,
                        interval,
                        self.events.clone(),
                        self.next_message.clone(),
                        self.config.membership,
                        self.waiting.clone(),
                    ))
                });
                self.joined.insert(channel.id.clone(), chatter);
                if channel.login.as_str() == "quiet_corner" {
                    self.quiet_corner_joined.store(true, Ordering::Relaxed);
                }
            }
            BackendRequest::Part { channel } => {
                let joined = self.joined.remove(&channel).ok_or_else(|| {
                    CommandError::new(
                        ErrorKind::NotJoined,
                        "channel.not_joined",
                        &[("channel", channel.as_str())],
                        format!("Not in channel {channel}"),
                    )
                })?;
                if let Some(task) = joined {
                    task.abort();
                }
                if channel == self::channel(&"quiet_corner".into()).id {
                    self.quiet_corner_joined.store(false, Ordering::Relaxed);
                }
                self.emit(Event::ChannelParted { channel }).await;
            }
            // As a real /me comes back: the account's line, as an action.
            BackendRequest::SendAction { channel, text } => {
                self.joined(&channel)?;
                let Some(account) = &self.account else {
                    return Err(CommandError::new(
                        ErrorKind::NotLoggedIn,
                        "account.required_to_send",
                        &[],
                        "Log in to send messages",
                    ));
                };
                let id = self.next_message.fetch_add(1, Ordering::Relaxed);
                let mut message = message(id, account.user.clone(), &text);
                message.kind = MessageKind::Action;
                message.color = Some(Rgb {
                    r: 0x9b,
                    g: 0x59,
                    b: 0xb6,
                });
                self.emit(Event::Line {
                    channel,
                    line: Line {
                        time: Timestamp::now(),
                        content: LineContent::Message(message),
                        flags: LineFlags {
                            own: true,
                            ..Default::default()
                        },
                    },
                })
                .await;
            }
            BackendRequest::Send {
                channel,
                text,
                reply_to: _,
            } => {
                self.joined(&channel)?;
                let Some(account) = &self.account else {
                    return Err(CommandError::new(
                        ErrorKind::NotLoggedIn,
                        "account.required_to_send",
                        &[],
                        "Log in to send messages",
                    ));
                };
                let id = self.next_message.fetch_add(1, Ordering::Relaxed);
                let mut message = message(id, account.user.clone(), &text);
                message.color = Some(Rgb {
                    r: 0x9b,
                    g: 0x59,
                    b: 0xb6,
                });
                self.emit(Event::Line {
                    channel,
                    line: Line {
                        time: Timestamp::now(),
                        content: LineContent::Message(message),
                        flags: LineFlags {
                            own: true,
                            ..Default::default()
                        },
                    },
                })
                .await;
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
                self.joined(&channel)?;
                let info = self.info.entry(channel.clone()).or_insert_with(demo_info);
                // Only what was sent is touched, as Twitch does it.
                if let Some(title) = title {
                    info.title = title;
                }
                if let Some(game) = game {
                    info.game = game;
                }
                if let Some(language) = language {
                    info.language = language;
                }
                if let Some(tags) = tags {
                    info.tags = tags;
                }
                if let Some(delay) = delay {
                    info.delay = delay;
                }
                if let Some(labels) = labels {
                    info.labels = labels;
                }
                let info = info.clone();
                self.emit(Event::ChannelInfo { channel, info }).await;
            }
            BackendRequest::ShieldMode { channel, on } => {
                self.joined(&channel)?;
                // Said as state as well as in a line, so the menu item
                // in the window knows which way it goes next.
                self.emit(Event::ShieldMode {
                    channel: channel.clone(),
                    on,
                })
                .await;
                let state = if on { "on" } else { "off" };
                self.notice(&channel, format!("Shield Mode {state}")).await;
            }
            BackendRequest::BlockTerm { channel, text, on } => {
                self.joined(&channel)?;
                let id = format!("term-{}", self.next_message.fetch_add(1, Ordering::Relaxed));
                let terms = self.blocked_terms(&channel);
                if on {
                    terms.push(liveside_proto::model::BlockedTerm {
                        id,
                        text: text.clone(),
                        created_at: "2026-10-13T20:00:00Z".to_owned(),
                        expires_at: None,
                    });
                } else {
                    terms.retain(|term| !term.text.eq_ignore_ascii_case(&text));
                }
                let what = if on { "Blocked" } else { "Unblocked" };
                self.notice(&channel, format!("{what} the term {text:?}"))
                    .await;
            }
            BackendRequest::CreatePoll {
                channel,
                title,
                choices,
                seconds,
            } => {
                self.joined(&channel)?;
                if let Some((running, _)) = self.polls.remove(&channel) {
                    running.abort();
                }
                let poll = Poll {
                    id: format!("poll-{}", self.next_message.fetch_add(1, Ordering::Relaxed)),
                    title,
                    choices: choices
                        .into_iter()
                        .enumerate()
                        .map(|(i, title)| PollChoice {
                            id: format!("c{i}"),
                            title,
                            votes: 0,
                        })
                        .collect(),
                    status: PollStatus::Active,
                    ends_at: String::new(),
                };
                let shared = Arc::new(Mutex::new(poll));
                let task = tokio::spawn(voting(
                    channel.clone(),
                    shared.clone(),
                    seconds,
                    self.events.clone(),
                ));
                self.polls.insert(channel, (task, shared));
            }
            BackendRequest::EndPoll { channel, archive } => {
                self.joined(&channel)?;
                let Some((running, shared)) = self.polls.remove(&channel) else {
                    // Nothing to stop; said the way Twitch would say it.
                    return Err(CommandError::new(
                        ErrorKind::InvalidArgument,
                        "poll.none",
                        &[],
                        "No poll is running",
                    ));
                };
                running.abort();
                // Stopped with its votes kept, or stopped and hidden.
                let mut poll = shared.lock().expect("the poll").clone();
                poll.status = match archive {
                    true => PollStatus::Archived,
                    false => PollStatus::Terminated,
                };
                self.emit(Event::PollUpdate { channel, poll }).await;
            }
            BackendRequest::CreatePrediction {
                channel,
                title,
                outcomes,
                seconds,
            } => {
                self.joined(&channel)?;
                if let Some((running, _)) = self.predictions.remove(&channel) {
                    running.abort();
                }
                let prediction = Prediction {
                    id: format!("pred-{}", self.next_message.fetch_add(1, Ordering::Relaxed)),
                    title,
                    outcomes: outcomes
                        .into_iter()
                        .enumerate()
                        .map(|(i, title)| PredictionOutcome {
                            id: format!("o{i}"),
                            title,
                            users: 0,
                            points: 0,
                        })
                        .collect(),
                    status: PredictionStatus::Active,
                    locks_at: String::new(),
                    winner: None,
                };
                let shared = Arc::new(Mutex::new(prediction));
                let task = tokio::spawn(betting(
                    channel.clone(),
                    shared.clone(),
                    seconds,
                    self.events.clone(),
                ));
                self.predictions.insert(channel, (task, shared));
            }
            BackendRequest::EndPrediction { channel, how } => {
                self.joined(&channel)?;
                let Some((running, shared)) = self.predictions.get(&channel) else {
                    return Err(CommandError::new(
                        ErrorKind::InvalidArgument,
                        "prediction.none",
                        &[],
                        "No prediction is running",
                    ));
                };
                running.abort();
                let mut prediction = shared.lock().expect("the prediction").clone();
                match &how {
                    // Betting closes, the prediction stays: this is the
                    // state a broadcaster must not forget about.
                    liveside_proto::PredictionEnd::Lock => {
                        prediction.status = PredictionStatus::Locked;
                        *shared.lock().expect("the prediction") = prediction.clone();
                    }
                    liveside_proto::PredictionEnd::Resolve { outcome } => {
                        prediction.status = PredictionStatus::Resolved;
                        prediction.winner = Some(outcome.clone());
                        self.predictions.remove(&channel);
                    }
                    liveside_proto::PredictionEnd::Cancel => {
                        prediction.status = PredictionStatus::Canceled;
                        self.predictions.remove(&channel);
                    }
                }
                self.emit(Event::PredictionUpdate {
                    channel,
                    prediction,
                })
                .await;
            }
            BackendRequest::ShowChannelInfo { channel } => {
                self.joined(&channel)?;
                let info = self.info.entry(channel.clone()).or_insert_with(demo_info);
                let info = info.clone();
                self.emit(Event::ChannelInfo { channel, info }).await;
            }
            BackendRequest::ShowSchedule { channel } => {
                self.joined(&channel)?;
                let schedule = self.schedule_of(&channel).clone();
                self.emit(Event::Schedule { channel, schedule }).await;
            }
            BackendRequest::AddSegment {
                channel,
                start,
                minutes,
                title,
                weekly,
            } => {
                self.joined(&channel)?;
                let id = format!("seg-{}", self.next_message.fetch_add(1, Ordering::Relaxed));
                let schedule = self.schedule_of(&channel);
                schedule
                    .segments
                    .push(liveside_proto::model::ScheduleSegment {
                        id,
                        start,
                        end: String::new(),
                        title,
                        category: String::new(),
                        weekly,
                        canceled: false,
                    });
                let _ = minutes;
                let schedule = schedule.clone();
                self.emit(Event::Schedule { channel, schedule }).await;
            }
            BackendRequest::CancelSegment {
                channel,
                segment,
                canceled,
            } => {
                self.joined(&channel)?;
                let schedule = self.schedule_of(&channel);
                if let Some(found) = schedule.segments.iter_mut().find(|s| s.id == segment) {
                    found.canceled = canceled;
                }
                let schedule = schedule.clone();
                self.emit(Event::Schedule { channel, schedule }).await;
            }
            BackendRequest::DeleteSegment { channel, segment } => {
                self.joined(&channel)?;
                let schedule = self.schedule_of(&channel);
                schedule.segments.retain(|kept| kept.id != segment);
                let schedule = schedule.clone();
                self.emit(Event::Schedule { channel, schedule }).await;
            }
            BackendRequest::SetVacation { channel, away } => {
                self.joined(&channel)?;
                let schedule = self.schedule_of(&channel);
                schedule.vacation = away;
                let schedule = schedule.clone();
                self.emit(Event::Schedule { channel, schedule }).await;
            }
            BackendRequest::ShowAds { channel } | BackendRequest::SnoozeAd { channel } => {
                self.joined(&channel)?;
                self.notice_code(
                    &channel,
                    "ads.next",
                    &[
                        // Said the way the real backend says it: the day
                        // and the hour, without the seconds.
                        ("at", "2026-10-06 20:30"),
                        ("seconds", "90"),
                        ("snoozes", "2"),
                    ],
                    "Next advertisement break at 2026-10-06 20:30, 90s, snoozes left: 2".to_owned(),
                )
                .await;
            }
            BackendRequest::SendWhisper {
                user: whispered,
                text,
            } => {
                let Some(account) = self.account.clone() else {
                    return Err(CommandError::new(
                        ErrorKind::NotLoggedIn,
                        "account.required",
                        &[],
                        "Log in first",
                    ));
                };
                // Said back as our own line, the way the real backend
                // does: Twitch does not echo a whisper we sent.
                let mine = Line {
                    time: Timestamp::now(),
                    content: LineContent::Message(ChatMessage {
                        id: format!("w{}", self.next_message.fetch_add(1, Ordering::Relaxed))
                            .into(),
                        sender: account.user.clone(),
                        color: None,
                        badges: Vec::new(),
                        fragments: vec![Fragment::Text { text: text.clone() }],
                        kind: MessageKind::Text,
                        bits: None,
                        reply: None,
                        shared_source: None,
                    }),
                    flags: LineFlags {
                        own: true,
                        ..LineFlags::default()
                    },
                };
                self.emit(Event::Whisper {
                    user: user(whispered.as_str()),
                    line: mine,
                })
                .await;
                // ...and an answer, so a thread has two sides to it.
                let reply = Line {
                    time: Timestamp::now(),
                    content: LineContent::Message(ChatMessage {
                        id: format!("w{}", self.next_message.fetch_add(1, Ordering::Relaxed))
                            .into(),
                        sender: user(whispered.as_str()),
                        color: None,
                        badges: Vec::new(),
                        fragments: vec![Fragment::Text {
                            text: "díky, mrknu na to".to_owned(),
                        }],
                        kind: MessageKind::Text,
                        bits: None,
                        reply: None,
                        shared_source: None,
                    }),
                    flags: LineFlags::default(),
                };
                self.emit(Event::Whisper {
                    user: user(whispered.as_str()),
                    line: reply,
                })
                .await;
            }
            BackendRequest::CreateClip { channel } => {
                self.joined(&channel)?;
                let id = format!(
                    "DemoClip{}",
                    self.next_message.fetch_add(1, Ordering::Relaxed)
                );
                self.notice_code(
                    &channel,
                    "clip.created",
                    &[("url", &format!("https://clips.twitch.tv/{id}/edit"))],
                    format!("Clip made: https://clips.twitch.tv/{id}/edit"),
                )
                .await;
            }
            BackendRequest::ShowRewards { channel } => {
                self.joined(&channel)?;
                let rewards = self.rewards_of(&channel).clone();
                self.emit(Event::Rewards { channel, rewards }).await;
            }
            BackendRequest::CreateReward {
                channel,
                title,
                cost,
                prompt,
            } => {
                self.joined(&channel)?;
                let id = format!("r{}", self.next_message.fetch_add(1, Ordering::Relaxed));
                let reward = Reward {
                    id,
                    title,
                    cost,
                    prompt,
                    enabled: true,
                    paused: false,
                    ours: true,
                };
                let rewards = self.rewards_of(&channel);
                rewards.push(reward);
                let rewards = rewards.clone();
                self.emit(Event::Rewards { channel, rewards }).await;
            }
            BackendRequest::UpdateReward {
                channel,
                reward,
                title,
                cost,
                prompt,
                enabled,
                paused,
            } => {
                self.joined(&channel)?;
                let rewards = self.rewards_of(&channel);
                // By id or by name, since a command takes the name.
                let found = rewards
                    .iter_mut()
                    .find(|kept| kept.id == reward || kept.title.eq_ignore_ascii_case(&reward));
                let Some(found) = found else {
                    return Err(CommandError::new(
                        ErrorKind::InvalidArgument,
                        "reward.unknown",
                        &[("reward", &reward)],
                        format!("No reward called {reward}"),
                    ));
                };
                if let Some(title) = title {
                    found.title = title;
                }
                if let Some(cost) = cost {
                    found.cost = cost;
                }
                if let Some(prompt) = prompt {
                    found.prompt = prompt;
                }
                if let Some(enabled) = enabled {
                    found.enabled = enabled;
                }
                if let Some(paused) = paused {
                    found.paused = paused;
                }
                let rewards = rewards.clone();
                self.emit(Event::Rewards { channel, rewards }).await;
            }
            BackendRequest::DeleteReward { channel, reward } => {
                self.joined(&channel)?;
                let rewards = self.rewards_of(&channel);
                rewards.retain(|kept| kept.id != reward);
                let rewards = rewards.clone();
                self.emit(Event::Rewards { channel, rewards }).await;
            }
            BackendRequest::AnswerRedemption {
                channel,
                redemption,
                fulfil,
                // Twitch wants the reward's id as well; the demo finds
                // the redemption by its own id and needs no more.
                reward: _,
            } => {
                self.joined(&channel)?;
                // Answered with what it was, the way Twitch answers: a
                // stub would make the line about it say nothing.
                let answered = {
                    let mut waiting = self.waiting.lock().expect("the queue");
                    waiting
                        .iter()
                        .position(|kept| kept.id == redemption)
                        .map(|i| waiting.remove(i))
                };
                let Some(mut answered) = answered else {
                    return Err(CommandError::new(
                        ErrorKind::InvalidArgument,
                        "redemption.unknown",
                        &[],
                        "No such redemption is waiting",
                    ));
                };
                answered.status = match fulfil {
                    true => RedemptionStatus::Fulfilled,
                    false => RedemptionStatus::Canceled,
                };
                self.emit(Event::RedemptionUpdate {
                    channel,
                    redemption: answered,
                })
                .await;
            }
            BackendRequest::ShowUser {
                channel,
                user: login,
            } => {
                self.joined(&channel)?;
                let who = user(login.as_str());
                self.emit(Event::UserCard {
                    channel,
                    card: liveside_proto::event::UserCard {
                        user: who,
                        created_at: "2019-04-01T12:00:00Z".to_owned(),
                        follows_since: Some("2021-06-02T19:30:00Z".to_owned()),
                        follows_known: true,
                        sub_known: true,
                        sub_tier: "2".to_owned(),
                        sub_gift: true,
                        banned: false,
                        banned_until: None,
                    },
                })
                .await;
            }
            BackendRequest::ShowPeople { channel, list } => {
                self.joined(&channel)?;
                let person = |name: &str, detail: &str| liveside_proto::event::Person {
                    name: name.to_owned(),
                    detail: detail.to_owned(),
                    gifted_by: String::new(),
                };
                let (mut total, mut points) = (None, None);
                let (kind, people) = match list {
                    crate::backend::PeopleList::Moderators => (
                        liveside_proto::event::PeopleList::Moderators,
                        vec![person("carol", ""), person("dave_42", "")],
                    ),
                    crate::backend::PeopleList::Vips => (
                        liveside_proto::event::PeopleList::Vips,
                        vec![person("alice", "")],
                    ),
                    crate::backend::PeopleList::Banned => (
                        liveside_proto::event::PeopleList::Banned,
                        vec![
                            person("spammer", "2026-10-02T21:00:00Z"),
                            person("bot_99", ""),
                        ],
                    ),
                    crate::backend::PeopleList::Followers => {
                        total = Some(1_234);
                        (
                            liveside_proto::event::PeopleList::Followers,
                            vec![
                                person("eve", "2026-10-15T18:02:11Z"),
                                person("bob_the_viewer", "2026-09-30T20:45:00Z"),
                                person("alice", "2025-12-24T09:00:00Z"),
                            ],
                        )
                    }
                    crate::backend::PeopleList::Subscribers => {
                        total = Some(3);
                        points = Some(7);
                        (
                            liveside_proto::event::PeopleList::Subscribers,
                            vec![
                                person("carol", "3"),
                                person("dave_42", "1"),
                                liveside_proto::event::Person {
                                    gifted_by: "carol".to_owned(),
                                    ..person("eve", "1")
                                },
                            ],
                        )
                    }
                };
                self.emit(Event::People {
                    channel,
                    list: kind,
                    people,
                    total,
                    points,
                })
                .await;
            }
            BackendRequest::ListEmotes { channel } => {
                use liveside_proto::model::{EmoteGroup, EmoteGroupKind};
                self.joined(&channel)?;
                // The invented channel's own emotes are two of the real
                // global ones, so that their pictures load.
                let emote = |&(code, id): &(&str, &str)| EmoteRef {
                    provider: EmoteProvider::Twitch,
                    id: id.into(),
                    code: code.into(),
                    animated: false,
                    zero_width: false,
                };
                let (own, global): (Vec<_>, Vec<_>) = EMOTES
                    .iter()
                    .partition(|(code, _)| matches!(*code, "SeemsGood" | "LUL"));
                let group = |kind, channel: &str, list: Vec<&(&str, &str)>| EmoteGroup {
                    kind,
                    channel: channel.to_owned(),
                    provider: EmoteProvider::Twitch,
                    emotes: list.into_iter().map(emote).collect(),
                };
                self.emit(Event::EmoteList {
                    channel,
                    groups: vec![
                        // Only the id is kept here; the frontend knows the name.
                        group(EmoteGroupKind::ThisChannel, "", own),
                        group(EmoteGroupKind::Twitch, "", global),
                    ],
                    complete: true,
                })
                .await;
            }
            BackendRequest::ShowModeratedChannels => {
                let channel = |id: &str, login: &str, name: &str| Channel {
                    id: id.into(),
                    login: login.into(),
                    display_name: name.to_owned(),
                };
                self.emit(Event::ModeratedChannels {
                    channels: vec![
                        channel("2001", "orbitcast", "Orbitcast"),
                        channel("2002", "brightmaple", "brightmaple"),
                        channel("2003", "pixel_garden", "Pixel_Garden"),
                    ],
                })
                .await;
            }
            // Two or three streaming, the rest not, so both halves of the
            // list show.
            BackendRequest::ShowFollowedChannels => {
                let mut channels = followed_live(self.quiet_corner_live.load(Ordering::Relaxed));
                for login in ["orbitcast", "brightmaple", "quiet_corner"] {
                    if !channels.iter().any(|f| f.channel.login.as_str() == login) {
                        channels.push(liveside_proto::model::FollowedChannel {
                            channel: channel(&login.into()),
                            live: None,
                        });
                    }
                }
                self.emit(Event::FollowedChannels { channels }).await;
            }
            BackendRequest::ShowBlockedTerms { channel } => {
                self.joined(&channel)?;
                let terms = self.blocked_terms(&channel).clone();
                self.emit(Event::BlockedTerms { channel, terms }).await;
            }
            BackendRequest::SetSuspicious {
                channel,
                user,
                status,
            } => {
                self.joined(&channel)?;
                let (_, name) = target(&user);
                // The same line Twitch sends back as
                // channel.suspicious_user.update, so the demo shows what
                // the real thing says, in the reader's language.
                let code = match status {
                    crate::backend::Suspicion::Monitored => "mod.suspicion_monitored",
                    crate::backend::Suspicion::Restricted => "mod.suspicion_restricted",
                    crate::backend::Suspicion::None => "mod.suspicion_cleared",
                };
                let me = self
                    .account
                    .as_ref()
                    .map(|account| account.user.display_name.clone())
                    .unwrap_or_else(|| "demo_user".to_owned());
                let text = match status {
                    crate::backend::Suspicion::Monitored => {
                        format!("{me} put {name} under watch")
                    }
                    crate::backend::Suspicion::Restricted => {
                        format!("{me} restricted {name}: their messages are held")
                    }
                    crate::backend::Suspicion::None => format!("{me} took {name} off watch"),
                };
                self.notice_code(
                    &channel,
                    code,
                    &[("moderator", me.as_str()), ("user", name.as_str())],
                    text,
                )
                .await;
            }
            BackendRequest::Shoutout { channel, to } => {
                self.joined(&channel)?;
                let (_, name) = target(&to);
                self.notice(&channel, format!("Shouted out {name}")).await;
            }
            BackendRequest::Raid { channel, to } => {
                self.joined(&channel)?;
                let (_, name) = target(&to);
                self.notice(&channel, format!("Raiding {name} in 90 seconds"))
                    .await;
            }
            BackendRequest::CancelRaid { channel } => {
                self.joined(&channel)?;
                self.notice(&channel, "Raid called off".to_owned()).await;
            }
            BackendRequest::Marker {
                channel,
                description,
            } => {
                self.joined(&channel)?;
                let note = description.map(|d| format!(": {d}")).unwrap_or_default();
                self.notice(
                    &channel,
                    format!("Marker set at 12:34 into the stream{note}"),
                )
                .await;
            }
            BackendRequest::Commercial { channel, seconds } => {
                self.joined(&channel)?;
                self.notice(&channel, format!("Running a {seconds}s commercial"))
                    .await;
            }
            BackendRequest::ResolveUnban {
                channel,
                request,
                allow,
            } => {
                self.joined(&channel)?;
                self.emit(Event::UnbanResolved {
                    channel,
                    request,
                    status: if allow { "approved" } else { "denied" }.to_owned(),
                    moderator: self
                        .account
                        .as_ref()
                        .map(|a| a.user.display_name.clone())
                        .unwrap_or_default(),
                })
                .await;
            }
            BackendRequest::ResolveHeld {
                channel,
                message,
                allow,
            } => {
                self.joined(&channel)?;
                self.emit(Event::HeldResolved {
                    channel: channel.clone(),
                    message,
                    status: if allow { "approved" } else { "denied" }.to_owned(),
                    moderator: self
                        .account
                        .as_ref()
                        .map(|a| a.user.display_name.clone())
                        .unwrap_or_default(),
                })
                .await;
            }
            BackendRequest::Pin {
                channel,
                message,
                seconds,
            } => {
                self.joined(&channel)?;
                let how_long = match seconds {
                    Some(seconds) => format!("for {seconds}s"),
                    None => "until the stream ends".to_owned(),
                };
                self.notice(&channel, format!("Pinned message {message} {how_long}"))
                    .await;
            }
            BackendRequest::Unpin { channel } => {
                self.joined(&channel)?;
                self.notice(&channel, "Unpinned the pinned message".to_owned())
                    .await;
            }
            BackendRequest::Delete { channel, message } => {
                self.joined(&channel)?;
                self.emit(Event::MessageDeleted { channel, message }).await;
            }
            BackendRequest::Timeout {
                channel,
                user,
                seconds,
                reason,
            } => {
                self.joined(&channel)?;
                let (id, name) = target(&user);
                self.emit(Event::UserCleared {
                    channel: channel.clone(),
                    user: id,
                    seconds: Some(seconds),
                })
                .await;
                // As the real backend does it: a coded line, so both
                // frontends say it in the reader's language.
                let me = self.moderator_name();
                self.channel_event(
                    &channel,
                    NoticeLevel::Info,
                    "mod.timeout",
                    &[
                        ("moderator", &me),
                        ("user", &name),
                        ("detail", &reason_suffix(&reason)),
                    ],
                    format!("{me} timed {name} out{}", reason_suffix(&reason)),
                )
                .await;
            }
            BackendRequest::Ban {
                channel,
                user,
                reason,
            } => {
                self.joined(&channel)?;
                let (id, name) = target(&user);
                self.emit(Event::UserCleared {
                    channel: channel.clone(),
                    user: id,
                    seconds: None,
                })
                .await;
                let me = self.moderator_name();
                self.channel_event(
                    &channel,
                    NoticeLevel::Info,
                    "mod.ban",
                    &[
                        ("moderator", &me),
                        ("user", &name),
                        ("detail", &reason_suffix(&reason)),
                    ],
                    format!("{me} banned {name}{}", reason_suffix(&reason)),
                )
                .await;
            }
            BackendRequest::Unban { channel, user } => {
                self.joined(&channel)?;
                let (_, name) = target(&user);
                self.notice(&channel, format!("{name} unbanned")).await;
            }
            BackendRequest::Warn {
                channel,
                user,
                reason,
            } => {
                self.joined(&channel)?;
                let (_, name) = target(&user);
                self.notice(&channel, format!("{name} warned: {reason}"))
                    .await;
            }
            BackendRequest::ClearChat { channel } => {
                self.joined(&channel)?;
                self.emit(Event::ChatCleared {
                    channel: channel.clone(),
                })
                .await;
            }
            BackendRequest::Announce { channel, text, .. } => {
                self.joined(&channel)?;
                self.notice(&channel, format!("Announcement: {text}")).await;
            }
            BackendRequest::SetRoomMode { channel, mode } => {
                self.joined(&channel)?;
                // Twitch reports the new modes back over EventSub; the
                // demo has to keep them itself, or asking for slow mode
                // would change nothing anybody can see.
                let modes = self.modes.entry(channel.clone()).or_default();
                match mode {
                    RoomModeChange::Slow(seconds) => modes.slow = seconds,
                    RoomModeChange::Followers(minutes) => modes.followers_only = minutes,
                    RoomModeChange::Subscribers(on) => modes.subscribers_only = on,
                    RoomModeChange::EmoteOnly(on) => modes.emote_only = on,
                    RoomModeChange::UniqueChat(on) => modes.unique_chat = on,
                    // Nothing on screen says what this one is.
                    RoomModeChange::NonModeratorDelay(_) => {}
                }
                let modes = modes.clone();
                self.emit(Event::RoomModesChanged { channel, modes }).await;
            }
            BackendRequest::SetRole {
                channel,
                user,
                role,
                on,
            } => {
                self.joined(&channel)?;
                let (_, name) = target(&user);
                let verb = if on { "given to" } else { "taken from" };
                self.notice(&channel, format!("{role:?} {verb} {name}"))
                    .await;
            }
        }
        Ok(())
    }
}

/// A poll voting for itself: a count every two seconds, and the result
/// when the time is up.
async fn voting(
    channel: UserId,
    poll: Arc<Mutex<Poll>>,
    seconds: u32,
    events: mpsc::Sender<BackendMessage>,
) {
    let send = |poll: Poll| {
        let events = events.clone();
        let channel = channel.clone();
        async move {
            let event = Event::PollUpdate { channel, poll };
            events.send(BackendMessage::Event(event)).await.is_ok()
        }
    };
    let start = poll.lock().expect("the poll").clone();
    if !send(start.clone()).await {
        return;
    }
    let mut n = hash(start.id.as_str());
    let steps = (seconds / 2).max(1);
    for _ in 0..steps {
        tokio::time::sleep(Duration::from_secs(2)).await;
        let counted = {
            let mut poll = poll.lock().expect("the poll");
            for choice in &mut poll.choices {
                n = mix(n);
                choice.votes += (n % 7) as u32;
            }
            poll.clone()
        };
        if !send(counted).await {
            return;
        }
    }
    let finished = {
        let mut poll = poll.lock().expect("the poll");
        poll.status = PollStatus::Completed;
        poll.clone()
    };
    send(finished).await;
}

/// A prediction betting on itself while the window is open, and then
/// locking: unlike a poll it does not finish, it waits to be paid out.
async fn betting(
    channel: UserId,
    prediction: Arc<Mutex<Prediction>>,
    seconds: u32,
    events: mpsc::Sender<BackendMessage>,
) {
    let send = |prediction: Prediction| {
        let events = events.clone();
        let channel = channel.clone();
        async move {
            let event = Event::PredictionUpdate {
                channel,
                prediction,
            };
            events.send(BackendMessage::Event(event)).await.is_ok()
        }
    };
    let start = prediction.lock().expect("the prediction").clone();
    if !send(start.clone()).await {
        return;
    }
    let mut n = hash(start.id.as_str());
    let steps = (seconds / 2).max(1);
    for _ in 0..steps {
        tokio::time::sleep(Duration::from_secs(2)).await;
        let counted = {
            let mut prediction = prediction.lock().expect("the prediction");
            for outcome in &mut prediction.outcomes {
                n = mix(n);
                outcome.users += (n % 3) as u32;
                outcome.points += n % 500;
            }
            prediction.clone()
        };
        if !send(counted).await {
            return;
        }
    }
    let locked = {
        let mut prediction = prediction.lock().expect("the prediction");
        prediction.status = PredictionStatus::Locked;
        prediction.clone()
    };
    send(locked).await;
}

/// What the demo channel says about itself before anything changes it.
fn demo_info() -> liveside_proto::model::ChannelInfo {
    liveside_proto::model::ChannelInfo {
        title: "Invented chat — Software and Game Development".to_owned(),
        game: "Software and Game Development".to_owned(),
        language: "cs".to_owned(),
        tags: vec!["rust".to_owned(), "cesky".to_owned()],
        delay: 0,
        labels: Vec::new(),
    }
}

/// The followed channels that stream in the demo: two always, and
/// quiet_corner when it does.
fn followed_live(quiet_corner: bool) -> Vec<liveside_proto::model::FollowedChannel> {
    use liveside_proto::model::{FollowedChannel, LiveStream, StreamStats};
    let minutes_ago = |m: i64| Timestamp(Timestamp::now().0 - m * 60_000);
    let live = |login: &str, category: &str, title: &str, viewers, minutes| FollowedChannel {
        channel: channel(&login.into()),
        live: Some(LiveStream {
            title: title.to_owned(),
            category: category.to_owned(),
            stats: StreamStats {
                viewers,
                started: minutes_ago(minutes),
            },
        }),
    };
    let mut list = vec![
        live(
            "pixel_garden",
            "Art",
            "Cozy pixel art, chat and tea",
            1234,
            125,
        ),
        live("night_owl_radio", "Music", "Lo-fi until sunrise", 87, 40),
    ];
    if quiet_corner {
        list.push(live(
            "quiet_corner",
            "Just Chatting",
            "Morning coffee",
            12,
            0,
        ));
    }
    list
}

/// Says every twenty seconds which followed channels stream, with
/// quiet_corner starting and stopping in turn: how things stand first,
/// then a start to be told about, unless it is joined, then an end that
/// is not.
async fn watch_followed(
    events: mpsc::Sender<BackendMessage>,
    quiet_corner: Arc<AtomicBool>,
    joined: Arc<AtomicBool>,
) {
    let mut ticker = tokio::time::interval(Duration::from_secs(20));
    let mut first = true;
    loop {
        ticker.tick().await;
        let now = if first {
            quiet_corner.load(Ordering::Relaxed)
        } else {
            !quiet_corner.fetch_xor(true, Ordering::Relaxed)
        };
        let started = if now && !first && !joined.load(Ordering::Relaxed) {
            vec![channel(&"quiet_corner".into()).id]
        } else {
            Vec::new()
        };
        first = false;
        let event = Event::FollowedStreams {
            live: followed_live(now),
            started,
        };
        if events.send(BackendMessage::Event(event)).await.is_err() {
            return;
        }
    }
}

/// Titles the invented stream changes to.
const TITLES: &[&str] = &["Invented chat", "Fixing the graph", "Questions"];

async fn chatter(
    channel: Channel,
    started: Timestamp,
    interval: Duration,
    events: mpsc::Sender<BackendMessage>,
    next_message: Arc<AtomicU64>,
    membership: bool,
    waiting: Arc<Mutex<Vec<Redemption>>>,
) {
    let mut ticker = tokio::time::interval(interval);
    ticker.tick().await; // The first tick is immediate.
    let mut n = hash(channel.login.as_str());
    let (mut viewers, mut ticks) = (1234u32, 0u64);
    loop {
        ticker.tick().await;
        n = mix(n);
        ticks += 1;
        // How many watch, every twenty lines or so, as Twitch would say
        // it once a minute: a few more or a few less each time.
        if ticks.is_multiple_of(20) {
            let step = (n % 121) as i64 - 55;
            viewers = (i64::from(viewers) + step).clamp(200, 5000) as u32;
            let event = Event::StreamStats {
                channel: channel.id.clone(),
                stats: liveside_proto::model::StreamStats { viewers, started },
            };
            if events.send(BackendMessage::Event(event)).await.is_err() {
                return;
            }
        }
        // And now and then a new title, which the graph marks.
        if ticks.is_multiple_of(150) {
            let title = TITLES[(ticks / 150) as usize % TITLES.len()];
            let line = Line {
                time: Timestamp::now(),
                content: LineContent::Notice {
                    level: NoticeLevel::Info,
                    text: format!("Now: {title} — Software and Game Development"),
                    code: "event.channel_update".to_owned(),
                    args: vec![
                        ("title".to_owned(), title.to_owned()),
                        (
                            "game".to_owned(),
                            "Software and Game Development".to_owned(),
                        ),
                    ],
                },
                flags: LineFlags::default(),
            };
            let event = Event::Line {
                channel: channel.id.clone(),
                line,
            };
            if events.send(BackendMessage::Event(event)).await.is_err() {
                return;
            }
        }
        let line = generated_line(&channel, n, &next_message);
        let event = BackendMessage::Event(Event::Line {
            channel: channel.id.clone(),
            line,
        });
        if events.send(event).await.is_err() {
            return;
        }
        // And now and then somebody banned asks to come back.
        if membership && n.is_multiple_of(13) {
            let who = CHATTERS[(n >> 7) as usize % CHATTERS.len()];
            let event = Event::UnbanRequested {
                channel: channel.id.clone(),
                request: liveside_proto::event::UnbanRequest {
                    id: format!("unban-{n}"),
                    user: user(who),
                    text: "sorry, it will not happen again".to_owned(),
                    time: Timestamp::now(),
                },
            };
            if events.send(BackendMessage::Event(event)).await.is_err() {
                return;
            }
        }
        // Every so often AutoMod catches something, so the queue can be
        // tried without a channel to moderate.
        if membership && n.is_multiple_of(11) {
            let who = CHATTERS[(n >> 5) as usize % CHATTERS.len()];
            let held = liveside_proto::event::HeldMessage {
                id: format!("held-{n}").into(),
                sender: user(who),
                text: "you are all a bunch of ***".to_owned(),
                reason: "automod".to_owned(),
                detail: "aggressive 2".to_owned(),
                time: Timestamp::now(),
            };
            let event = Event::MessageHeld {
                channel: channel.id.clone(),
                held,
            };
            if events.send(BackendMessage::Event(event)).await.is_err() {
                return;
            }
        }
        // Now and then somebody Twitch does not trust says something.
        // The message goes first and the suspicion after it, which is the
        // harder of the two orders to get right. Often enough to be seen:
        // a demo exists to show what the real thing does rarely.
        if n.is_multiple_of(6) {
            let who = CHATTERS[(n >> 13) as usize % CHATTERS.len()];
            let line = generated_line(&channel, mix(n), &next_message);
            // Only a message can be marked; anything else this made is
            // left alone rather than stopping the generator.
            if let LineContent::Message(message) = &line.content {
                let id = message.id.clone();
                let said = Event::Line {
                    channel: channel.id.clone(),
                    line: line.clone(),
                };
                if events.send(BackendMessage::Event(said)).await.is_err() {
                    return;
                }
                let marked = Event::SuspiciousMessage {
                    channel: channel.id.clone(),
                    message: id,
                    user: user(who),
                    suspicion: Box::new(liveside_proto::model::Suspicion {
                        status: liveside_proto::model::SuspicionStatus::Monitored,
                        reasons: vec!["ban_evader".to_owned()],
                        evasion: "likely".to_owned(),
                        shared_bans: 0,
                    }),
                };
                if events.send(BackendMessage::Event(marked)).await.is_err() {
                    return;
                }
            }
        }
        // And now and then another moderator decides about one of them,
        // which is what channel.suspicious_user.update reports: without
        // it, one moderator's decision is invisible to the next.
        if n.is_multiple_of(29) {
            let about = (n >> 9) as usize % CHATTERS.len();
            let who = CHATTERS[about];
            // Someone else: a moderator deciding about themselves reads
            // like a bug even when it is only a demo.
            let step = 1 + (n >> 3) as usize % (CHATTERS.len() - 1);
            let moderator = CHATTERS[(about + step) % CHATTERS.len()];
            let (code, text) = match n.is_multiple_of(58) {
                false => (
                    "mod.suspicion_monitored",
                    format!("{moderator} put {who} under watch"),
                ),
                true => (
                    "mod.suspicion_cleared",
                    format!("{moderator} took {who} off watch"),
                ),
            };
            let event = Event::Line {
                channel: channel.id.clone(),
                line: Line {
                    time: Timestamp::now(),
                    content: LineContent::Notice {
                        level: NoticeLevel::Warning,
                        text,
                        code: code.to_owned(),
                        args: vec![
                            ("moderator".to_owned(), moderator.to_owned()),
                            ("user".to_owned(), who.to_owned()),
                        ],
                    },
                    flags: LineFlags::default(),
                },
            };
            if events.send(BackendMessage::Event(event)).await.is_err() {
                return;
            }
        }
        // Every so often a message from the channel this one is sharing
        // its chat with, so the mark on it can be seen.
        if n.is_multiple_of(17) {
            let who = CHATTERS[(n >> 11) as usize % CHATTERS.len()];
            let mut line = generated_line(&channel, mix(n), &next_message);
            if let LineContent::Message(message) = &mut line.content {
                message.sender = user(who);
                message.shared_source = Some(Box::new(liveside_proto::model::SharedSource {
                    channel: Channel {
                        id: "9001".into(),
                        login: "sousedni_kanal".into(),
                        display_name: "sousedni_kanal".to_owned(),
                    },
                    message_id: message.id.clone(),
                    badges: Vec::new(),
                }));
            }
            let event = Event::Line {
                channel: channel.id.clone(),
                line,
            };
            if events.send(BackendMessage::Event(event)).await.is_err() {
                return;
            }
        }
        // And now and then somebody pays for something, which is what a
        // broadcaster keeps the window open for.
        if n.is_multiple_of(9) {
            let who = CHATTERS[(n >> 9) as usize % CHATTERS.len()];
            let (code, text, args) = match (n >> 3) % 4 {
                0 => (
                    "money.subscribe",
                    format!("{who} subscribed (tier 1)"),
                    vec![
                        ("user".to_owned(), who.to_owned()),
                        ("tier".to_owned(), "1".to_owned()),
                    ],
                ),
                1 => (
                    "money.resubscribe",
                    format!("{who} resubscribed, 7 months (tier 1)"),
                    vec![
                        ("user".to_owned(), who.to_owned()),
                        ("tier".to_owned(), "1".to_owned()),
                        ("months".to_owned(), "7".to_owned()),
                        ("text".to_owned(), "díky za streamy!".to_owned()),
                    ],
                ),
                2 => (
                    "money.gift",
                    format!("{who} gave 5 subscriptions (tier 1)"),
                    vec![
                        ("user".to_owned(), who.to_owned()),
                        ("tier".to_owned(), "1".to_owned()),
                        ("count".to_owned(), "5".to_owned()),
                    ],
                ),
                _ => (
                    "money.cheer",
                    format!("{who} cheered 500 bits"),
                    vec![
                        ("user".to_owned(), who.to_owned()),
                        ("bits".to_owned(), "500".to_owned()),
                        ("text".to_owned(), "PogChamp".to_owned()),
                    ],
                ),
            };
            let event = Event::Line {
                channel: channel.id.clone(),
                line: Line {
                    time: Timestamp::now(),
                    content: LineContent::Notice {
                        level: NoticeLevel::Info,
                        text,
                        code: code.to_owned(),
                        args,
                    },
                    flags: LineFlags::default(),
                },
            };
            if events.send(BackendMessage::Event(event)).await.is_err() {
                return;
            }
        }
        // And now and then somebody spends their channel points, with or
        // without something typed into the reward.
        if n.is_multiple_of(11) {
            let who = CHATTERS[(n >> 5) as usize % CHATTERS.len()];
            let (reward, cost) = REWARDS[(n >> 7) as usize % REWARDS.len()];
            let asked = n.is_multiple_of(22);
            let (code, text, args) = match asked {
                false => (
                    "event.redemption",
                    format!("{who} redeemed {reward} ({cost} points)"),
                    vec![
                        ("user".to_owned(), who.to_owned()),
                        ("reward".to_owned(), reward.to_owned()),
                        ("cost".to_owned(), cost.to_owned()),
                    ],
                ),
                true => (
                    "event.redemption_text",
                    format!("{who} redeemed {reward} ({cost} points): hraj Factorio"),
                    vec![
                        ("user".to_owned(), who.to_owned()),
                        ("reward".to_owned(), reward.to_owned()),
                        ("cost".to_owned(), cost.to_owned()),
                        ("text".to_owned(), "hraj Factorio".to_owned()),
                    ],
                ),
            };
            let event = Event::Line {
                channel: channel.id.clone(),
                line: Line {
                    time: Timestamp::now(),
                    content: LineContent::Notice {
                        level: NoticeLevel::Info,
                        text,
                        code: code.to_owned(),
                        args,
                    },
                    flags: LineFlags::default(),
                },
            };
            if events.send(BackendMessage::Event(event)).await.is_err() {
                return;
            }
            // And into the queue, since the demo pretends every reward
            // here was made by this client — the only case Twitch lets
            // anybody answer.
            let redemption = Redemption {
                id: format!("red-{n}"),
                reward_id: format!("r{}", (n >> 7) as usize % REWARDS.len()),
                reward_title: reward.to_owned(),
                cost: cost.parse().unwrap_or(0),
                user: user(who),
                text: match asked {
                    true => "hraj Factorio".to_owned(),
                    false => String::new(),
                },
                status: RedemptionStatus::Unfulfilled,
                time: Timestamp::now(),
            };
            waiting.lock().expect("the queue").push(redemption.clone());
            let event = Event::RedemptionUpdate {
                channel: channel.id.clone(),
                redemption,
            };
            if events.send(BackendMessage::Event(event)).await.is_err() {
                return;
            }
        }
        // Now and then somebody follows, which is the kind of event a
        // broadcaster keeps the window open for.
        if n.is_multiple_of(7) {
            let who = CHATTERS[(n >> 3) as usize % CHATTERS.len()];
            let event = Event::Line {
                channel: channel.id.clone(),
                line: Line {
                    time: Timestamp::now(),
                    content: LineContent::Notice {
                        level: NoticeLevel::Info,
                        text: format!("{who} is now following"),
                        code: "event.follow".to_owned(),
                        args: vec![("user".to_owned(), who.to_owned())],
                    },
                    flags: LineFlags::default(),
                },
            };
            if events.send(BackendMessage::Event(event)).await.is_err() {
                return;
            }
        }
        // Every so often somebody comes or goes without saying anything,
        // which is the other half of what IRC membership reports.
        if membership && n.is_multiple_of(5) {
            let login = Login(format!("lurker_{:03}", mix(n) % 1000));
            let event = if n.is_multiple_of(10) {
                Event::ChatterJoined {
                    channel: channel.id.clone(),
                    login,
                }
            } else {
                Event::ChatterParted {
                    channel: channel.id.clone(),
                    login,
                }
            };
            if events.send(BackendMessage::Event(event)).await.is_err() {
                return;
            }
        }
    }
}

fn generated_line(channel: &Channel, n: u64, next_message: &AtomicU64) -> Line {
    // Two independent draws, so the same speaker does not always come with
    // the same phrase.
    let sender = user(CHATTERS[n as usize % CHATTERS.len()]);
    let phrase =
        PHRASES[mix(n) as usize % PHRASES.len()].replace("{streamer}", channel.login.as_str());
    let id = next_message.fetch_add(1, Ordering::Relaxed);
    let mut message = message(id, sender, &phrase);
    let h = hash(message.sender.login.as_str());
    message.color = Some(Rgb {
        r: 80 + (h % 150) as u8,
        g: 80 + ((h >> 8) % 150) as u8,
        b: 80 + ((h >> 16) % 150) as u8,
    });
    if h.is_multiple_of(3) {
        // Twitch's own picture for the badge, as the real backend would
        // attach: the demo is also how the drawing of badges is tried out.
        message.badges.push(Badge {
            set_id: "subscriber".into(),
            id: "12".into(),
            info: String::new(),
            image: "https://static-cdn.jtvnw.net/badges/v1/5d9f2208-5dd8-11e7-8513-2ff4adfae661/2"
                .into(),
            title: "Subscriber".into(),
        });
    }
    if h.is_multiple_of(5) {
        message.badges.push(Badge {
            set_id: "moderator".into(),
            id: "1".into(),
            info: String::new(),
            image: "https://static-cdn.jtvnw.net/badges/v1/3267646d-33f0-4b17-b3df-f923a41db1d0/2"
                .into(),
            title: "Moderator".into(),
        });
    }
    // Now and then somebody says what they do, with /me.
    if mix(n ^ 0x5bd1).is_multiple_of(17) {
        message.kind = MessageKind::Action;
        message.fragments = vec![Fragment::Text {
            text: "waves at the chat".to_owned(),
        }];
    }
    Line {
        time: Timestamp::now(),
        content: LineContent::Message(message),
        flags: LineFlags::default(),
    }
}

fn message(id: u64, sender: User, text: &str) -> ChatMessage {
    ChatMessage {
        id: format!("demo-{id}").into(),
        sender,
        color: None,
        badges: Vec::new(),
        fragments: fragments(text),
        kind: MessageKind::Text,
        bits: None,
        reply: None,
        shared_source: None,
    }
}

/// Splits text into fragments the way Twitch would: emote codes and
/// `@mentions` become their own fragments, the rest stays text.
fn fragments(text: &str) -> Vec<Fragment> {
    let mut out = Vec::new();
    let mut pending = String::new();
    for (i, word) in text.split(' ').enumerate() {
        if i > 0 {
            pending.push(' ');
        }
        let special = if let Some(&(code, id)) = EMOTES.iter().find(|(code, _)| *code == word) {
            Some(Fragment::Emote {
                emote: EmoteRef {
                    provider: EmoteProvider::Twitch,
                    id: id.into(),
                    code: code.into(),
                    animated: false,
                    zero_width: false,
                },
            })
        } else {
            word.strip_prefix('@')
                .filter(|l| !l.is_empty())
                .map(|login| Fragment::Mention {
                    text: word.into(),
                    user: user(login),
                })
        };
        match special {
            Some(fragment) => {
                if !pending.is_empty() {
                    out.push(Fragment::Text {
                        text: std::mem::take(&mut pending),
                    });
                }
                out.push(fragment);
            }
            None => pending.push_str(word),
        }
    }
    if !pending.is_empty() {
        out.push(Fragment::Text { text: pending });
    }
    out
}

fn user(name: &str) -> User {
    let login = name.to_ascii_lowercase();
    User {
        id: UserId(hash(&login).to_string()),
        login: Login(login),
        display_name: name.to_owned(),
    }
}

fn channel(login: &Login) -> Channel {
    let u = user(login.as_str());
    Channel {
        id: u.id,
        login: u.login,
        display_name: u.display_name,
    }
}

fn target(user_target: &UserTarget) -> (UserId, String) {
    match user_target {
        UserTarget::Id(id) => (id.clone(), format!("user {id}")),
        UserTarget::Login(login) => (user(login.as_str()).id, login.to_string()),
    }
}

fn reason_suffix(reason: &Option<String>) -> String {
    reason
        .as_ref()
        .map(|r| format!(": {r}"))
        .unwrap_or_default()
}

/// SplitMix64 step: turns any input into well-spread pseudo-random bits.
fn mix(x: u64) -> u64 {
    let mut z = x.wrapping_add(0x9e3779b97f4a7c15);
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d049bb133111eb);
    z ^ (z >> 31)
}

/// FNV-1a, truncated to keep IDs short. Stable across runs, unlike std's
/// hasher.
fn hash(s: &str) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in s.bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x100000001b3);
    }
    h % 1_000_000_000
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fragments_split_emotes_and_mentions() {
        let f = fragments("hi @Bob look Kappa Kappa end");
        let texts: Vec<_> = f.iter().map(Fragment::text).collect();
        assert_eq!(
            texts,
            ["hi ", "@Bob", " look ", "Kappa", " ", "Kappa", " end"]
        );
        assert!(matches!(&f[1], Fragment::Mention { user, .. } if user.login.as_str() == "bob"));
        assert!(matches!(&f[3], Fragment::Emote { emote } if emote.id == "25"));
    }

    #[test]
    fn plain_text_is_one_fragment() {
        assert_eq!(
            fragments("just words"),
            vec![Fragment::Text {
                text: "just words".into()
            }]
        );
    }

    #[test]
    fn ids_are_stable() {
        assert_eq!(user("Alice").id, user("alice").id);
        assert_ne!(user("alice").id, user("bob").id);
    }
}
