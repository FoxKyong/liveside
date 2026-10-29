//! The engine task: single owner of all chat state.

use std::collections::{BTreeSet, HashMap, VecDeque};

use liveside_proto::event::{DataFlow, DataFlowKind};
use liveside_proto::ids::Login;
use liveside_proto::model::{Channel, Fragment, Line, LineContent, NoticeLevel, RoomModes};
use liveside_proto::{
    Account, ChannelState, Command, CommandError, CommandResult, ConnectionState, ErrorKind, Event,
    Seq, Snapshot, UserId,
};
use tokio::sync::{broadcast, mpsc, oneshot};
use tokio::task::JoinHandle;

use crate::backend::{BackendIo, BackendMessage, BackendRequest, Request, UserTarget};
use crate::history::{History, HistoryConfig};
use crate::input;

#[derive(Debug, Clone)]
pub struct EngineConfig {
    /// Lines kept per channel. Older lines are dropped.
    pub scrollback: usize,
    /// Events buffered per attached frontend. A frontend that falls further
    /// behind is resynchronized with a fresh snapshot.
    pub event_buffer: usize,
    /// The local chat log, and the history restored from it on join.
    pub history: HistoryConfig,
    /// Words that highlight a message besides the account's own name.
    pub highlight_words: Vec<String>,
    /// The highlight, ignore and filter rules.
    pub rules: crate::rules::Rules,
    /// The categories rules and `/join [category]` refer to.
    pub addressbook: crate::addressbook::AddressBook,
    /// When a message counts as repeated.
    pub repeated: crate::repeated::Repeated,
    /// Where the VirusTotal key is kept, when checking links with
    /// VirusTotal is switched on; `None` when it is off.
    pub virustotal: Option<std::path::PathBuf>,
    /// Whether links may be followed from this machine to say where they
    /// lead, which tells every host on the way the reader's address.
    pub follow_links: bool,
    /// The user's own bots.
    pub bots: Vec<liveside_proto::model::BotConfig>,
    /// Where running bots hold their locks, so one runs once per machine.
    pub bot_locks: std::path::PathBuf,
    /// Where the open channels are written down, to be joined again at the
    /// next start (`crate::open_channels`); `None` when that is off.
    pub open_channels: Option<std::path::PathBuf>,
    /// The channels from the settings file. With `open_channels` they are
    /// opened first, ahead of the rest of the list, which is the order a
    /// frontend lists them in; otherwise a frontend joins them itself.
    pub channels: Vec<String>,
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self {
            scrollback: 1000,
            event_buffer: 4096,
            history: HistoryConfig::default(),
            highlight_words: Vec::new(),
            rules: crate::rules::Rules::default(),
            addressbook: Default::default(),
            repeated: Default::default(),
            virustotal: None,
            follow_links: false,
            bots: Vec::new(),
            bot_locks: crate::bots::default_lock_dir(),
            open_channels: None,
            channels: Vec::new(),
        }
    }
}

/// How many highlighted and how many ignored lines are kept for the
/// lists of them, each.
const COLLECTED: usize = 500;

/// How often a channel hears how many messages the ignore rules hid.
const HIDDEN_REPORT: std::time::Duration = std::time::Duration::from_secs(30);

/// The code of that line, which no rule looks at and no log keeps.
const HIDDEN_CODE: &str = "ignore.hidden";

/// A line a bot said, waiting to come back as the account's.
struct BotSaid {
    bot: String,
    /// The channel's login.
    channel: String,
    text: String,
    at: std::time::Instant,
}

/// How often a bot held by another Liveside is tried again.
const BOT_TAKEOVER: std::time::Duration = std::time::Duration::from_secs(5);

/// How many lines of a bot's log `/bot log` shows.
const BOT_LOG_SHOWN: usize = 10;

/// How long a line a bot said is waited for to come back as the
/// account's, to keep it from the bots.
const BOT_ECHO: std::time::Duration = std::time::Duration::from_secs(30);

/// How far back the engine looks for a message it has already shown.
/// Generous: restoring history can bring back dozens of lines at once.
const DEDUPE_WINDOW: usize = 200;

/// An event with its position in the engine's event stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sequenced {
    pub seq: Seq,
    pub event: Event,
}

/// What a frontend gets when attaching: the state as of `seq`, and a receiver
/// for every event after it. Taken atomically, so nothing is missed.
#[derive(Debug)]
pub struct Attachment {
    pub seq: Seq,
    pub snapshot: Snapshot,
    pub events: broadcast::Receiver<Sequenced>,
}

/// The outcome of a command, and the engine's sequence number when it
/// completed: every event the command caused has a sequence number at or
/// below `seq`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Completion {
    pub result: CommandResult,
    pub seq: Seq,
}

/// The engine has stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EngineStopped;

impl std::fmt::Display for EngineStopped {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("engine stopped")
    }
}

impl std::error::Error for EngineStopped {}

enum Input {
    Attach(oneshot::Sender<Attachment>),
    Command {
        command: Command,
        reply: oneshot::Sender<Completion>,
    },
}

/// Cheap, cloneable access to a running engine.
#[derive(Debug, Clone)]
pub struct EngineHandle {
    inputs: mpsc::Sender<Input>,
}

impl std::fmt::Debug for Input {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Input::Attach(_) => f.write_str("Attach"),
            Input::Command { command, .. } => write!(f, "Command({command:?})"),
        }
    }
}

impl EngineHandle {
    pub async fn attach(&self) -> Result<Attachment, EngineStopped> {
        let (reply, rx) = oneshot::channel();
        self.inputs
            .send(Input::Attach(reply))
            .await
            .map_err(|_| EngineStopped)?;
        rx.await.map_err(|_| EngineStopped)
    }

    /// Submits a command. The returned receiver yields its result; awaiting
    /// it is optional.
    pub async fn command(
        &self,
        command: Command,
    ) -> Result<oneshot::Receiver<Completion>, EngineStopped> {
        let (reply, rx) = oneshot::channel();
        self.inputs
            .send(Input::Command { command, reply })
            .await
            .map_err(|_| EngineStopped)?;
        Ok(rx)
    }

    /// Submits a command and waits for its result.
    pub async fn run(&self, command: Command) -> Result<CommandResult, EngineStopped> {
        let completion = self.command(command).await?.await;
        completion.map(|c| c.result).map_err(|_| EngineStopped)
    }
}

/// Starts the engine on the current Tokio runtime. `spawn_backend` receives
/// the backend's ends of the channels and must start the backend task.
///
/// The engine runs until a `Shutdown` command, or until every
/// [`EngineHandle`] is dropped. The returned join handle completes then.
pub fn start(
    config: EngineConfig,
    spawn_backend: impl FnOnce(BackendIo),
) -> (EngineHandle, JoinHandle<()>) {
    let (inputs_tx, inputs) = mpsc::channel(256);
    let (requests_tx, requests) = mpsc::unbounded_channel();
    let (backend_events_tx, backend_events) = mpsc::channel(1024);
    let (broadcast, _) = broadcast::channel(config.event_buffer);
    let (link_answers_tx, link_answers) = mpsc::channel(64);
    let (bot_output_tx, bot_output) = mpsc::unbounded_channel();

    spawn_backend(BackendIo {
        requests,
        events: backend_events_tx,
    });

    let engine = Engine {
        bots: crate::bots::Bots::new(config.bots.clone(), config.bot_locks.clone(), bot_output_tx),
        bot_said: VecDeque::new(),
        history: History::new(config.history.clone()),
        history_broken: false,
        open_channels_broken: false,
        backend_flows: Vec::new(),
        backend_name: String::new(),
        config,
        seq: Seq(0),
        broadcast,
        backend: requests_tx,
        account: None,
        connection: ConnectionState::Disconnected,
        channels: Vec::new(),
        whispers: std::collections::BTreeMap::new(),
        pending: HashMap::new(),
        next_token: 0,
        link_reports: HashMap::new(),
        highlighted: VecDeque::new(),
        ignored: VecDeque::new(),
        follower: None,
        link_jobs: None,
        link_answers: link_answers_tx,
    };
    let task = tokio::spawn(engine.run(inputs, backend_events, link_answers, bot_output));
    (EngineHandle { inputs: inputs_tx }, task)
}

struct ChannelData {
    channel: Channel,
    modes: RoomModes,
    lines: VecDeque<Line>,
    /// Who is in the channel, when the backend tracks membership. A set,
    /// because JOIN can arrive twice and in any order against PART.
    chatters: BTreeSet<Login>,
    /// Whether it is streaming right now.
    live: bool,
    /// Messages AutoMod or a blocked term caught, oldest first.
    held: Vec<liveside_proto::event::HeldMessage>,
    /// People asking to be let back in, oldest first.
    unban_requests: Vec<liveside_proto::event::UnbanRequest>,
    /// The poll running right now, if one is.
    poll: Option<liveside_proto::model::Poll>,
    /// And the prediction, which outlives its betting window: it stays
    /// until somebody says who won.
    prediction: Option<liveside_proto::model::Prediction>,
    /// Redemptions waiting to be answered, oldest first.
    redemptions: Vec<liveside_proto::model::Redemption>,
    /// Whether Shield Mode is on, so a menu can be a toggle rather than
    /// two items that guess.
    shield: bool,
    /// How many watch and since when, while it streams.
    stats: Option<liveside_proto::model::StreamStats>,
    /// Suspicions that arrived before the message they are about.
    pending_suspicion: Vec<(liveside_proto::MessageId, liveside_proto::model::Suspicion)>,
    /// Who has written here this session, for `config:firstmsg`.
    writers: std::collections::HashSet<UserId>,
    /// Messages the ignore rules hid since the channel was last told.
    hidden: usize,
    /// What people said lately, to tell when they repeat themselves.
    said: crate::repeated::Memory,
    /// How many watched this stream, minute by minute.
    viewers: liveside_proto::model::ViewerHistory,
}

struct Engine {
    config: EngineConfig,
    /// Whispers, one thread per person, bounded like a channel's lines.
    whispers: std::collections::BTreeMap<UserId, liveside_proto::WhisperThread>,
    /// The local chat log: the only source of history, and its only sink.
    history: History,
    /// Set after a failed write, so the warning is not repeated per line.
    history_broken: bool,
    /// Likewise for the list of open channels.
    open_channels_broken: bool,
    /// What the backend told us it does with the user's data.
    backend_flows: Vec<DataFlow>,
    /// What the backend calls itself.
    backend_name: String,
    seq: Seq,
    broadcast: broadcast::Sender<Sequenced>,
    backend: mpsc::UnboundedSender<Request>,
    account: Option<Account>,
    connection: ConnectionState,
    /// In join order.
    channels: Vec<ChannelData>,
    /// Commands waiting for backend results, by request token.
    pending: HashMap<u64, Pending>,
    next_token: u64,
    /// What has been found out about links this session, by address and
    /// by who found it out.
    link_reports:
        HashMap<(String, liveside_proto::model::LinkSource), liveside_proto::model::LinkReport>,
    /// Follows redirects, made with the first link to follow.
    follower: Option<std::sync::Arc<crate::redirects::Follower>>,
    /// The VirusTotal worker, started with the first question.
    link_jobs: Option<mpsc::UnboundedSender<crate::virustotal::Job>>,
    /// Where the worker sends its answers.
    link_answers: mpsc::Sender<liveside_proto::model::LinkReport>,
    /// What lit up and what was hidden this session, the newest last.
    highlighted: VecDeque<liveside_proto::model::CollectedLine>,
    ignored: VecDeque<liveside_proto::model::CollectedLine>,
    /// The user's own bots.
    bots: crate::bots::Bots,
    /// What bots said lately, by channel login, to know their lines when
    /// they come back as the account's: a bot must not hear them.
    bot_said: VecDeque<BotSaid>,
}

