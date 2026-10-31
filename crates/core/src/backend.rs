//! Interface between the engine and a chat backend (Twitch, or the demo
//! backend used for development and tests).
//!
//! A backend is a task that receives [`Request`]s and emits
//! [`BackendMessage`]s: events, and the result of each request once it is
//! done. The engine applies the events to its state and forwards them to
//! frontends; it never talks to the network itself.
//!
//! Results travel on the same queue as events, so the engine sees a
//! request's result after every event the backend emitted for it. That is
//! what lets frontends rely on "the response comes after its effects".

use liveside_proto::{CommandResult, Event, Login, MessageId, UserId};
use tokio::sync::mpsc;

/// A user addressed by a moderation action. Typed commands carry IDs, while
/// text typed by the user (`/timeout name`) only has a login; the backend
/// resolves it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UserTarget {
    Id(UserId),
    Login(Login),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BackendRequest {
    Login,
    Logout,
    /// `channel` is already normalized: lowercase, without `#`.
    Join {
        channel: Login,
    },
    Part {
        channel: UserId,
    },
    Send {
        channel: UserId,
        text: String,
        reply_to: Option<MessageId>,
    },
    /// `/me`: what the account does, which only IRC can send.
    SendAction {
        channel: UserId,
        text: String,
    },
    Delete {
        channel: UserId,
        message: MessageId,
    },
    Timeout {
        channel: UserId,
        user: UserTarget,
        seconds: u32,
        reason: Option<String>,
    },
    Ban {
        channel: UserId,
        user: UserTarget,
        reason: Option<String>,
    },
    Unban {
        channel: UserId,
        user: UserTarget,
    },
    /// Warn a user: Twitch makes them acknowledge it before chatting again.
    Warn {
        channel: UserId,
        user: UserTarget,
        reason: String,
    },
    /// Remove every message in the channel.
    ClearChat {
        channel: UserId,
    },
    /// Post the highlighted message moderators can send.
    Announce {
        channel: UserId,
        text: String,
        /// `primary`, `blue`, `green`, `orange` or `purple`.
        color: String,
    },
    /// Change one chat restriction; the others stay as they are.
    SetRoomMode {
        channel: UserId,
        mode: RoomModeChange,
    },
    /// Change what the channel says it is streaming. Twitch only lets the
    /// broadcaster do this, however many channels one moderates.
    SetChannelInfo {
        channel: UserId,
        title: Option<String>,
        /// A category by name, as Twitch spells it.
        game: Option<String>,
        /// Two-letter code, or `other`.
        language: Option<String>,
        tags: Option<Vec<String>>,
        /// Seconds to hold the broadcast back. Partners only.
        delay: Option<u32>,
        /// Twitch's content labels, as the whole set: what is not in the
        /// list is turned off.
        labels: Option<Vec<String>>,
    },
    /// Twitch's panic button: the overrides a channel set up for a wave
    /// of abuse, on or off.
    ShieldMode {
        channel: UserId,
        on: bool,
    },
    /// Block a word or phrase, or unblock one. `*` at either end of a
    /// term stands for anything.
    BlockTerm {
        channel: UserId,
        text: String,
        on: bool,
    },
    /// Start a poll, or stop the one that is running.
    CreatePoll {
        channel: UserId,
        title: String,
        choices: Vec<String>,
        seconds: u32,
    },
    EndPoll {
        channel: UserId,
        archive: bool,
    },
    /// Start a prediction, or bring the running one to an end.
    CreatePrediction {
        channel: UserId,
        title: String,
        outcomes: Vec<String>,
        seconds: u32,
    },
    EndPrediction {
        channel: UserId,
        how: liveside_proto::PredictionEnd,
    },
    /// What the channel says about itself, as a form wants to read it.
    ShowChannelInfo {
        channel: UserId,
    },
    /// The channel's streaming schedule, and changing it.
    ShowSchedule {
        channel: UserId,
    },
    AddSegment {
        channel: UserId,
        start: String,
        minutes: u32,
        title: String,
        weekly: bool,
    },
    CancelSegment {
        channel: UserId,
        segment: String,
        canceled: bool,
    },
    DeleteSegment {
        channel: UserId,
        segment: String,
    },
    SetVacation {
        channel: UserId,
        away: Option<(String, String)>,
    },
    /// The advertisement schedule, and pushing the next break back.
    ShowAds {
        channel: UserId,
    },
    SnoozeAd {
        channel: UserId,
    },
    /// The channels this account moderates.
    ShowModeratedChannels,
    /// The emotes one can write in this channel, answered with
    /// `Event::EmoteList`.
    ListEmotes {
        channel: UserId,
    },
    /// The channels this account follows, live ones with their streams.
    ShowFollowedChannels,
    /// Whisper somebody, by login.
    SendWhisper {
        user: Login,
        text: String,
    },
    /// Clip the last minute and a half of the stream.
    CreateClip {
        channel: UserId,
    },
    /// Channel point rewards: what there is, adding one, removing one,
    /// and answering what somebody spent their points on.
    ShowRewards {
        channel: UserId,
    },
    CreateReward {
        channel: UserId,
        title: String,
        cost: u64,
        prompt: String,
    },
    UpdateReward {
        channel: UserId,
        /// The reward's id, or its name: a command takes the name, since
        /// nobody is going to type a Twitch id.
        reward: String,
        title: Option<String>,
        cost: Option<u64>,
        prompt: Option<String>,
        enabled: Option<bool>,
        paused: Option<bool>,
    },
    DeleteReward {
        channel: UserId,
        reward: String,
    },
    AnswerRedemption {
        channel: UserId,
        reward: String,
        redemption: String,
        fulfil: bool,
    },
    /// Look one person up, for their card.
    ShowUser {
        channel: UserId,
        user: Login,
    },
    /// Say who moderates, who is a VIP, or who is banned.
    ShowPeople {
        channel: UserId,
        list: PeopleList,
    },
    /// Say what is blocked, as a notice in the channel.
    ShowBlockedTerms {
        channel: UserId,
    },
    /// Watch a chatter, hold their messages for review, or lift both.
    SetSuspicious {
        channel: UserId,
        user: UserTarget,
        status: Suspicion,
    },
    /// Point the channel at another broadcaster: the pop-up with a follow
    /// button. A moderator may send these.
    Shoutout {
        channel: UserId,
        to: UserTarget,
    },
    /// Send the viewers to another channel, or call that off. Broadcaster
    /// only.
    Raid {
        channel: UserId,
        to: UserTarget,
    },
    CancelRaid {
        channel: UserId,
    },
    /// Mark this moment in the stream, for cutting a highlight later.
    Marker {
        channel: UserId,
        description: Option<String>,
    },
    /// Run an advertisement. Partners and affiliates only, and only the
    /// broadcaster themselves.
    Commercial {
        channel: UserId,
        seconds: u32,
    },
    /// Answer someone's request to be unbanned.
    ResolveUnban {
        channel: UserId,
        request: String,
        allow: bool,
    },
    /// Let a message AutoMod is holding through, or throw it away.
    ResolveHeld {
        channel: UserId,
        message: MessageId,
        allow: bool,
    },
    /// Pin a message to the top of the chat room, or take down whatever
    /// is pinned. Twitch keeps one moderator-pinned message per channel.
    Pin {
        channel: UserId,
        message: MessageId,
        /// 30 to 1800 seconds; without one it stays until the stream ends.
        seconds: Option<u32>,
    },
    Unpin {
        channel: UserId,
    },
    /// Give or take moderator or VIP status. Broadcaster only.
    SetRole {
        channel: UserId,
        user: UserTarget,
        role: Role,
        on: bool,
    },
}

