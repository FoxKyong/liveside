//! Notifications from the engine to all attached frontends.

use serde::{Deserialize, Serialize};

use crate::ids::{Login, MessageId, Timestamp, UserId};
use crate::model::{
    Channel, ChannelInfo, Line, NoticeLevel, Poll, Prediction, Redemption, Reward, RoomModes,
    Schedule, User,
};

/// The logged-in account.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Account {
    pub user: User,
    /// OAuth scopes the token grants.
    pub scopes: Vec<String>,
}

/// State of the connection to Twitch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum ConnectionState {
    Disconnected,
    Connecting,
    Connected,
    /// Lost the connection, trying again.
    Reconnecting {
        attempt: u32,
        reason: String,
    },
}

/// A message caught before it reached the chat, waiting for a decision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HeldMessage {
    pub id: MessageId,
    pub sender: User,
    /// What they wrote, as text: enough to judge it by.
    pub text: String,
    /// Why it was held, in Twitch's words: `automod` or `blocked_term`.
    pub reason: String,
    /// What AutoMod objected to (`aggressive`, `sexuality`, ...) or the
    /// blocked term it matched. Empty when Twitch did not say.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub detail: String,
    pub time: Timestamp,
}

/// Which list of people was asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PeopleList {
    Moderators,
    Vips,
    Banned,
    /// Who follows the channel, the newest first.
    Followers,
    /// Who subscribes to it.
    Subscribers,
}

/// Somebody a list names, and what the list says about them: when a
/// timeout runs out, since when they follow, their tier (`1`, `2`, `3`);
/// nothing for a ban, a moderator or a VIP.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Person {
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub detail: String,
    /// Who gave them the subscription, when it was a gift.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub gifted_by: String,
}

/// What is known about one person in one channel. The parts a moderator
/// is not allowed to ask for are simply absent, rather than guessed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserCard {
    pub user: User,
    /// When the account was made, as Twitch writes it.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub created_at: String,
    /// Since when they follow this channel, when Twitch answered and
    /// they do.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub follows_since: Option<String>,
    /// Whether Twitch answered the question. It refuses in a channel
    /// this account neither owns nor moderates, and "no" and "we were
    /// not allowed to ask" are different things to say about a person.
    #[serde(default)]
    pub follows_known: bool,
    #[serde(default)]
    pub sub_known: bool,
    /// What they subscribe at: `1`, `2`, `3`, or empty for neither
    /// subscribed nor answerable.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub sub_tier: String,
    /// Whether that subscription was a gift.
    #[serde(default)]
    pub sub_gift: bool,
    /// Whether they are banned here at all...
    #[serde(default)]
    pub banned: bool,
    /// ...and when the timeout runs out, if it is one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub banned_until: Option<String>,
}

/// A request from someone banned to be let back in.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnbanRequest {
    /// Twitch's id for the request, which is what resolves it.
    pub id: String,
    pub user: User,
    /// What they wrote in their own defence.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub text: String,
    pub time: Timestamp,
}

/// A joined channel with its current state; part of a snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChannelState {
    pub channel: Channel,
    pub modes: RoomModes,
    /// Most recent lines, oldest first. Bounded by the engine's scrollback
    /// setting.
    pub lines: Vec<Line>,
    /// Logins known to be in the channel, when the engine tracks
    /// membership (see `ChatterSource`). Empty otherwise, and frontends
    /// then fall back to whoever has talked.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub chatters: Vec<Login>,
    /// Messages waiting for a moderator's decision, oldest first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub held: Vec<HeldMessage>,
    /// Unban requests waiting for an answer, oldest first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unban_requests: Vec<UnbanRequest>,
    /// The poll running in the channel, if one is, so that a frontend
    /// joining late or reconnecting sees it rather than waiting for the
    /// next update.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub poll: Option<Poll>,
    /// The prediction running in the channel, if one is, for the same
    /// reason as the poll.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prediction: Option<Prediction>,
    /// Redemptions waiting to be answered, oldest first. Only those of
    /// rewards this client created ever appear: Twitch lets no other
    /// application answer them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub redemptions: Vec<Redemption>,
    /// Whether Shield Mode is on, as far as the engine knows: asked on
    /// joining and kept up by the two events. False also means "not
    /// known to be on", as for `live`.
    #[serde(default)]
    pub shield: bool,
    /// Whether the channel is streaming, as far as the engine knows.
    /// False until it has been able to ask, which is why frontends should
    /// treat it as "not known to be live" rather than "known to be off".
    #[serde(default)]
    pub live: bool,
    /// How many are watching and since when, while it is streaming.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stats: Option<crate::model::StreamStats>,
    /// How many watched this stream over time, for a graph.
    #[serde(default, skip_serializing_if = "crate::model::ViewerHistory::is_empty")]
    pub viewers: crate::model::ViewerHistory,
}