/// A command whose backend requests are not all done yet.
struct Pending {
    reply: oneshot::Sender<Completion>,
    /// Backend requests still outstanding; the reply goes out at zero.
    remaining: usize,
    /// The first failure; the command fails if any request fails.
    error: Option<CommandError>,
    /// The channel this was asked to join, if it was: a frontend showing
    /// the name while it waits has to be told when it will never come.
    joining: Option<liveside_proto::Login>,
}

enum Flow {
    Continue,
    Stop,
}

/// Keeps a line in one of the lists of what lit up or was hidden.
fn collect(
    list: &mut VecDeque<liveside_proto::model::CollectedLine>,
    channel: Channel,
    line: Line,
) {
    list.push_back(liveside_proto::model::CollectedLine { channel, line });
    while list.len() > COLLECTED {
        list.pop_front();
    }
}

/// What the rules made of a line.
enum Judged {
    Hidden,
    Shown { filtered: bool },
}

impl Engine {
    async fn run(
        mut self,
        mut inputs: mpsc::Receiver<Input>,
        mut backend: mpsc::Receiver<BackendMessage>,
        mut link_answers: mpsc::Receiver<liveside_proto::model::LinkReport>,
        mut bot_output: mpsc::UnboundedReceiver<crate::bots::Output>,
    ) {
        let mut backend_open = true;
        for event in self.bots.autostart() {
            self.publish(event);
        }
        self.reopen_channels();
        // How often a channel is told how much the ignore rules hid: a
        // line per message would be what ignoring was meant to stop.
        let mut hidden_check =
            tokio::time::interval_at(tokio::time::Instant::now() + HIDDEN_REPORT, HIDDEN_REPORT);
        // How often a bot another Liveside runs is looked at again, to take
        // it over once that one lets it go.
        let mut bot_takeover =
            tokio::time::interval_at(tokio::time::Instant::now() + BOT_TAKEOVER, BOT_TAKEOVER);
        loop {
            tokio::select! {
                _ = hidden_check.tick() => self.say_hidden(),
                _ = bot_takeover.tick() => {
                    for event in self.bots.take_over() {
                        self.publish(event);
                    }
                }
                input = inputs.recv() => match input {
                    Some(Input::Attach(reply)) => {
                        let _ = reply.send(self.attach());
                    }
                    Some(Input::Command { command, reply }) => {
                        if let Flow::Stop = self.command(command, reply) {
                            break;
                        }
                    }
                    // Every handle is gone: nobody can observe us anymore.
                    None => break,
                },
                Some(report) = link_answers.recv() => self.link_report(report),
                Some(output) = bot_output.recv() => self.bot_output(output),
                message = backend.recv(), if backend_open => match message {
                    Some(BackendMessage::Event(event)) => self.apply(event),
                    Some(BackendMessage::Done { token, result }) => self.done(token, result),
                    None => {
                        backend_open = false;
                        for (_, pending) in self.pending.drain() {
                            let result = Err(backend_gone());
                            let _ = pending.reply.send(Completion { result, seq: self.seq });
                        }
                    }
                },
            }
        }
        // Their input ends with the engine; they have a moment to finish.
        self.bots.shutdown().await;
    }

    fn attach(&self) -> Attachment {
        Attachment {
            seq: self.seq,
            snapshot: Snapshot {
                account: self.account.clone(),
                connection: self.connection.clone(),
                channels: self
                    .channels
                    .iter()
                    .map(|c| ChannelState {
                        channel: c.channel.clone(),
                        modes: c.modes.clone(),
                        lines: c.lines.iter().cloned().collect(),
                        chatters: c.chatters.iter().cloned().collect(),
                        live: c.live,
                        held: c.held.clone(),
                        unban_requests: c.unban_requests.clone(),
                        poll: c.poll.clone(),
                        prediction: c.prediction.clone(),
                        redemptions: c.redemptions.clone(),
                        shield: c.shield,
                        stats: c.stats,
                        viewers: c.viewers.clone(),
                    })
                    .collect(),
                backend: self.backend_name.clone(),
                data_flows: self.data_flows(),
                whispers: self.whispers.values().cloned().collect(),
                link_reports: self.link_reports.values().cloned().collect(),
                highlighted: self.highlighted.iter().cloned().collect(),
                ignored: self.ignored.iter().cloned().collect(),
                bots: self.bots.statuses(),
            },
            events: self.broadcast.subscribe(),
        }
    }

    fn command(&mut self, command: Command, reply: oneshot::Sender<Completion>) -> Flow {
        let requests = match command {
            Command::Shutdown => {
                self.reply(reply, Ok(()));
                return Flow::Stop;
            }
            Command::CheckLink { url, scan } => {
                self.check_link(url, scan);
                self.reply(reply, Ok(()));
                return Flow::Continue;
            }
            Command::FollowLink { url } => {
                self.follow_link(url);
                self.reply(reply, Ok(()));
                return Flow::Continue;
            }
            Command::StartBot { name } => {
                let result = self.bots.start(&name).map(|events| {
                    for event in events {
                        self.publish(event);
                    }
                });
                self.reply(reply, result);
                return Flow::Continue;
            }
            Command::StopBot { name } => {
                let result = self.bots.stop(&name).map(|events| {
                    for event in events {
                        self.publish(event);
                    }
                });
                self.reply(reply, result);
                return Flow::Continue;
            }
            Command::SetBots { bots } => {
                let result = self.bots.set(bots).map(|event| self.publish(event));
                self.reply(reply, result);
                return Flow::Continue;
            }
            Command::SetRules {
                highlight_words,
                highlight,
                ignore,
                filter,
                addressbook,
                repeated,
            } => {
                let repeated = match crate::repeated::Repeated::read(&repeated) {
                    Ok(repeated) => repeated,
                    Err(e) => {
                        let args: Vec<(&str, &str)> =
                            e.args.iter().map(|(k, v)| (*k, v.as_str())).collect();
                        return self.fail(
                            reply,
                            CommandError::new(ErrorKind::InvalidArgument, e.code, &args, e.message),
                        );
                    }
                };
                let book = match crate::addressbook::AddressBook::from_map(addressbook) {
                    Ok(book) => book,
                    Err(e) => {
                        let args: Vec<(&str, &str)> =
                            e.args.iter().map(|(k, v)| (*k, v.as_str())).collect();
                        return self.fail(
                            reply,
                            CommandError::new(ErrorKind::InvalidArgument, e.code, &args, e.message),
                        );
                    }
                };
                let result = match crate::rules::Rules::read(&highlight, &ignore, &filter) {
                    Ok(rules) => {
                        self.config.highlight_words = highlight_words;
                        self.config.rules = rules;
                        self.config.addressbook = book;
                        self.config.repeated = repeated;
                        Ok(())
                    }
                    Err(errors) => {
                        let first = &errors[0];
                        Err(CommandError::new(
                            ErrorKind::InvalidArgument,
                            "rules.invalid",
                            &[
                                ("list", first.list),
                                ("number", &first.number.to_string()),
                                ("rule", &first.rule),
                                ("error", &first.error.message),
                            ],
                            first.to_string(),
                        ))
                    }
                };
                self.reply(reply, result);
                return Flow::Continue;
            }
            Command::Login => vec![BackendRequest::Login],
            Command::Logout => vec![BackendRequest::Logout],
            // One channel, or several, or `[category]` from the address
            // book.
            Command::Join { channel } => {
                match self.joining(&liveside_proto::join_targets(channel.as_str())) {
                    Ok(requests) => requests,
                    Err(e) => return self.fail(reply, e),
                }
            }
            Command::Part { channel } => {
                if self.channel(&channel).is_none() {
                    return self.fail(reply, not_joined(&channel));
                }
                vec![BackendRequest::Part { channel }]
            }
            Command::Input {
                channel,
                text,
                reply_to,
            } => {
                if self.channel(&channel).is_none() {
                    return self.fail(reply, not_joined(&channel));
                }
                if let Some(args) = input::bot_args(&text) {
                    let result = self.bot_command(&channel, args);
                    self.reply(reply, result);
                    return Flow::Continue;
                }
                // Joining is answered here rather than by the parser,
                // which does not know the address book.
                if let Some(args) = input::join_args(&text) {
                    match self.joining(&liveside_proto::join_targets(args)) {
                        Ok(requests) => requests,
                        Err(e) => return self.fail(reply, e),
                    }
                } else {
                    match input::parse(&channel, &text, reply_to) {
                        Ok(requests) => requests,
                        Err(e) => return self.fail(reply, e),
                    }
                }
            }
            Command::SendMessage {
                channel,
                text,
                reply_to,
            } => vec![BackendRequest::Send {
                channel,
                text,
                reply_to,
            }],
            Command::DeleteMessage { channel, message } => {
                vec![BackendRequest::Delete { channel, message }]
            }
            Command::Timeout {
                channel,
                user,
                seconds,
                reason,
            } => vec![BackendRequest::Timeout {
                channel,
                user: UserTarget::Id(user),
                seconds,
                reason,
            }],
            Command::Ban {
                channel,
                user,
                reason,
            } => vec![BackendRequest::Ban {
                channel,
                user: UserTarget::Id(user),
                reason,
            }],
            Command::Unban { channel, user } => vec![BackendRequest::Unban {
                channel,
                user: UserTarget::Id(user),
            }],
            Command::CreatePoll {
                channel,
                title,
                choices,
                seconds,
            } => vec![BackendRequest::CreatePoll {
                channel,
                title,
                choices,
                seconds,
            }],
            Command::EndPoll { channel, archive } => {
                vec![BackendRequest::EndPoll { channel, archive }]
            }
            Command::CreatePrediction {
                channel,
                title,
                outcomes,
                seconds,
            } => vec![BackendRequest::CreatePrediction {
                channel,
                title,
                outcomes,
                seconds,
            }],
            Command::EndPrediction { channel, how } => {
                vec![BackendRequest::EndPrediction { channel, how }]
            }
            Command::ShowChannelInfo { channel } => {
                vec![BackendRequest::ShowChannelInfo { channel }]
            }
            Command::SetChannelInfo {
                channel,
                title,
                game,
                language,
                tags,
                delay,
                labels,
            } => vec![BackendRequest::SetChannelInfo {
                channel,
                title,
                game,
                language,
                tags,
                delay,
                labels,
            }],
            Command::ShowSchedule { channel } => vec![BackendRequest::ShowSchedule { channel }],
            Command::AddSegment {
                channel,
                start,
                minutes,
                title,
                weekly,
            } => vec![BackendRequest::AddSegment {
                channel,
                start,
                minutes,
                title,
                weekly,
            }],
            Command::CancelSegment {
                channel,
                segment,
                canceled,
            } => vec![BackendRequest::CancelSegment {
                channel,
                segment,
                canceled,
            }],
            Command::DeleteSegment { channel, segment } => {
                vec![BackendRequest::DeleteSegment { channel, segment }]
            }
            Command::SetVacation { channel, away } => {
                vec![BackendRequest::SetVacation { channel, away }]
            }
            Command::ShowAds { channel } => vec![BackendRequest::ShowAds { channel }],
            Command::SnoozeAd { channel } => vec![BackendRequest::SnoozeAd { channel }],
            Command::SendWhisper { user, text } => {
                vec![BackendRequest::SendWhisper { user, text }]
            }
            Command::ShowModeratedChannels => vec![BackendRequest::ShowModeratedChannels],
            Command::ShowFollowedChannels => vec![BackendRequest::ShowFollowedChannels],
            Command::CreateClip { channel } => vec![BackendRequest::CreateClip { channel }],
            Command::ShowRewards { channel } => vec![BackendRequest::ShowRewards { channel }],
            Command::CreateReward {
                channel,
                title,
                cost,
                prompt,
            } => vec![BackendRequest::CreateReward {
                channel,
                title,
                cost,
                prompt,
            }],
            Command::UpdateReward {
                channel,
                reward,
                title,
                cost,
                prompt,
                enabled,
                paused,
            } => vec![BackendRequest::UpdateReward {
                channel,
                reward,
                title,
                cost,
                prompt,
                enabled,
                paused,
            }],
            Command::DeleteReward { channel, reward } => {
                vec![BackendRequest::DeleteReward { channel, reward }]
            }
            Command::AnswerRedemption {
                channel,
                reward,
                redemption,
                fulfil,
            } => vec![BackendRequest::AnswerRedemption {
                channel,
                reward,
                redemption,
                fulfil,
            }],
            Command::ShowUser { channel, user } => {
                vec![BackendRequest::ShowUser { channel, user }]
            }
            Command::ResolveUnban {
                channel,
                request,
                allow,
            } => vec![BackendRequest::ResolveUnban {
                channel,
                request,
                allow,
            }],
            Command::ResolveHeld {
                channel,
                message,
                allow,
            } => vec![BackendRequest::ResolveHeld {
                channel,
                message,
                allow,
            }],
        };
        self.forward(requests, reply);
        Flow::Continue
    }