/// One chat restriction, as a command changes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RoomModeChange {
    /// `None` turns it off; `Some(seconds)` sets the wait.
    Slow(Option<u32>),
    /// `Some(minutes)` requires following for that long.
    Followers(Option<u32>),
    Subscribers(bool),
    /// How long everyone but the moderators waits before their message
    /// appears. Twitch allows 2, 4 or 6 seconds; `None` turns it off.
    NonModeratorDelay(Option<u32>),
    EmoteOnly(bool),
    UniqueChat(bool),
}

/// Which list of people to show.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PeopleList {
    Moderators,
    Vips,
    Banned,
    Followers,
    Subscribers,
}

/// What a moderator has decided about a chatter they do not trust.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Suspicion {
    /// Their messages are flagged for moderators, but still shown.
    Monitored,
    /// Their messages are held until a moderator lets them through.
    Restricted,
    /// Neither: back to an ordinary chatter.
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Moderator,
    Vip,
}

/// A request and the token its result must carry. The backend answers when
/// the action is done (or failed), not when it was merely queued, and must
/// answer every request exactly once.
#[derive(Debug)]
pub struct Request {
    pub token: u64,
    pub request: BackendRequest,
}

#[derive(Debug)]
// An event is what this enum exists to carry, and the other variant is
// an answer to a command — rarer by orders of magnitude. Boxing the
// event to even the two out would put an allocation in the path of every
// chat message to save a few bytes on the path of a command result.
#[allow(clippy::large_enum_variant)]
pub enum BackendMessage {
    Event(Event),
    Done { token: u64, result: CommandResult },
}

/// The backend's ends of the channels to the engine.
#[derive(Debug)]
pub struct BackendIo {
    /// Unbounded: requests come from users and are few, and a bounded queue
    /// could deadlock with a backend blocked on a full event queue.
    pub requests: mpsc::UnboundedReceiver<Request>,
    pub events: mpsc::Sender<BackendMessage>,
}