/// Everything a frontend needs to render the current state from scratch.
/// Sent once after attaching, before any live event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Snapshot {
    pub account: Option<Account>,
    pub connection: ConnectionState,
    pub channels: Vec<ChannelState>,
    /// Which backend the engine runs: `twitch`, `demo`, ... Frontends say
    /// so when it is not the real thing, because invented chat looks
    /// exactly like the real one otherwise.
    #[serde(default)]
    pub backend: String,
    /// What this engine does with the user's data, for the frontend to
    /// show. Sent with every snapshot so a frontend can answer the
    /// question without knowing how the engine was configured.
    #[serde(default)]
    pub data_flows: Vec<DataFlow>,
    /// Whispers, one thread per person. The same shape as a channel
    /// without any of the moderation, which is what a whisper is.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub whispers: Vec<WhisperThread>,
    /// Links checked so far in this session, so a frontend that attaches
    /// later shows them like one that was there.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub link_reports: Vec<crate::model::LinkReport>,
    /// What lit up this session, in every channel, oldest first, so a
    /// frontend can list it after the channel has scrolled it away.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub highlighted: Vec<crate::model::CollectedLine>,
    /// And what the ignore rules hid, which is not anywhere else.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ignored: Vec<crate::model::CollectedLine>,
    /// The user's own bots, and whether each runs.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub bots: Vec<crate::model::BotStatus>,
    /// Channels asked for and not joined yet, in the order they were
    /// asked for. The engine starts joining the channels of the last run
    /// before any frontend attaches, so their `ChannelJoining` went to
    /// nobody; a frontend shows these as it shows that event.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub joining: Vec<crate::Login>,
}

/// What has been said to and from one person.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WhisperThread {
    pub user: User,
    pub lines: Vec<Line>,
}

/// One thing the engine does with the user's data: a service it talks to,
/// or something it keeps on disk.
///
/// The point is that this list is built from the same settings that drive
/// the behaviour, so it cannot quietly drift from what the code does.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DataFlow {
    /// Who is involved, e.g. `Twitch` or `7TV`.
    pub party: String,
    /// What the party is when it is not a name, e.g. `flow.party.me` for
    /// "This machine", so that it too can be said in another language.
    /// Empty for 7TV, Twitch and the like, which are called that
    /// everywhere.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub party_code: String,
    pub kind: DataFlowKind,
    /// What is sent or stored, in one sentence. English, shown as it is
    /// by a frontend that does not know the code.
    pub detail: String,
    /// What this flow is, e.g. `flow.twitch.api`; a frontend that knows
    /// the code says it in the reader's own language.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub code: String,
    /// What to fill into the pattern, e.g. the directory being written to.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<(String, String)>,
    /// Whether it is happening with the current settings.
    pub enabled: bool,
    /// How to turn it off, e.g. `--no-third-party-emotes`: an option, a
    /// command or a settings key, never words, since it is shown as it is
    /// in every language. Empty when the feature cannot be switched off
    /// (talking to Twitch, for a Twitch client).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub switch: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DataFlowKind {
    /// Leaves this machine.
    Network,
    /// Stays on this machine, but is written down.
    Disk,
}