    /// Hands requests to the backend. The command completes when all are
    /// done: with the first failure, or with success.
    fn forward(&mut self, requests: Vec<BackendRequest>, reply: oneshot::Sender<Completion>) {
        let token = self.next_token;
        self.next_token += 1;
        let joining = requests.iter().find_map(|request| match request {
            BackendRequest::Join { channel } => Some(channel.clone()),
            _ => None,
        });
        let remaining = requests.len();
        for request in requests {
            if self.backend.send(Request { token, request }).is_err() {
                self.reply(reply, Err(backend_gone()));
                return;
            }
        }
        if remaining == 0 {
            self.reply(reply, Ok(()));
            return;
        }
        let pending = Pending {
            reply,
            remaining,
            error: None,
            joining,
        };
        self.pending.insert(token, pending);
    }

    /// Records one backend result; completes the command after the last.
    fn done(&mut self, token: u64, result: CommandResult) {
        let Some(pending) = self.pending.get_mut(&token) else {
            // A backend bug: it answered twice, or invented a token.
            self.publish(log(
                NoticeLevel::Error,
                "internal.unknown_request",
                &[("token", &token.to_string())],
                format!("Internal: result for unknown request {token}"),
            ));
            return;
        };
        pending.remaining -= 1;
        if let Err(e) = result {
            pending.error.get_or_insert(e);
        }
        if pending.remaining == 0 {
            let pending = self.pending.remove(&token).expect("present above");
            if let (Some(login), Some(_)) = (&pending.joining, &pending.error) {
                self.publish(Event::ChannelJoinFailed {
                    login: login.clone(),
                });
            }
            let result = pending.error.map_or(Ok(()), Err);
            self.reply(pending.reply, result);
        }
    }

    fn reply(&self, reply: oneshot::Sender<Completion>, result: CommandResult) {
        let _ = reply.send(Completion {
            result,
            seq: self.seq,
        });
    }

    fn fail(&self, reply: oneshot::Sender<Completion>, error: CommandError) -> Flow {
        self.reply(reply, Err(error));
        Flow::Continue
    }

    /// Applies a backend event to the state and publishes it.
    fn apply(&mut self, mut event: Event) {
        let scrollback = self.config.scrollback;
        // Annotate before anything stores or sees the line.
        if let Event::Whisper { line, .. } = &mut event {
            crate::links::mark_line(line);
        }
        // The line as it came, when a filter changed it: the log keeps
        // what was said, and the rules apply again when it is read back.
        let mut as_said: Option<Line> = None;
        // History a backend sends goes through the rules as well; the
        // local log's is judged where it is read.
        if let Event::History { channel, lines } = &mut event {
            let channel = channel.clone();
            lines.retain_mut(|line| matches!(self.judge(&channel, line), Judged::Shown { .. }));
        }
        if let Event::Line { channel, line } = &mut event {
            crate::links::mark_line(line);
            // A suspicion that arrived before its message waits for it;
            // Twitch sends the two separately and in no fixed order.
            // Attached to the event, so frontends need no bookkeeping of
            // their own for this half of it.
            if let LineContent::Message(message) = &line.content {
                let id = message.id.clone();
                if let Some(c) = self.channel_mut(channel)
                    && let Some(i) = c.pending_suspicion.iter().position(|(kept, _)| kept == &id)
                {
                    let (_, suspicion) = c.pending_suspicion.remove(i);
                    line.flags.suspicion = Some(Box::new(suspicion));
                }
            }
            let before = line.clone();
            match self.judge(channel, line) {
                Judged::Hidden => {
                    // Not shown, and not kept here, but logged: the log is
                    // what was said, and the rules may change. And kept in
                    // the list of what was hidden, for whoever wants to
                    // look.
                    let channel = channel.clone();
                    let Some(c) = self.channel_mut(&channel) else {
                        return;
                    };
                    c.hidden += 1;
                    let (login, about) = (c.channel.login.clone(), c.channel.clone());
                    // Only to the bots that hear the people it hides.
                    self.tell_bots(&channel, &before, true);
                    if !before.flags.historical {
                        self.record(&login, &before);
                        collect(&mut self.ignored, about, line.clone());
                        self.publish(Event::Ignored {
                            channel,
                            line: line.clone(),
                        });
                    }
                    return;
                }
                Judged::Shown { filtered: true } => as_said = Some(before),
                Judged::Shown { filtered: false } => {}
            }
        }
        match &event {
            Event::LoginPending { .. } | Event::LoginFailed { .. } | Event::Log { .. } => {}
            // The engine's own, never the backend's; kept where it is made.
            Event::LinkReport { .. } => {}
            // The engine's own as well.
            Event::Bots { .. } | Event::BotState { .. } | Event::BotLog { .. } => {}
            Event::Backend { name } => self.backend_name = name.clone(),
            Event::DataFlows { flows } => {
                self.backend_flows = flows.clone();
                // Publish the whole list, engine's own included.
                let flows = self.data_flows();
                self.publish(Event::DataFlows { flows });
                return;
            }
            Event::LoggedIn { account } => self.account = Some(account.clone()),
            Event::LoggedOut => self.account = None,
            Event::Connection { state } => self.connection = state.clone(),
            Event::ChannelJoined { channel, modes } => {
                let known = match self.channel_mut(&channel.id) {
                    Some(c) => {
                        c.channel = channel.clone();
                        c.modes = modes.clone();
                        true
                    }
                    None => {
                        self.channels.push(ChannelData {
                            channel: channel.clone(),
                            modes: modes.clone(),
                            lines: VecDeque::new(),
                            chatters: BTreeSet::new(),
                            live: false,
                            held: Vec::new(),
                            unban_requests: Vec::new(),
                            poll: None,
                            prediction: None,
                            redemptions: Vec::new(),
                            shield: false,
                            stats: None,
                            pending_suspicion: Vec::new(),
                            writers: std::collections::HashSet::new(),
                            hidden: 0,
                            said: Default::default(),
                            viewers: Default::default(),
                        });
                        false
                    }
                };
                // Restoring after a rejoin would duplicate what is on
                // screen, so only a channel new to this session gets it.
                if !known {
                    let restored = self.restore(channel.clone());
                    // The join must be published before what follows it.
                    self.publish(event);
                    if let Some(history) = restored {
                        self.publish(history);
                    }
                    self.remember_open_channels();
                    return;
                }
            }
            Event::ChannelParted { channel } => {
                self.channels.retain(|c| &c.channel.id != channel);
                self.remember_open_channels();
            }
            Event::ChatterJoined { channel, login } => {
                let Some(c) = self.channel_mut(channel) else {
                    return;
                };
                c.chatters.insert(login.clone());
            }
            Event::ChatterParted { channel, login } => {
                let Some(c) = self.channel_mut(channel) else {
                    return;
                };
                c.chatters.remove(login);
            }
            Event::MessageHeld { channel, held } => {
                if let Some(c) = self.channels.iter_mut().find(|c| &c.channel.id == channel) {
                    // The same message can be reported twice if a session
                    // is re-established; hold it once.
                    if !c.held.iter().any(|other| other.id == held.id) {
                        c.held.push(held.clone());
                    }
                }
            }
            Event::HeldResolved {
                channel, message, ..
            } => {
                if let Some(c) = self.channels.iter_mut().find(|c| &c.channel.id == channel) {
                    c.held.retain(|held| &held.id != message);
                }
            }
            // Only the frontends care: the engine has nothing to keep
            // about a channel that is not there yet.
            Event::ChannelJoining { .. } | Event::ChannelJoinFailed { .. } => {}
            // Asked for and answered once; the engine keeps nothing.
            Event::ShieldMode { channel, on } => {
                if let Some(state) = self.channel_mut(channel) {
                    state.shield = *on;
                }
            }
            // A whisper belongs to the person, not to a channel.
            Event::Whisper { user, line } => {
                let scrollback = self.config.scrollback;
                let thread = self.whispers.entry(user.id.clone()).or_insert_with(|| {
                    liveside_proto::WhisperThread {
                        user: user.clone(),
                        lines: Vec::new(),
                    }
                });
                thread.user = user.clone();
                thread.lines.push(line.clone());
                if thread.lines.len() > scrollback {
                    let extra = thread.lines.len() - scrollback;
                    thread.lines.drain(..extra);
                }
            }
            // Kept where it is made, in the hidden branch above.
            Event::Ignored { .. } => {}
            Event::UserCard { .. }
            | Event::People { .. }
            | Event::BlockedTerms { .. }
            | Event::ModeratedChannels { .. }
            | Event::FollowedChannels { .. }
            | Event::FollowedStreams { .. }
            | Event::Rewards { .. }
            | Event::ChannelInfo { .. }
            | Event::Schedule { .. } => {}
            // The queue of redemptions: kept while unanswered, dropped
            // once it has been answered, here or anywhere else.
            Event::RedemptionUpdate {
                channel,
                redemption,
            } => {
                use liveside_proto::model::RedemptionStatus;
                if let Some(state) = self.channel_mut(channel) {
                    state.redemptions.retain(|kept| kept.id != redemption.id);
                    if redemption.status == RedemptionStatus::Unfulfilled {
                        state.redemptions.push(redemption.clone());
                    }
                }
            }
            // Kept until it is paid out or called off: a locked
            // prediction is precisely the one a broadcaster must not
            // lose sight of.
            Event::PredictionUpdate {
                channel,
                prediction,
            } => {
                use liveside_proto::model::PredictionStatus;
                let over = matches!(
                    prediction.status,
                    PredictionStatus::Resolved | PredictionStatus::Canceled
                );
                if let Some(state) = self.channel_mut(channel) {
                    state.prediction = (!over).then(|| prediction.clone());
                }
            }
            // A poll is kept until it ends, so that a frontend attaching
            // late sees what everyone else is looking at.
            Event::PollUpdate { channel, poll } => {
                let finished = poll.status != liveside_proto::model::PollStatus::Active;
                if let Some(state) = self.channel_mut(channel) {
                    state.poll = (!finished).then(|| poll.clone());
                }
            }
            Event::UnbanRequested { channel, request } => {
                if let Some(c) = self.channels.iter_mut().find(|c| &c.channel.id == channel)
                    && !c.unban_requests.iter().any(|other| other.id == request.id)
                {
                    c.unban_requests.push(request.clone());
                }
            }
            Event::UnbanResolved {
                channel, request, ..
            } => {
                if let Some(c) = self.channels.iter_mut().find(|c| &c.channel.id == channel) {
                    c.unban_requests.retain(|waiting| &waiting.id != request);
                }
            }
            Event::Stream { channel, live } => {
                if let Some(c) = self.channels.iter_mut().find(|c| &c.channel.id == channel) {
                    c.live = *live;
                    // A stream that went off has nobody watching it.
                    if !*live {
                        c.stats = None;
                    }
                }
            }
            Event::StreamStats { channel, stats } => {
                if let Some(c) = self.channel_mut(channel) {
                    c.stats = Some(*stats);
                    c.viewers.record(liveside_proto::Timestamp::now(), stats);
                }
            }
            Event::Chatters { channel, logins } => {
                let Some(c) = self.channel_mut(channel) else {
                    return;
                };
                c.chatters = logins.iter().cloned().collect();
            }
            Event::RoomModesChanged { channel, modes } => {
                let Some(c) = self.channel_mut(channel) else {
                    return;
                };
                c.modes = modes.clone();
            }
            Event::Line { channel, line } => {
                let Some(c) = self.channel_mut(channel) else {
                    return;
                };
                // A new title or category, as a mark on the viewer graph.
                if let LineContent::Notice { code, args, .. } = &line.content
                    && code == "event.channel_update"
                    && !line.flags.historical
                {
                    let arg = |name: &str| {
                        args.iter()
                            .find(|(k, _)| k == name)
                            .map(|(_, v)| v.as_str())
                            .unwrap_or_default()
                    };
                    c.viewers.mark(line.time, arg("title"), arg("game"));
                }
                // The same message can arrive twice: restored history can
                // overlap what is about to be received live, for instance
                // when another engine on this machine logged it a moment
                // ago. Showing it twice is worse than dropping it.
                if let LineContent::Message(message) = &line.content
                    && !message.id.as_str().is_empty()
                    && c.lines.iter().rev().take(DEDUPE_WINDOW).any(|other| {
                        matches!(&other.content, LineContent::Message(seen) if seen.id == message.id)
                    })
                {
                    return;
                }
                let login = c.channel.login.clone();
                let about = c.channel.clone();
                c.lines.push_back(line.clone());
                while c.lines.len() > scrollback {
                    c.lines.pop_front();
                }
                // What lit up now, not what lit up in history.
                if line.flags.highlighted && !line.flags.historical {
                    collect(&mut self.highlighted, about, line.clone());
                }
                // Lines restored from the log must not be written back.
                if !line.flags.historical {
                    self.record(&login, as_said.as_ref().unwrap_or(line));
                }
                // Bots hear what was said, not what a filter made of it,
                // and once, like the screen.
                self.tell_bots(channel, as_said.as_ref().unwrap_or(line), false);
            }
            Event::History { channel, lines } => {
                let Some(c) = self.channel_mut(channel) else {
                    return;
                };
                // History may repeat what the channel already shows, the
                // same way a live line may repeat history.
                let known: std::collections::HashSet<_> = c
                    .lines
                    .iter()
                    .filter_map(|line| match &line.content {
                        LineContent::Message(message) => Some(message.id.clone()),
                        _ => None,
                    })
                    .collect();
                for line in lines.iter().rev() {
                    if let LineContent::Message(message) = &line.content
                        && known.contains(&message.id)
                    {
                        continue;
                    }
                    if c.lines.len() >= scrollback {
                        break;
                    }
                    c.lines.push_front(line.clone());
                }
            }
            // The message itself arrives separately and may arrive
            // after this, so what is not found now is remembered and
            // applied when it does.
            Event::SuspiciousMessage {
                channel,
                message,
                suspicion,
                ..
            } => {
                let Some(c) = self.channel_mut(channel) else {
                    return;
                };
                let mut marked = false;
                for line in c.lines.iter_mut() {
                    if let LineContent::Message(m) = &line.content
                        && &m.id == message
                    {
                        line.flags.suspicion = Some(suspicion.clone());
                        marked = true;
                    }
                }
                if !marked {
                    c.pending_suspicion
                        .push((message.clone(), (**suspicion).clone()));
                    // A message that never arrives must not pile up.
                    if c.pending_suspicion.len() > 50 {
                        c.pending_suspicion.remove(0);
                    }
                }
            }
            Event::MessageDeleted { channel, message } => {
                let Some(c) = self.channel_mut(channel) else {
                    return;
                };
                for line in c.lines.iter_mut() {
                    if let LineContent::Message(m) = &line.content
                        && &m.id == message
                    {
                        line.flags.deleted = true;
                    }
                }
            }
            Event::UserCleared { channel, user, .. } => {
                let Some(c) = self.channel_mut(channel) else {
                    return;
                };
                for line in c.lines.iter_mut() {
                    if let LineContent::Message(m) = &line.content
                        && &m.sender.id == user
                    {
                        line.flags.deleted = true;
                    }
                }
            }
            Event::ChatCleared { channel } => {
                if self.channel(channel).is_none() {
                    return;
                }
            }
        }
        self.publish(event);
    }

    /// Everything this engine does with the user's data: what the backend
    /// reported, plus the engine's own local log.
    fn data_flows(&self) -> Vec<DataFlow> {
        let mut flows = self.backend_flows.clone();
        flows.push(DataFlow {
            party: "This machine".to_owned(),
            party_code: "flow.party.me".to_owned(),
            kind: DataFlowKind::Disk,
            detail: match self.config.history.dir.as_ref() {
                Some(dir) if self.history.is_enabled() => format!(
                    "Chat of joined channels is written to {} (0600) and \
                     restored from there when joining again",
                    dir.display()
                ),
                _ => "No chat log is written; channels start empty".to_owned(),
            },
            code: match self.config.history.dir.as_ref() {
                Some(_) if self.history.is_enabled() => "flow.history.on".to_owned(),
                _ => "flow.history.off".to_owned(),
            },
            args: match self.config.history.dir.as_ref() {
                Some(dir) if self.history.is_enabled() => {
                    vec![("dir".to_owned(), dir.display().to_string())]
                }
                _ => Vec::new(),
            },
            enabled: self.history.is_enabled(),
            switch: "--no-history".to_owned(),
        });
        let reopen = self.config.open_channels.as_ref();
        flows.push(DataFlow {
            party: "This machine".to_owned(),
            party_code: "flow.party.me".to_owned(),
            kind: DataFlowKind::Disk,
            detail: match reopen {
                Some(path) => format!(
                    "The names of the open channels are written to {} (0600) \
                     and opened again at the next start",
                    path.display()
                ),
                None => "Which channels are open is not written down; a start \
                         opens the channels in the settings"
                    .to_owned(),
            },
            code: if reopen.is_some() {
                "flow.channels.on"
            } else {
                "flow.channels.off"
            }
            .to_owned(),
            args: reopen
                .map(|path| vec![("path".to_owned(), path.display().to_string())])
                .unwrap_or_default(),
            enabled: reopen.is_some(),
            switch: "chat.reopen_channels".to_owned(),
        });
        let virustotal = self.config.virustotal.is_some();
        flows.push(DataFlow {
            party: "VirusTotal".to_owned(),
            party_code: String::new(),
            kind: DataFlowKind::Network,
            detail: if virustotal {
                "A link you ask about is sent to VirusTotal with your API key; \
                 one you have it scan becomes visible to VirusTotal's customers"
                    .to_owned()
            } else {
                "Nothing is sent; checking links is switched off".to_owned()
            },
            code: if virustotal {
                "flow.virustotal.on"
            } else {
                "flow.virustotal.off"
            }
            .to_owned(),
            args: Vec::new(),
            enabled: virustotal,
            switch: "security.virustotal".to_owned(),
        });
        let follow = self.config.follow_links;
        flows.push(DataFlow {
            party: "The sites a link leads through".to_owned(),
            party_code: "flow.party.link_hosts".to_owned(),
            kind: DataFlowKind::Network,
            detail: if follow {
                "A link you ask where it leads is followed from this machine; \
                 every site on the way sees your IP address, none is rendered"
                    .to_owned()
            } else {
                "No link is followed from this machine".to_owned()
            },
            code: if follow {
                "flow.redirects.on"
            } else {
                "flow.redirects.off"
            }
            .to_owned(),
            args: Vec::new(),
            enabled: follow,
            switch: "security.resolver".to_owned(),
        });
        flows
    }