/// Something that happened in the engine.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum Event {
    /// Show this to the user: open `uri` and enter `code` (Device Code
    /// Flow). Valid for `expires_in_secs`.
    LoginPending {
        uri: String,
        code: String,
        expires_in_secs: u32,
    },
    LoggedIn {
        account: Account,
    },
    LoginFailed {
        reason: String,
    },
    LoggedOut,

    Connection {
        state: ConnectionState,
    },

    /// Someone joined or left a channel, as Twitch IRC reports it. Only
    /// sent while membership tracking is on.
    ChatterJoined {
        channel: UserId,
        login: Login,
    },
    ChatterParted {
        channel: UserId,
        login: Login,
    },
    /// AutoMod, or the channel's own blocked terms, caught a message and
    /// is holding it for a moderator to decide. Only sent where the
    /// account moderates.
    MessageHeld {
        channel: UserId,
        held: HeldMessage,
    },
    /// A held message was decided, here or by another moderator, or it
    /// expired. The frontend takes it off the list either way.
    HeldResolved {
        channel: UserId,
        message: MessageId,
        /// What became of it, for saying so: `approved`, `denied`,
        /// `expired` or whatever else Twitch invents.
        status: String,
        /// Who decided, when Twitch says. Empty when it expired.
        #[serde(default, skip_serializing_if = "String::is_empty")]
        moderator: String,
    },

    /// How a poll stands: started, a new count, or finished. The status
    /// in it says which, so a frontend has one thing to handle rather
    /// than three.
    PollUpdate {
        channel: UserId,
        poll: Poll,
    },

    /// How a prediction stands: started, a new amount bet, locked, or
    /// finished. The status in it says which.
    PredictionUpdate {
        channel: UserId,
        prediction: Prediction,
    },

    /// What is known about a link, as asked for with `CheckLink`. Sent
    /// again as the answer changes: checking, then the result.
    LinkReport {
        report: crate::model::LinkReport,
    },

    /// The bots, all of them, after the list was set with `SetBots`.
    Bots {
        bots: Vec<crate::model::BotStatus>,
    },
    /// A bot started, stopped, ended or could not start.
    BotState {
        name: String,
        state: crate::model::BotState,
    },
    /// A line for a bot's log: what it wrote to its standard error, or
    /// what the engine has to say about it.
    BotLog {
        name: String,
        line: crate::model::BotLogLine,
    },

    /// What the channel says about itself, as asked for.
    ChannelInfo {
        channel: UserId,
        info: ChannelInfo,
    },

    /// A whisper, sent or received. `user` is always the other person,
    /// so a frontend has one thread to put it in either way.
    Whisper {
        user: User,
        line: Line,
    },

    /// When the channel says it will be streaming, as asked for.
    Schedule {
        channel: UserId,
        schedule: Schedule,
    },

    /// The rewards the channel offers, as asked for.
    Rewards {
        channel: UserId,
        rewards: Vec<Reward>,
    },

    /// Somebody spent their points, or what they spent them on was
    /// answered. The status says which.
    RedemptionUpdate {
        channel: UserId,
        redemption: Redemption,
    },

    /// A channel has been asked for and is not there yet: the name is
    /// known, nothing else is. Frontends show it at once, because
    /// joining takes a lookup, a dozen subscriptions and several
    /// questions, and an empty window reads as a broken one.
    ChannelJoining {
        login: crate::Login,
    },
    /// ...and it will not be joined after all.
    ChannelJoinFailed {
        login: crate::Login,
    },

    /// Shield Mode went on or off in a channel.
    ShieldMode {
        channel: UserId,
        on: bool,
    },

    /// Twitch thinks somebody is worth watching, and this is what they
    /// just said. The message itself arrives as a line of its own; this
    /// marks it rather than repeating it.
    SuspiciousMessage {
        channel: UserId,
        message: crate::MessageId,
        user: User,
        /// Boxed: this is the widest thing an event carries, and every
        /// other event would be padded out to its size.
        suspicion: Box<crate::model::Suspicion>,
    },

    /// A list of people the channel keeps: its moderators, its VIPs, or
    /// whoever is banned from it. Asked for, and answered once.
    People {
        channel: UserId,
        list: PeopleList,
        people: Vec<Person>,
        /// How many there are in all, for a list too long to send whole
        /// (followers, subscribers). Followers can be counted by anyone
        /// and named only by the channel and its moderators, so this may
        /// come with nobody named.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        total: Option<u64>,
        /// Subscriber points, which decide the channel's standing.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        points: Option<u64>,
    },

    /// The channels this account moderates, as asked for: what there is to
    /// join besides the channels one already knows.
    ModeratedChannels {
        channels: Vec<crate::model::Channel>,
    },

    /// The channels this account follows, as asked for: the live ones
    /// first, the most watched first, then the rest by name.
    FollowedChannels {
        channels: Vec<crate::model::FollowedChannel>,
    },

    /// A line the ignore rules hid: not in the channel, only in the list
    /// of what was hidden.
    Ignored {
        channel: UserId,
        line: Line,
    },

    /// Which of the followed channels stream now, the most watched first,
    /// heard every minute while the account allows it. `started` names
    /// the ones that began since the last look and are not joined (a
    /// joined one says so itself, with [`Event::Stream`]); it is empty
    /// at the first look, which is how things stand rather than news.
    FollowedStreams {
        live: Vec<crate::model::FollowedChannel>,
        started: Vec<UserId>,
    },

    /// The words and phrases the channel blocks, as asked for: a table in
    /// the window, a line in the terminal.
    BlockedTerms {
        channel: UserId,
        terms: Vec<crate::model::BlockedTerm>,
    },

    /// What was asked about one person, for the window to show and the
    /// terminal to print.
    UserCard {
        channel: UserId,
        card: UserCard,
    },

    /// Someone banned from the channel has asked to be let back in.
    UnbanRequested {
        channel: UserId,
        request: UnbanRequest,
    },
    /// An unban request was decided, here or by another moderator.
    UnbanResolved {
        channel: UserId,
        request: String,
        /// `approved`, `denied`, `canceled` — Twitch's own word.
        status: String,
        #[serde(default, skip_serializing_if = "String::is_empty")]
        moderator: String,
    },

    /// A channel started or stopped streaming. Sent when joining, with
    /// what the engine found, and whenever it changes after that.
    Stream {
        channel: UserId,
        live: bool,
    },

    /// How many are watching a stream and since when it runs, asked of
    /// Twitch about once a minute while the channel is streaming. A stream
    /// going off clears it; no event says so separately.
    StreamStats {
        channel: UserId,
        stats: crate::model::StreamStats,
    },

    /// The whole membership of a channel at once, replacing what the
    /// frontend holds: sent after (re)joining, from the server's list.
    Chatters {
        channel: UserId,
        logins: Vec<Login>,
    },

    /// Names the backend the engine is running, once at startup.
    Backend {
        name: String,
    },

    /// The engine's data flows changed, e.g. a backend started and said
    /// what it will contact. Replaces the list a frontend holds.
    DataFlows {
        flows: Vec<DataFlow>,
    },

    /// Something the user should know that belongs to no channel: a provider
    /// that would not answer, a subscription Twitch revoked. Frontends show
    /// these in their status view.
    ///
    /// Libraries must not print such things to stderr: with the engine in
    /// the same process as a terminal frontend, that corrupts the screen.
    Log {
        level: NoticeLevel,
        text: String,
        #[serde(default, skip_serializing_if = "String::is_empty")]
        code: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        args: Vec<(String, String)>,
    },

    ChannelJoined {
        channel: Channel,
        modes: RoomModes,
    },
    ChannelParted {
        channel: UserId,
    },
    RoomModesChanged {
        channel: UserId,
        modes: RoomModes,
    },

    /// A new line at the end of a channel's scrollback.
    Line {
        channel: UserId,
        line: Line,
    },
    /// Lines loaded from history, oldest first, to be shown before the
    /// channel's current lines.
    History {
        channel: UserId,
        lines: Vec<Line>,
    },

    /// A single message was deleted.
    MessageDeleted {
        channel: UserId,
        message: MessageId,
    },
    /// All messages of a user were removed: timeout (`seconds` set) or ban.
    UserCleared {
        channel: UserId,
        user: UserId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        seconds: Option<u32>,
    },
    /// The whole chat was cleared by a moderator.
    ChatCleared {
        channel: UserId,
    },
}