    /// Runs a channel's line through the rules: whether it lights up and
    /// how, whether it is shown at all, and what of it is filtered out.
    /// The account's own lines are left as they are: never lit up, as
    /// before there were rules, and never hidden or filtered.
    fn judge(&mut self, channel: &UserId, line: &mut Line) -> Judged {
        // What the engine says about hiding is not for the rules to hide.
        if let LineContent::Notice { code, .. } = &line.content
            && code == HIDDEN_CODE
        {
            return Judged::Shown { filtered: false };
        }
        let sender = match &line.content {
            LineContent::Message(message) => Some(message.sender.id.clone()),
            LineContent::UserNotice {
                message: Some(message),
                ..
            } => Some(message.sender.id.clone()),
            _ => None,
        };
        let own = line.flags.own
            || self
                .account
                .as_ref()
                .is_some_and(|account| sender.as_ref() == Some(&account.user.id));
        // The channel by its field alone, so the settings can be read
        // beside it.
        let Some(c) = self.channels.iter_mut().find(|c| &c.channel.id == channel) else {
            return Judged::Shown { filtered: false };
        };
        // Only live lines count towards somebody's first: history would
        // make everyone a regular before they say anything today.
        let first = match (&sender, line.flags.historical) {
            (Some(sender), false) => c.writers.insert(sender.clone()),
            _ => false,
        };
        let mut subject = crate::rules::Subject::of(line, c.channel.login.as_str(), first, c.live)
            .with_book(&self.config.addressbook);
        let rules = &self.config.rules;
        if own {
            return Judged::Shown { filtered: false };
        }
        // Said again? Live messages only, as with a first message: history
        // read again would count everything twice.
        let repeated = &self.config.repeated;
        if repeated.is_on()
            && !line.flags.historical
            && let LineContent::Message(message) = &line.content
            && repeated
                .only
                .as_ref()
                .is_none_or(|rule| rule.matches(&subject))
        {
            let text = message.plain_text();
            if text.chars().filter(|c| !c.is_whitespace()).count() >= repeated.min_length {
                let count = c.said.said(repeated, &message.sender.id, line.time, &text);
                if count >= repeated.times {
                    line.flags.repeated = count;
                    subject.repeated = count;
                }
            }
        }
        let rule = rules.highlight.iter().find(|rule| rule.matches(&subject));
        let style = rule.map(|rule| rule.style.clone()).unwrap_or_default();
        line.flags.highlighted =
            rule.is_some() || highlights(self.account.as_ref(), &self.config.highlight_words, line);
        line.flags.highlight_color = style.color;
        line.flags.highlight_background = style.background;
        line.flags.highlight_silent = style.silent;
        line.flags.highlight_quiet = style.quiet;
        if !style.over_ignore && rules.ignore.iter().any(|rule| rule.matches(&subject)) {
            return Judged::Hidden;
        }
        // The first filter that applies, in the text people wrote; emotes,
        // mentions and links stay whole.
        let LineContent::Message(message) = &mut line.content else {
            return Judged::Shown { filtered: false };
        };
        let Some(filter) = rules.filter.iter().find(|rule| rule.matches(&subject)) else {
            return Judged::Shown { filtered: false };
        };
        let mut filtered = false;
        for fragment in &mut message.fragments {
            if let Fragment::Text { text } = fragment
                && let Some(changed) = filter.filter_text(text)
            {
                *text = changed;
                filtered = true;
            }
        }
        Judged::Shown { filtered }
    }

    /// Tells each channel how many messages the ignore rules hid since
    /// it was last told, if any.
    fn say_hidden(&mut self) {
        // Also the time to forget what people said too long ago.
        let now = liveside_proto::Timestamp::now();
        let window = self.config.repeated.window;
        for c in &mut self.channels {
            c.said.forget(now, window);
        }
        let counts: Vec<(UserId, usize)> = self
            .channels
            .iter_mut()
            .filter(|c| c.hidden > 0)
            .map(|c| (c.channel.id.clone(), std::mem::take(&mut c.hidden)))
            .collect();
        for (channel, count) in counts {
            let count = count.to_string();
            let text = format!("{count} messages hidden by the ignore rules");
            self.apply(Event::Line {
                channel,
                line: Line {
                    time: liveside_proto::Timestamp::now(),
                    content: LineContent::Notice {
                        level: NoticeLevel::Info,
                        text,
                        code: HIDDEN_CODE.to_owned(),
                        args: vec![("count".to_owned(), count)],
                    },
                    flags: liveside_proto::model::LineFlags::default(),
                },
            });
        }
    }

    /// Reads a channel's local history, as an event to publish after the
    /// join. Failure is reported to the user but never blocks the join.
    fn restore(&mut self, channel: Channel) -> Option<Event> {
        if !self.history.is_enabled() {
            return None;
        }
        match self.history.load(channel.login.as_str()) {
            Ok(lines) if lines.is_empty() => None,
            Ok(mut lines) => {
                // A log written before links were found has them as text.
                lines.iter_mut().for_each(crate::links::mark_line);
                // The rules as they are now, not as they were when it was
                // written: what is ignored today stays hidden in history.
                lines.retain_mut(|line| {
                    matches!(self.judge(&channel.id, line), Judged::Shown { .. })
                });
                if lines.is_empty() {
                    return None;
                }
                let scrollback = self.config.scrollback;
                let c = self.channel_mut(&channel.id)?;
                for line in lines.iter().rev() {
                    if c.lines.len() >= scrollback {
                        break;
                    }
                    c.lines.push_front(line.clone());
                }
                Some(Event::History {
                    channel: channel.id,
                    lines,
                })
            }
            Err(e) => {
                self.publish(log(
                    NoticeLevel::Warning,
                    "history.unreadable",
                    &[
                        ("channel", channel.login.as_str()),
                        ("error", &e.to_string()),
                    ],
                    format!("Cannot read the local history of #{}: {e}", channel.login),
                ));
                None
            }
        }
    }

    /// Joins the channels that were open when the engine last ran, as a
    /// frontend joining them would. Nobody waits for the answer: a channel
    /// that cannot be joined any more is not opened, and is left out the
    /// next time the list is written.
    fn reopen_channels(&mut self) {
        let Some(path) = &self.config.open_channels else {
            return;
        };
        // The settings file's channels first, as a frontend lists them, so
        // the list is written down again in the order it is shown in.
        let mut logins = self.config.channels.clone();
        for login in crate::open_channels::load(path) {
            if !logins.contains(&login) {
                logins.push(login);
            }
        }
        // One at a time: a name that cannot be joined, a typo in the
        // settings file, must not keep the others closed. The frontend
        // that joins it as well says what is wrong with it.
        for login in logins {
            if let Ok(requests) = self.joining(std::slice::from_ref(&login)) {
                let (reply, _) = oneshot::channel();
                self.forward(requests, reply);
            }
        }
    }

    /// Writes down which channels are open, for the next start, after a
    /// channel is joined or left; a failure is reported once.
    fn remember_open_channels(&mut self) {
        let Some(path) = &self.config.open_channels else {
            return;
        };
        if self.open_channels_broken {
            return;
        }
        let logins: Vec<&str> = self
            .channels
            .iter()
            .map(|c| c.channel.login.as_str())
            .collect();
        if let Err(e) = crate::open_channels::save(path, &logins) {
            self.open_channels_broken = true;
            self.publish(log(
                NoticeLevel::Warning,
                "open_channels.write_failed",
                &[("error", &e.to_string())],
                format!("Cannot write down which channels are open, stopping: {e}"),
            ));
        }
    }

    /// Appends a line to the local log, reporting a failure once: a full or
    /// read-only disk would otherwise repeat on every message.
    fn record(&mut self, login: &Login, line: &Line) {
        if !self.history.is_enabled() || self.history_broken {
            return;
        }
        // What went wrong is not chat. A night of "Invalid OAuth token"
        // belongs nowhere, and least of all in a log that is read back
        // into the channel every time the client starts.
        if let LineContent::Notice {
            level: NoticeLevel::Error,
            ..
        } = &line.content
        {
            return;
        }
        // Nor how much was hidden, nor what the bots are doing: that is
        // about this session's screen.
        if let LineContent::Notice { code, .. } = &line.content
            && (code == HIDDEN_CODE || code.starts_with("bot."))
        {
            return;
        }
        if let Err(e) = self.history.append(login.as_str(), line) {
            self.history_broken = true;
            self.publish(log(
                NoticeLevel::Warning,
                "history.write_failed",
                &[("error", &e.to_string())],
                format!("Cannot write the local chat log, stopping: {e}"),
            ));
        }
    }

    /// Asks VirusTotal about a link, or says again what it already said.
    fn check_link(&mut self, url: String, scan: bool) {
        use liveside_proto::model::{LinkReport, LinkSource, LinkState};
        let source = LinkSource::VirusTotal;
        let Some(key_path) = self.config.virustotal.clone() else {
            self.link_report(LinkReport {
                url,
                source,
                state: LinkState::Failed {
                    code: "link.off".to_owned(),
                    detail: "Checking links with VirusTotal is switched off in the settings"
                        .to_owned(),
                },
            });
            return;
        };
        let key = (url.clone(), source);
        let known = self.link_reports.get(&key).map(|report| &report.state);
        match known {
            // Under way already; asking twice would only spend the quota.
            Some(LinkState::Checking | LinkState::Scanning) => return,
            // An answer stands for the session, unless the reader now asks
            // for the scan that a lookup did not do.
            Some(state @ (LinkState::Found { .. } | LinkState::Unknown))
                if !(scan && *state == LinkState::Unknown) =>
            {
                let report = self.link_reports[&key].clone();
                self.publish(Event::LinkReport { report });
                return;
            }
            _ => {}
        }
        let answers = self.link_answers.clone();
        let jobs = self
            .link_jobs
            .get_or_insert_with(|| crate::virustotal::spawn(key_path, answers));
        let job = crate::virustotal::Job {
            url: url.clone(),
            scan,
        };
        if jobs.send(job).is_err() {
            self.link_jobs = None;
            return;
        }
        self.link_report(LinkReport {
            url,
            source,
            state: LinkState::Checking,
        });
    }

    /// Follows a link's redirects from this machine, or says again where
    /// it was found to lead.
    fn follow_link(&mut self, url: String) {
        use liveside_proto::model::{LinkReport, LinkSource, LinkState};
        let source = LinkSource::Redirects;
        if !self.config.follow_links {
            self.link_report(LinkReport {
                url,
                source,
                state: LinkState::Failed {
                    code: "link.follow_off".to_owned(),
                    detail: "Following links is switched off in the settings".to_owned(),
                },
            });
            return;
        }
        let key = (url.clone(), source);
        match self.link_reports.get(&key).map(|report| &report.state) {
            Some(LinkState::Checking) => return,
            Some(LinkState::Leads { .. }) => {
                let report = self.link_reports[&key].clone();
                self.publish(Event::LinkReport { report });
                return;
            }
            // A failure may have been the network; asking again is free.
            _ => {}
        }
        let follower = self.follower.get_or_insert_with(Default::default).clone();
        let answers = self.link_answers.clone();
        let asked = url.clone();
        tokio::spawn(async move {
            let state = follower.follow(&asked).await;
            let _ = answers
                .send(LinkReport {
                    url: asked,
                    source,
                    state,
                })
                .await;
        });
        self.link_report(LinkReport {
            url,
            source,
            state: LinkState::Checking,
        });
    }

    /// Keeps an answer about a link and tells every frontend.
    fn link_report(&mut self, report: liveside_proto::model::LinkReport) {
        self.link_reports
            .insert((report.url.clone(), report.source), report.clone());
        self.publish(Event::LinkReport { report });
    }

    /// `/bots`, and `/bot start`, `stop` or `log` with a name: what they
    /// say goes to the channel they were typed in.
    fn bot_command(&mut self, channel: &UserId, args: &str) -> CommandResult {
        let (verb, name) = args.split_once(char::is_whitespace).unwrap_or((args, ""));
        let name = name.trim();
        match (verb.to_ascii_lowercase().as_str(), name) {
            ("list", _) => {
                let statuses = self.bots.statuses();
                if statuses.is_empty() {
                    self.bot_notice(channel, "bot.none", &[], "No bots; see docs/bots.md".into());
                }
                for status in statuses {
                    let bot = status.config.name.as_str();
                    let (code, args, text) = match &status.state {
                        liveside_proto::model::BotState::Running => (
                            "bot.is_running",
                            vec![("bot", bot)],
                            format!("{bot}: running"),
                        ),
                        liveside_proto::model::BotState::Stopped => (
                            "bot.is_stopped",
                            vec![("bot", bot)],
                            format!("{bot}: stopped"),
                        ),
                        liveside_proto::model::BotState::Ended { code: Some(0) } => (
                            "bot.is_finished",
                            vec![("bot", bot)],
                            format!("{bot}: finished"),
                        ),
                        liveside_proto::model::BotState::Ended { .. } => (
                            "bot.is_crashed",
                            vec![("bot", bot)],
                            format!("{bot}: crashed"),
                        ),
                        liveside_proto::model::BotState::Failed { .. } => (
                            "bot.is_failed",
                            vec![("bot", bot)],
                            format!("{bot}: cannot start"),
                        ),
                        liveside_proto::model::BotState::Elsewhere => (
                            "bot.is_elsewhere",
                            vec![("bot", bot)],
                            format!("{bot}: running in another Liveside"),
                        ),
                    };
                    self.bot_notice(channel, code, &args, text);
                }
                Ok(())
            }
            ("start", name) if !name.is_empty() => {
                for event in self.bots.start(name)? {
                    self.publish(event);
                }
                Ok(())
            }
            ("stop", name) if !name.is_empty() => {
                for event in self.bots.stop(name)? {
                    self.publish(event);
                }
                Ok(())
            }
            ("log", name) if !name.is_empty() => {
                let Some(status) = self
                    .bots
                    .statuses()
                    .into_iter()
                    .find(|s| s.config.name.eq_ignore_ascii_case(name))
                else {
                    return Err(CommandError::new(
                        ErrorKind::InvalidArgument,
                        "bot.unknown",
                        &[("bot", name)],
                        format!("No bot is called {name}"),
                    ));
                };
                let bot = status.config.name.clone();
                self.bot_notice(
                    channel,
                    "bot.log",
                    &[("bot", &bot)],
                    format!("Log of {bot}:"),
                );
                let skip = status.log.len().saturating_sub(BOT_LOG_SHOWN);
                for line in status.log.into_iter().skip(skip) {
                    // What the bot wrote itself has no code; it gets one,
                    // so that it too stays out of the chat log.
                    if line.code.is_empty() {
                        self.bot_notice(
                            channel,
                            "bot.output",
                            &[("line", &line.text)],
                            line.text.clone(),
                        );
                        continue;
                    }
                    let args: Vec<(&str, &str)> = line
                        .args
                        .iter()
                        .map(|(k, v)| (k.as_str(), v.as_str()))
                        .collect();
                    self.bot_notice(channel, &line.code, &args, line.text.clone());
                }
                Ok(())
            }
            _ => Err(CommandError::new(
                ErrorKind::InvalidArgument,
                "usage.bot",
                &[],
                "Usage: /bots, or /bot start|stop|log <name>",
            )),
        }
    }

    /// A line about bots in a channel, for this session's screen only.
    fn bot_notice(&mut self, channel: &UserId, code: &str, args: &[(&str, &str)], text: String) {
        self.apply(Event::Line {
            channel: channel.clone(),
            line: Line {
                time: liveside_proto::Timestamp::now(),
                content: LineContent::Notice {
                    level: NoticeLevel::Info,
                    text,
                    code: code.to_owned(),
                    args: args
                        .iter()
                        .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
                        .collect(),
                },
                flags: liveside_proto::model::LineFlags::default(),
            },
        });
    }

    /// Hands a chat message to the bots of its channel: one said just now,
    /// and not one a bot said itself, coming back as the account's, which
    /// a bot would answer, and that answer again. What the user types as
    /// the account they hear like anybody's.
    fn tell_bots(&mut self, channel: &UserId, line: &Line, hidden: bool) {
        let LineContent::Message(message) = &line.content else {
            return;
        };
        if line.flags.historical {
            return;
        }
        let Some(login) = self.channel(channel).map(|c| c.channel.login.clone()) else {
            return;
        };
        let now = std::time::Instant::now();
        while self
            .bot_said
            .front()
            .is_some_and(|said| now.duration_since(said.at) > BOT_ECHO)
        {
            self.bot_said.pop_front();
        }
        let mut said_by = None;
        if self
            .account
            .as_ref()
            .is_some_and(|account| account.user.id == message.sender.id)
        {
            let text = message.plain_text();
            let text = text.trim();
            if let Some(at) = self
                .bot_said
                .iter()
                .position(|said| said.channel == login.as_str() && said.text == text)
            {
                said_by = self.bot_said.remove(at).map(|said| said.bot);
            }
        }
        for event in self
            .bots
            .message(login.as_str(), message, said_by.as_deref(), hidden)
        {
            self.publish(event);
        }
    }

    /// What a bot's run reported, and what it asked to say.
    fn bot_output(&mut self, output: crate::bots::Output) {
        let (events, said) = self.bots.report(output, std::time::Instant::now());
        for event in events {
            self.publish(event);
        }
        for line in said {
            self.bot_says(line);
        }
    }

    /// Sends what a bot said, as the account; Twitch's refusal goes to the
    /// bot's log.
    fn bot_says(&mut self, said: crate::bots::Said) {
        let channel = self
            .channels
            .iter()
            .find(|c| c.channel.login.as_str().eq_ignore_ascii_case(&said.channel))
            .map(|c| c.channel.id.clone());
        let Some(channel) = channel else {
            let note = self.bots.note(
                &said.name,
                "bot.not_joined",
                &[("channel", &said.channel)],
                format!("#{} is not joined", said.channel),
            );
            if let Some(event) = note {
                self.publish(event);
            }
            return;
        };
        self.bot_said.push_back(BotSaid {
            bot: said.name.clone(),
            channel: said.channel.clone(),
            text: said.text.clone(),
            at: std::time::Instant::now(),
        });
        let (reply, answer) = oneshot::channel();
        self.forward(
            vec![BackendRequest::Send {
                channel,
                text: said.text,
                reply_to: said.reply_to,
            }],
            reply,
        );
        let reports = self.bots.reports();
        tokio::spawn(async move {
            if let Ok(Completion {
                result: Err(error), ..
            }) = answer.await
            {
                let _ = reports.send(crate::bots::Output::Refused {
                    name: said.name,
                    error,
                });
            }
        });
    }

    fn publish(&mut self, event: Event) {
        self.seq = Seq(self.seq.0 + 1);
        // No receivers is fine: nobody is attached right now.
        let _ = self.broadcast.send(Sequenced {
            seq: self.seq,
            event,
        });
    }

    fn channel(&self, id: &UserId) -> Option<&ChannelData> {
        self.channels.iter().find(|c| &c.channel.id == id)
    }

    fn channel_mut(&mut self, id: &UserId) -> Option<&mut ChannelData> {
        self.channels.iter_mut().find(|c| &c.channel.id == id)
    }
}

/// Whether a line should stand out: it addresses the logged-in account
/// (a mention fragment, or the login in the text), or it contains one of
/// the configured words. Own messages never count.
///
/// This is the whole highlight system for now; Chatty's rule language
/// (user:, cat:, re: and friends) comes later.
fn highlights(account: Option<&Account>, words: &[String], line: &Line) -> bool {
    let LineContent::Message(message) = &line.content else {
        return false;
    };
    if let Some(account) = account
        && message.sender.id == account.user.id
    {
        return false;
    }
    let me = account.map(|account| account.user.login.as_str());
    message.fragments.iter().any(|fragment| match fragment {
        Fragment::Mention { user, .. } => account.is_some_and(|account| user.id == account.user.id),
        Fragment::Text { text } => {
            me.is_some_and(|me| contains_word(text, me))
                || words.iter().any(|word| contains_word(text, word))
        }
        _ => false,
    })
}

/// Case-insensitive search for `name` as a separate word.
fn contains_word(text: &str, name: &str) -> bool {
    if name.is_empty() {
        return false;
    }
    let text = text.to_lowercase();
    let name = name.to_lowercase();
    text.match_indices(&name).any(|(i, _)| {
        let before_ok = text[..i]
            .chars()
            .next_back()
            .is_none_or(|c| !c.is_alphanumeric() && c != '_');
        let after_ok = text[i + name.len()..]
            .chars()
            .next()
            .is_none_or(|c| !c.is_alphanumeric() && c != '_');
        before_ok && after_ok
    })
}

/// A log event with the code a frontend needs to translate it.
fn log(level: NoticeLevel, code: &str, args: &[(&str, &str)], text: String) -> Event {
    Event::Log {
        level,
        text,
        code: code.to_owned(),
        args: args
            .iter()
            .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
            .collect(),
    }
}

impl Engine {
    /// What joining these means: channels by name, and every channel of a
    /// `[category]` in the address book. Nothing is asked of Twitch when
    /// any of it cannot be read.
    fn joining(&mut self, targets: &[String]) -> Result<Vec<BackendRequest>, CommandError> {
        let mut logins: Vec<Login> = Vec::new();
        for target in targets {
            match crate::addressbook::category_target(target) {
                Some(Ok((category, which))) => {
                    let names = self.config.addressbook.names_in(&category, which);
                    if names.is_empty() {
                        return Err(CommandError::new(
                            ErrorKind::InvalidArgument,
                            "join.category_empty",
                            &[("category", &category)],
                            format!("Nothing in the address book is in the category {category}"),
                        ));
                    }
                    for name in names {
                        logins.push(input::normalize_channel(name)?);
                    }
                }
                Some(Err(e)) => {
                    let args: Vec<(&str, &str)> =
                        e.args.iter().map(|(k, v)| (*k, v.as_str())).collect();
                    return Err(CommandError::new(
                        ErrorKind::InvalidArgument,
                        e.code,
                        &args,
                        e.message,
                    ));
                }
                None => logins.push(input::normalize_channel(target)?),
            }
        }
        if logins.is_empty() {
            return Err(CommandError::new(
                ErrorKind::InvalidArgument,
                "usage.join",
                &[],
                "Usage: /join <channel>[,<channel>...]",
            ));
        }
        let mut seen = std::collections::HashSet::new();
        logins.retain(|login| seen.insert(login.clone()));
        Ok(logins
            .into_iter()
            .map(|channel| {
                // Said before anything is asked of Twitch, so every
                // frontend can show the name while the work happens.
                self.publish(Event::ChannelJoining {
                    login: channel.clone(),
                });
                BackendRequest::Join { channel }
            })
            .collect())
    }
}

fn not_joined(channel: &UserId) -> CommandError {
    CommandError::new(
        ErrorKind::NotJoined,
        "channel.not_joined",
        &[("channel", channel.as_str())],
        format!("Not in channel {channel}"),
    )
}

fn backend_gone() -> CommandError {
    CommandError::new(
        ErrorKind::Internal,
        "backend.stopped",
        &[],
        "Chat backend stopped",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use liveside_proto::Timestamp;
    use liveside_proto::model::{ChatMessage, LineFlags, MessageKind, User};

    /// A channel with nothing in it, for the apply() tests.
    fn empty_channel(id: &str) -> ChannelData {
        ChannelData {
            channel: liveside_proto::model::Channel {
                id: id.into(),
                login: "quiet".into(),
                display_name: "quiet".to_owned(),
            },
            modes: liveside_proto::model::RoomModes::default(),
            lines: VecDeque::new(),
            chatters: BTreeSet::new(),
            live: false,
            held: Vec::new(),
            unban_requests: Vec::new(),
            poll: None,
            prediction: None,
            redemptions: Vec::new(),
            shield: false,
            stats: None,
            pending_suspicion: Vec::new(),
            writers: std::collections::HashSet::new(),
            hidden: 0,
            said: Default::default(),
            viewers: Default::default(),
        }
    }

    fn message_line(id: &str) -> Line {
        Line {
            time: Timestamp(1_790_000_000_000),
            content: LineContent::Message(ChatMessage {
                id: id.into(),
                sender: User {
                    id: "7".into(),
                    login: "alice".into(),
                    display_name: "alice".to_owned(),
                },
                color: None,
                badges: Vec::new(),
                fragments: Vec::new(),
                kind: MessageKind::Text,
                bits: None,
                reply: None,
                shared_source: None,
            }),
            flags: LineFlags::default(),
        }
    }

    fn suspicion_of(channel: &UserId, id: &str) -> Event {
        Event::SuspiciousMessage {
            channel: channel.clone(),
            message: id.into(),
            user: User {
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
        }
    }

    /// An engine with one channel and nothing else, for testing what
    /// apply() does to the lines it keeps.
    fn engine_with_channel() -> Engine {
        let (broadcast, _) = broadcast::channel(16);
        let (backend, _keep) = mpsc::unbounded_channel();
        let mut engine = Engine {
            history: History::new(crate::history::HistoryConfig::disabled()),
            history_broken: false,
            open_channels_broken: false,
            backend_flows: Vec::new(),
            backend_name: "test".to_owned(),
            config: EngineConfig::default(),
            seq: Seq(0),
            broadcast,
            backend,
            account: None,
            connection: ConnectionState::Disconnected,
            channels: vec![empty_channel("1001")],
            whispers: std::collections::BTreeMap::new(),
            pending: HashMap::new(),
            next_token: 0,
            link_reports: HashMap::new(),
            highlighted: VecDeque::new(),
            ignored: VecDeque::new(),
            follower: None,
            link_jobs: None,
            link_answers: mpsc::channel(1).0,
            bots: crate::bots::Bots::new(
                Vec::new(),
                std::env::temp_dir().join("liveside-engine-test-bots"),
                mpsc::unbounded_channel().0,
            ),
            bot_said: VecDeque::new(),
        };
        engine.config.history = crate::history::HistoryConfig::disabled();
        engine
    }

    #[test]
    fn a_suspicion_finds_its_message_in_either_order() {
        let channel: UserId = "1001".into();

        // The message first, and what Twitch thinks of whoever sent it
        // after: the line already kept is marked.
        let mut engine = engine_with_channel();
        engine.apply(Event::Line {
            channel: channel.clone(),
            line: message_line("m1"),
        });
        engine.apply(suspicion_of(&channel, "m1"));
        assert!(marked(&engine, "m1"), "message first");

        // ...and the other way round, which Twitch also does: the
        // suspicion waits rather than being thrown away.
        let mut engine = engine_with_channel();
        engine.apply(suspicion_of(&channel, "m2"));
        engine.apply(Event::Line {
            channel: channel.clone(),
            line: message_line("m2"),
        });
        assert!(marked(&engine, "m2"), "suspicion first");
    }

    fn said(sender: &str, words: &str) -> Event {
        let mut line = line(sender, text(words));
        if let LineContent::Message(message) = &mut line.content {
            message.id = format!("{sender}-{words}").into();
            message.sender.login = format!("user{sender}").into();
        }
        Event::Line {
            channel: "1001".into(),
            line,
        }
    }

    fn with_rules(highlight: &[&str], ignore: &[&str], filter: &[&str]) -> Engine {
        let own = |list: &[&str]| list.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
        let mut engine = engine_with_channel();
        engine.config.rules =
            crate::rules::Rules::read(&own(highlight), &own(ignore), &own(filter)).unwrap();
        engine
    }

    fn kept(engine: &Engine) -> Vec<(String, bool)> {
        engine.channels[0]
            .lines
            .iter()
            .map(|line| {
                let words = match &line.content {
                    LineContent::Message(m) => m.plain_text(),
                    LineContent::Notice { text, .. } => text.clone(),
                    LineContent::UserNotice { system_text, .. } => system_text.clone(),
                };
                (words, line.flags.highlighted)
            })
            .collect()
    }

    #[test]
    fn a_highlight_rule_lights_up_in_its_colours_and_the_first_one_decides() {
        let mut engine = with_rules(
            &["color:orange bgcolor:#000 config:silent w:rust", "w:rust"],
            &[],
            &[],
        );
        engine.apply(said("7", "I like rust"));
        engine.apply(said("7", "I like go"));
        assert_eq!(
            kept(&engine),
            [("I like rust".into(), true), ("I like go".into(), false)]
        );
        let flags = &engine.channels[0].lines[0].flags;
        assert_eq!(
            flags.highlight_color,
            liveside_proto::Rgb::parse_hex("ffa500")
        );
        assert_eq!(
            flags.highlight_background,
            liveside_proto::Rgb::parse_hex("000000")
        );
        assert!(flags.highlight_silent && !flags.highlight_quiet);
    }

    #[test]
    fn ignored_messages_are_hidden_and_counted_in_the_channel_later() {
        let mut engine = with_rules(&["config:!ignore w:urgent"], &["user:user9"], &[]);
        let mut events = engine.broadcast.subscribe();
        engine.apply(said("9", "spam"));
        engine.apply(said("9", "more spam"));
        engine.apply(said("9", "urgent spam"));
        engine.apply(said("7", "hello"));
        // The one a highlight rule lets through is shown.
        assert_eq!(
            kept(&engine),
            [("urgent spam".into(), true), ("hello".into(), false)]
        );
        // Nothing about the hidden ones reached a frontend.
        let mut published = 0;
        while let Ok(event) = events.try_recv() {
            if let Event::Line { line, .. } = event.event {
                assert!(
                    !matches!(&line.content, LineContent::Message(m) if m.plain_text().ends_with(" spam") && !m.plain_text().starts_with("urgent"))
                );
                published += 1;
            }
        }
        assert_eq!(published, 2);

        engine.say_hidden();
        let (words, _) = kept(&engine).pop().unwrap();
        assert_eq!(words, "2 messages hidden by the ignore rules");
        let LineContent::Notice { code, args, .. } = &engine.channels[0].lines[2].content else {
            panic!("a notice");
        };
        assert_eq!(code, HIDDEN_CODE);
        assert_eq!(args, &[("count".to_owned(), "2".to_owned())]);
        // Said once: nothing hidden since, nothing to say.
        engine.say_hidden();
        assert_eq!(kept(&engine).len(), 3);
    }

    #[test]
    fn what_lit_up_and_what_was_hidden_are_kept_for_their_lists() {
        let mut engine = with_rules(&["w:rust"], &["user:user9"], &[]);
        let mut events = engine.broadcast.subscribe();
        engine.apply(said("7", "rust is nice"));
        engine.apply(said("9", "buy followers"));
        engine.apply(said("7", "plain"));
        let mut historical = line("7", text("old rust"));
        historical.flags.historical = true;
        engine.apply(Event::Line {
            channel: "1001".into(),
            line: historical,
        });
        let words = |list: &[liveside_proto::model::CollectedLine]| -> Vec<String> {
            list.iter()
                .map(|c| match &c.line.content {
                    LineContent::Message(m) => format!("#{} {}", c.channel.login, m.plain_text()),
                    _ => String::new(),
                })
                .collect()
        };
        let snapshot = engine.attach().snapshot;
        assert_eq!(words(&snapshot.highlighted), ["#quiet rust is nice"]);
        assert_eq!(words(&snapshot.ignored), ["#quiet buy followers"]);
        // A frontend hears the hidden one as hidden, and only as that.
        let mut ignored = 0;
        while let Ok(event) = events.try_recv() {
            match event.event {
                Event::Ignored { line, .. } => {
                    assert!(
                        matches!(&line.content, LineContent::Message(m) if m.plain_text() == "buy followers")
                    );
                    ignored += 1;
                }
                Event::Line { line, .. } => {
                    assert!(
                        !matches!(&line.content, LineContent::Message(m) if m.plain_text() == "buy followers")
                    );
                }
                _ => {}
            }
        }
        assert_eq!(ignored, 1);
    }

    #[test]
    fn the_account_itself_is_never_hidden_filtered_or_lit_up() {
        let mut engine = with_rules(&["w:spam"], &["w:spam"], &["w:spam"]);
        engine.account = Some(account("brightmaple"));
        engine.apply(said("42", "my spam"));
        assert_eq!(kept(&engine), [("my spam".into(), false)]);
    }

    #[test]
    fn a_filter_changes_what_is_shown_and_the_log_keeps_what_was_said() {
        let dir =
            std::env::temp_dir().join(format!("liveside-engine-filter-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut engine = with_rules(&[], &[], &["replacement:*** w:darn"]);
        engine.history = History::new(crate::history::HistoryConfig {
            dir: Some(dir.clone()),
            lines: 20,
            max_bytes: 1024 * 1024,
        });
        engine.apply(said("7", "oh darn it, darn"));
        assert_eq!(kept(&engine), [("oh *** it, ***".into(), false)]);
        let logged = engine.history.load("quiet").unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
        let LineContent::Message(message) = &logged[0].content else {
            panic!("a message");
        };
        assert_eq!(message.plain_text(), "oh darn it, darn");
    }

    #[test]
    fn the_address_book_gives_rules_their_categories_and_joins_a_category() {
        let mut engine = with_rules(&[], &[], &[]);
        let (reply, _answer) = oneshot::channel();
        engine.command(
            Command::SetRules {
                highlight_words: Vec::new(),
                highlight: vec!["cat:friends".into()],
                ignore: vec!["chanCat:muted".into()],
                filter: Vec::new(),
                addressbook: [
                    ("User7".to_owned(), vec!["friends".to_owned()]),
                    ("#harborlight".to_owned(), vec!["streams".to_owned()]),
                    ("brightmaple".to_owned(), vec!["streams".to_owned()]),
                ]
                .into(),
                repeated: Default::default(),
            },
            reply,
        );
        engine.apply(said("7", "hi"));
        engine.apply(said("8", "hello"));
        assert_eq!(
            kept(&engine),
            [("hi".into(), true), ("hello".into(), false)]
        );

        let joined = |engine: &mut Engine, what: &str| -> Result<Vec<String>, String> {
            engine
                .joining(&liveside_proto::join_targets(what))
                .map(|requests| {
                    requests
                        .into_iter()
                        .map(|r| match r {
                            BackendRequest::Join { channel } => channel.to_string(),
                            _ => panic!("only joins"),
                        })
                        .collect()
                })
                .map_err(|e| e.code)
        };
        assert_eq!(
            joined(&mut engine, "[streams], harborlight, other"),
            Ok(vec![
                "harborlight".into(),
                "brightmaple".into(),
                "other".into()
            ]),
            "the book's order, each channel once"
        );
        assert_eq!(
            joined(&mut engine, "[streams #]"),
            Ok(vec!["harborlight".into()])
        );
        assert_eq!(
            joined(&mut engine, "[streams !#]"),
            Ok(vec!["brightmaple".into()])
        );
        assert_eq!(
            joined(&mut engine, "a [nobody]"),
            Err("join.category_empty".into()),
            "nothing is joined when part of it cannot be"
        );
        assert_eq!(input::join_args("/JOIN [streams]"), Some("[streams]"));
        assert_eq!(input::join_args("/joined"), None);
    }

    #[test]
    fn somebody_saying_it_again_is_marked_and_rules_can_see_it() {
        let mut engine = with_rules(&["config:repeatedmsg|3"], &[], &[]);
        for words in ["buy followers", "Buy  followers", "hi", "buy followers!"] {
            engine.apply(said("7", words));
        }
        // Somebody else saying the same is not them repeating it.
        engine.apply(said("8", "buy followers"));
        let marks: Vec<(String, u32, bool)> = engine.channels[0]
            .lines
            .iter()
            .map(|line| match &line.content {
                LineContent::Message(m) => {
                    (m.plain_text(), line.flags.repeated, line.flags.highlighted)
                }
                _ => panic!("messages"),
            })
            .collect();
        assert_eq!(
            marks,
            [
                ("buy followers".into(), 0, false),
                ("Buy  followers".into(), 2, false),
                ("hi".into(), 0, false),
                ("buy followers!".into(), 3, true),
                ("buy followers".into(), 0, false),
            ]
        );
        // History is not counted, and 0 turns it off.
        let mut old = line("7", text("buy followers"));
        old.flags.historical = true;
        engine.apply(Event::Line {
            channel: "1001".into(),
            line: old,
        });
        assert_eq!(engine.channels[0].lines.back().unwrap().flags.repeated, 0);
        engine.config.repeated.times = 0;
        engine.apply(said("7", "buy followers"));
        assert_eq!(engine.channels[0].lines.back().unwrap().flags.repeated, 0);
    }

    #[test]
    fn repetition_can_be_looked_for_only_where_a_rule_says() {
        let mut engine = with_rules(&[], &[], &[]);
        engine.config.repeated =
            crate::repeated::Repeated::read(&liveside_proto::model::RepeatedConfig {
                only: "!user:user7".into(),
                ..Default::default()
            })
            .unwrap();
        // Not quite the same, or they would be one message twice.
        for words in ["same again", "same again!"] {
            engine.apply(said("7", words));
            engine.apply(said("8", words));
        }
        let marks: Vec<u32> = engine.channels[0]
            .lines
            .iter()
            .map(|line| line.flags.repeated)
            .collect();
        assert_eq!(marks, [0, 0, 0, 2]);
    }

    #[test]
    fn the_audience_is_kept_with_what_changed_and_handed_over() {
        let mut engine = with_rules(&[], &[], &[]);
        let started = liveside_proto::Timestamp(1_000);
        for viewers in [100, 140] {
            engine.apply(Event::StreamStats {
                channel: "1001".into(),
                stats: liveside_proto::model::StreamStats { viewers, started },
            });
        }
        engine.apply(Event::Line {
            channel: "1001".into(),
            line: Line {
                time: liveside_proto::Timestamp(5_000),
                content: LineContent::Notice {
                    level: NoticeLevel::Info,
                    text: "Now: Chess — Chess".into(),
                    code: "event.channel_update".into(),
                    args: vec![
                        ("title".into(), "Chess".into()),
                        ("game".into(), "Chess".into()),
                    ],
                },
                flags: Default::default(),
            },
        });
        let snapshot = engine.attach().snapshot;
        let viewers = &snapshot.channels[0].viewers;
        assert_eq!(
            viewers
                .samples
                .iter()
                .map(|s| s.viewers)
                .collect::<Vec<_>>(),
            [100, 140]
        );
        assert_eq!(viewers.started, Some(started));
        assert_eq!(viewers.marks[0].title, "Chess");
    }

    #[test]
    fn a_first_message_is_the_first_this_session() {
        let mut engine = with_rules(&["config:firstmsg"], &[], &[]);
        engine.apply(said("7", "hi"));
        engine.apply(said("7", "again"));
        engine.apply(said("8", "hello"));
        assert_eq!(
            kept(&engine),
            [
                ("hi".into(), true),
                ("again".into(), false),
                ("hello".into(), true)
            ]
        );
    }

    #[test]
    fn rules_set_from_a_frontend_apply_at_once_or_not_at_all() {
        let mut engine = engine_with_channel();
        let set = |engine: &mut Engine, highlight: &str| {
            let (reply, answer) = oneshot::channel();
            engine.command(
                Command::SetRules {
                    highlight_words: vec!["word".into()],
                    highlight: vec![highlight.into()],
                    ignore: vec!["user:user9".into()],
                    filter: Vec::new(),
                    addressbook: Default::default(),
                    repeated: Default::default(),
                },
                reply,
            );
            answer.blocking_recv().unwrap().result
        };
        assert!(set(&mut engine, "w:rust").is_ok());
        assert_eq!(engine.config.rules.ignore.len(), 1);
        assert_eq!(engine.config.highlight_words, ["word"]);
        let refused = set(&mut engine, "msgs:friends").unwrap_err();
        assert_eq!(refused.code, "rules.invalid");
        assert!(
            refused.message.contains("highlight rule 1"),
            "{}",
            refused.message
        );
        // What was there before stays.
        assert_eq!(engine.config.rules.highlight[0].source, "w:rust");
    }

    fn marked(engine: &Engine, id: &str) -> bool {
        engine.channels.iter().any(|c| {
            c.lines.iter().any(|line| {
                matches!(&line.content, LineContent::Message(m) if m.id.as_str() == id)
                    && line.flags.suspicion.is_some()
            })
        })
    }

    fn account(login: &str) -> Account {
        Account {
            user: User {
                id: "42".into(),
                login: login.into(),
                display_name: login.into(),
            },
            scopes: Vec::new(),
        }
    }

    fn line(sender_id: &str, fragments: Vec<Fragment>) -> Line {
        Line {
            time: Timestamp(0),
            content: LineContent::Message(ChatMessage {
                id: "m".into(),
                sender: User {
                    id: sender_id.into(),
                    login: "someone".into(),
                    display_name: "Someone".into(),
                },
                color: None,
                badges: Vec::new(),
                fragments,
                kind: MessageKind::Text,
                bits: None,
                reply: None,
                shared_source: None,
            }),
            flags: LineFlags::default(),
        }
    }

    fn text(text: &str) -> Vec<Fragment> {
        vec![Fragment::Text { text: text.into() }]
    }

    #[test]
    fn mentions_are_highlighted() {
        let me = account("brightmaple");
        assert!(highlights(
            Some(&me),
            &[],
            &line("7", text("hi BrightMaple, look"))
        ));
        assert!(highlights(Some(&me), &[], &line("7", text("brightmaple!"))));
        assert!(highlights(
            Some(&me),
            &[],
            &line(
                "7",
                vec![Fragment::Mention {
                    text: "@BrightMaple".into(),
                    user: me.user.clone(),
                }],
            )
        ));
    }

    #[test]
    fn configured_words_highlight_too() {
        let me = account("brightmaple");
        let words = vec!["rust".to_owned(), "twitch".to_owned()];
        assert!(highlights(
            Some(&me),
            &words,
            &line("7", text("i like Rust"))
        ));
        // Whole words only, as for the account's own name.
        assert!(!highlights(Some(&me), &words, &line("7", text("rusty"))));
        // Still never our own messages.
        assert!(!highlights(Some(&me), &words, &line("42", text("rust"))));
        // And they work before logging in, unlike a mention.
        assert!(highlights(None, &words, &line("7", text("rust"))));
    }

    #[test]
    fn near_misses_are_not_highlighted() {
        let me = account("brightmaple");
        // Part of a longer word.
        assert!(!highlights(
            Some(&me),
            &[],
            &line("7", text("brightmaplexyz"))
        ));
        assert!(!highlights(
            Some(&me),
            &[],
            &line("7", text("_brightmaple"))
        ));
        // Our own message.
        assert!(!highlights(
            Some(&me),
            &[],
            &line("42", text("brightmaple"))
        ));
        // Not logged in.
        assert!(!highlights(None, &[], &line("7", text("brightmaple"))));
    }
}
