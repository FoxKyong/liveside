//! Data model shared by the engine and frontends.
//!
//! Shaped after Twitch's EventSub payloads (structured message fragments
//! rather than IRC tags), but independent of them: third-party emotes are
//! already resolved by the engine, and there is room for engine-side
//! annotations such as highlights.

use serde::{Deserialize, Serialize};

use crate::ids::{Login, MessageId, Rgb, Timestamp, UserId};

/// A Twitch user as seen in chat.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct User {
    pub id: UserId,
    pub login: Login,
    /// Display name; may differ from the login in case or be localized
    /// (e.g. CJK characters).
    pub display_name: String,
}

/// A channel, identified by its broadcaster.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Channel {
    pub id: UserId,
    pub login: Login,
    pub display_name: String,
}

/// Chat restrictions currently active in a channel.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoomModes {
    pub emote_only: bool,
    /// Minimum follow age in minutes; `Some(0)` means any follower.
    pub followers_only: Option<u32>,
    pub subscribers_only: bool,
    /// Seconds between messages per user.
    pub slow: Option<u32>,
    pub unique_chat: bool,
}

/// A poll the channel is running, as it stands right now.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Poll {
    pub id: String,
    pub title: String,
    pub choices: Vec<PollChoice>,
    pub status: PollStatus,
    /// When voting closes (or closed), as Twitch writes it.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub ends_at: String,
}

/// One thing that can be voted for, and how many have.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PollChoice {
    pub id: String,
    pub title: String,
    /// Votes in total: the free one plus any paid with channel points.
    #[serde(default)]
    pub votes: u32,
}

/// Where a poll has got to. Twitch has more endings than beginnings: a
/// poll that ran its course, one stopped early with its votes kept, and
/// one stopped and hidden.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PollStatus {
    Active,
    Completed,
    Terminated,
    Archived,
}

/// A prediction the channel is running: viewers bet channel points on
/// what will happen, and the broadcaster says afterwards who was right.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Prediction {
    pub id: String,
    pub title: String,
    pub outcomes: Vec<PredictionOutcome>,
    pub status: PredictionStatus,
    /// When betting closes, as Twitch writes it. The time that matters:
    /// after it, the broadcaster has a day to say who won.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub locks_at: String,
    /// Which outcome was paid out, once one has been.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub winner: Option<String>,
}

/// One thing that can be bet on, with what has been bet on it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PredictionOutcome {
    pub id: String,
    pub title: String,
    /// How many people took this side...
    #[serde(default)]
    pub users: u32,
    /// ...and how many channel points they put on it.
    #[serde(default)]
    pub points: u64,
}

/// Where a prediction has got to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PredictionStatus {
    /// Betting is open.
    Active,
    /// Betting is closed and the outcome is not yet known. Twitch gives
    /// the broadcaster 24 hours here, and cancels it afterwards.
    Locked,
    /// Paid out to whoever took `winner`.
    Resolved,
    /// Called off; everybody got their points back.
    Canceled,
}

/// What a channel says about itself: the form behind `/title` and
/// `/game`, with the fields those two never touched.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChannelInfo {
    pub title: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub game: String,
    /// Two-letter code, or `other`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub language: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    /// Seconds the broadcast is held back. Partners only.
    #[serde(default)]
    pub delay: u32,
    /// Twitch's own label names, e.g. `DrugsIntoxication`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub labels: Vec<String>,
}

/// What the channel has said it will stream, and when.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Schedule {
    pub segments: Vec<ScheduleSegment>,
    /// When the broadcaster is away, if they have said so.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vacation: Option<(String, String)>,
}

/// One planned broadcast.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScheduleSegment {
    pub id: String,
    /// When it starts, as Twitch writes it.
    pub start: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub end: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub title: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub category: String,
    /// Every week, rather than once.
    #[serde(default)]
    pub weekly: bool,
    /// Called off: still on the schedule, not happening.
    #[serde(default)]
    pub canceled: bool,
}

/// A reward viewers can buy with channel points.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reward {
    pub id: String,
    pub title: String,
    pub cost: u64,
    /// What the viewer is asked, when the reward asks anything.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub prompt: String,
    pub enabled: bool,
    /// Paused: still listed, but nobody can redeem it just now.
    #[serde(default)]
    pub paused: bool,
    /// Whether this client may answer its redemptions. Twitch allows that
    /// only to the application that created the reward, so a reward made
    /// in the dashboard is readable here and not answerable.
    #[serde(default)]
    pub ours: bool,
}

/// Somebody spending their points on a reward, and what became of it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Redemption {
    pub id: String,
    pub reward_id: String,
    pub reward_title: String,
    pub cost: u64,
    pub user: User,
    /// What they typed, for a reward that asks for something.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub text: String,
    pub status: RedemptionStatus,
    pub time: Timestamp,
}

/// Where a redemption has got to. Only an unanswered one can be answered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RedemptionStatus {
    Unfulfilled,
    Fulfilled,
    Canceled,
}

/// A chat badge as sent by Twitch (e.g. set `subscriber`, id `12`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Badge {
    pub set_id: String,
    pub id: String,
    /// Extra data, e.g. exact subscription months. Empty if none.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub info: String,
    /// Where its picture is, when the backend could look it up. Twitch
    /// hands out badge pictures per channel and globally, and neither is
    /// derivable from the ids, so the backend fills this in rather than
    /// leaving a frontend to ask Twitch itself.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub image: String,
    /// What the badge is called, for a tooltip. Empty when unknown.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub title: String,
}

impl EmoteRef {
    /// Where to fetch this emote's picture. `scale` is 1, 2 or 3, roughly
    /// 1x, 2x and 4x. `None` for an emote that has no picture anywhere.
    ///
    /// Each provider's own API gave these; 7TV has no PNG, so the animated
    /// formats are asked for and the frontend decodes what it can.
    pub fn image_url(&self, scale: u8) -> Option<String> {
        let scale = scale.clamp(1, 3);
        let id = &self.id;
        Some(match self.provider {
            EmoteProvider::Twitch => {
                format!("https://static-cdn.jtvnw.net/emoticons/v2/{id}/default/dark/{scale}.0")
            }
            EmoteProvider::SevenTv => format!("https://cdn.7tv.app/emote/{id}/{scale}x.webp"),
            EmoteProvider::Bttv => format!("https://cdn.betterttv.net/emote/{id}/{scale}x"),
            EmoteProvider::Ffz => {
                let size = match scale {
                    1 => 1,
                    2 => 2,
                    _ => 4,
                };
                format!("https://cdn.frankerfacez.com/emote/{id}/{size}")
            }
            EmoteProvider::Local => return None,
        })
    }
}

/// Emotes that belong together in a list to pick from: one channel's,
/// Twitch's own, or one provider's.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EmoteGroup {
    pub kind: EmoteGroupKind,
    /// The channel's display name for a channel's emotes, empty otherwise.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub channel: String,
    pub provider: EmoteProvider,
    pub emotes: Vec<EmoteRef>,
}

/// What an [`EmoteGroup`] is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EmoteGroupKind {
    /// The Twitch emotes of the channel asked about that one may use.
    ThisChannel,
    /// Another channel's Twitch emotes that one may use, as a subscriber
    /// or a follower there.
    OtherChannel,
    /// Twitch's own, for everyone.
    Twitch,
    /// A third-party provider's emotes for the channel asked about.
    ProviderChannel,
    /// A third-party provider's emotes for every channel.
    ProviderGlobal,
}

/// Where an emote comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EmoteProvider {
    Twitch,
    Bttv,
    Ffz,
    SevenTv,
    /// User-defined local emote.
    Local,
}

/// Reference to an emote inside a message. Image URLs are not part of the
/// message; frontends resolve them from provider and id.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EmoteRef {
    pub provider: EmoteProvider,
    pub id: String,
    /// Text that produced the emote, e.g. `Kappa`.
    pub code: String,
    /// An animated version is available.
    #[serde(default)]
    pub animated: bool,
    /// Zero-width emote drawn over the previous one (7TV/BTTV overlays).
    #[serde(default)]
    pub zero_width: bool,
}

/// One piece of a chat message, in order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Fragment {
    Text {
        text: String,
    },
    Emote {
        #[serde(flatten)]
        emote: EmoteRef,
    },
    Mention {
        text: String,
        user: User,
    },
    Cheermote {
        text: String,
        /// Lowercase prefix, e.g. `cheer` for `Cheer100`.
        prefix: String,
        bits: u32,
        tier: u32,
    },
    /// A GIF attached by Twitch. The URL must be used unmodified.
    Gif {
        text: String,
        id: String,
        url: String,
    },
    /// A URL detected in the text. Detected by the engine, not by Twitch.
    Link {
        text: String,
        url: String,
    },
}

impl Fragment {
    /// The text this fragment stands for in plain-text rendering.
    pub fn text(&self) -> &str {
        match self {
            Fragment::Text { text }
            | Fragment::Mention { text, .. }
            | Fragment::Cheermote { text, .. }
            | Fragment::Gif { text, .. }
            | Fragment::Link { text, .. } => text,
            Fragment::Emote { emote } => &emote.code,
        }
    }
}

/// Twitch's special message kinds (EventSub `message_type`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageKind {
    #[default]
    Text,
    ChannelPointsHighlighted,
    ChannelPointsSubOnly,
    UserIntro,
    PowerUpsMessageEffect,
    PowerUpsGigantifiedEmote,
    /// Written with `/me`: shown as something the sender does, without
    /// the colon and in their colour.
    Action,
    /// A value this version does not know yet.
    #[serde(other)]
    Unknown,
}

/// The message a reply answers to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReplyTo {
    pub parent_id: MessageId,
    pub parent_user: User,
    pub parent_text: String,
    /// First message of the thread (equals `parent_id` for direct replies).
    pub thread_id: MessageId,
}

/// Origin of a message delivered through a Shared Chat session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SharedSource {
    pub channel: Channel,
    pub message_id: MessageId,
    pub badges: Vec<Badge>,
}

/// A chat message from a user.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatMessage {
    pub id: MessageId,
    pub sender: User,
    /// Name colour as set by the user; `None` if never set. Frontends choose
    /// a fallback and may adjust it for readability.
    pub color: Option<Rgb>,
    pub badges: Vec<Badge>,
    pub fragments: Vec<Fragment>,
    #[serde(default)]
    pub kind: MessageKind,
    /// Total bits cheered in this message.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bits: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply: Option<Box<ReplyTo>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shared_source: Option<Box<SharedSource>>,
}

impl ChatMessage {
    /// The message as plain text, emotes as their codes.
    pub fn plain_text(&self) -> String {
        self.fragments.iter().map(Fragment::text).collect()
    }
}

/// Engine-side annotations of a line, e.g. from highlight rules.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LineFlags {
    /// Sent by the logged-in account.
    #[serde(default)]
    pub own: bool,
    /// Matched a highlight rule (or mentions the account).
    #[serde(default)]
    pub highlighted: bool,
    /// Colour requested by the matching highlight rule, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub highlight_color: Option<Rgb>,
    /// Background requested by the matching highlight rule, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub highlight_background: Option<Rgb>,
    /// The rule wants no sound for it (`config:silent`).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub highlight_silent: bool,
    /// The rule wants no desktop notification for it (`config:!notify`).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub highlight_quiet: bool,
    /// Deleted by a moderator or by a timeout/ban. Kept for display.
    #[serde(default)]
    pub deleted: bool,
    /// From the history loaded on join, not received live.
    #[serde(default)]
    pub historical: bool,
    /// Why Twitch thinks this person is worth watching, when it does.
    /// Set on the message rather than kept in a list of its own: the
    /// reason is only useful beside what they actually said.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub suspicion: Option<Box<Suspicion>>,
    /// How many times the sender has now said this, or something like
    /// it, lately; 0 when it is not repeated enough to say so.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub repeated: u32,
}

fn is_zero(n: &u32) -> bool {
    *n == 0
}

/// When a message counts as repeated: the `[repeated]` settings, which
/// the engine applies as it does the rules.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RepeatedConfig {
    /// From the how-manyth time a message is marked; 0 turns it off.
    pub times: u32,
    /// How alike two messages must be, in percent, ignoring case and
    /// whitespace.
    pub similarity: u32,
    /// How far back to look.
    pub minutes: u32,
    /// Shorter messages are left alone.
    pub min_length: u32,
    /// A rule a message has to match to be looked at, such as
    /// `!status:M` to leave moderators alone. Empty for every message.
    pub only: String,
}

impl Default for RepeatedConfig {
    fn default() -> Self {
        Self {
            times: 2,
            similarity: 90,
            minutes: 60,
            min_length: 2,
            only: String::new(),
        }
    }
}

/// What Twitch says about somebody it does not trust.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Suspicion {
    /// Whether a moderator put them under watch, or restricted them.
    pub status: SuspicionStatus,
    /// Twitch's own reasons: `ban_evader`, `banned_in_shared_channel`,
    /// `manually_added`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reasons: Vec<String>,
    /// How likely Twitch thinks it is that they are evading a ban:
    /// `unknown`, `possible`, `likely`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub evasion: String,
    /// In how many channels sharing bans with this one they are banned.
    #[serde(default)]
    pub shared_bans: u32,
}

/// How closely a suspicious person is being held.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SuspicionStatus {
    /// Twitch flagged them; nobody has decided anything.
    None,
    /// A moderator is watching them.
    Monitored,
    /// Their messages are held before anybody sees them.
    Restricted,
}

/// Severity of an informational line.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NoticeLevel {
    Info,
    Warning,
    Error,
}

/// What a line in a channel's scrollback is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum LineContent {
    Message(ChatMessage),
    /// Subscription, raid, announcement and similar events shown in chat.
    /// `message` is the optional text the user attached.
    UserNotice {
        system_text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        message: Option<ChatMessage>,
    },
    /// Informational text from Twitch or the engine (e.g. "Joined #abc",
    /// "You are timed out").
    Notice {
        level: NoticeLevel,
        /// English text, shown as it is by a frontend that does not know
        /// the code.
        text: String,
        /// What this notice is, e.g. `channel.joined`; empty when the text
        /// comes from Twitch or from the frontend itself.
        #[serde(default, skip_serializing_if = "String::is_empty")]
        code: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        args: Vec<(String, String)>,
    },
}

/// One line in a channel's scrollback.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Line {
    pub time: Timestamp,
    pub content: LineContent,
    #[serde(default)]
    pub flags: LineFlags,
}

/// What a service said about a link from chat.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LinkReport {
    /// The address as `Fragment::Link` carries it.
    pub url: String,
    /// Who answered: one link can have an answer from each.
    #[serde(default)]
    pub source: LinkSource,
    pub state: LinkState,
}

/// Who found something out about a link.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LinkSource {
    #[default]
    VirusTotal,
    /// Its redirects, followed by the engine itself.
    Redirects,
}

/// Where a check of a link stands.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum LinkState {
    /// Asked, no answer yet (VirusTotal allows four questions a minute).
    Checking,
    /// VirusTotal has a report.
    Found {
        /// How many of its engines call it malicious, or suspicious.
        malicious: u32,
        suspicious: u32,
        /// How many looked at it at all.
        engines: u32,
        /// When it was last analysed.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        analysed: Option<Timestamp>,
        /// Where the link ends after its redirects, when that is
        /// somewhere else, and the way there.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        final_url: Option<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        redirects: Vec<String>,
    },
    /// Where following its redirects led. `final_url` is `None` when the
    /// link does not redirect at all. Only HTTP redirects are seen: a page
    /// that moves on by itself (JavaScript, `meta refresh`) is not.
    Leads {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        final_url: Option<String>,
        /// Every address on the way, the link itself first.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        redirects: Vec<String>,
        /// The status the last address answered with.
        status: u16,
    },
    /// VirusTotal has never seen it. It can be submitted for a scan.
    Unknown,
    /// Submitted; VirusTotal is scanning it.
    Scanning,
    /// It could not be checked. `code` says why, for a frontend to say in
    /// the reader's language (`link.no_key`, `link.bad_key`, `link.quota`,
    /// `link.off`, `link.slow`, `link.failed`, and for redirects
    /// `link.private`, `link.scheme`, `link.too_many`, `link.timeout`,
    /// `link.follow_off`); `detail` is the English fallback.
    Failed { code: String, detail: String },
}

/// A word or phrase a channel blocks. A `*` at either end stands for
/// anything.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlockedTerm {
    /// What Twitch removes it by.
    pub id: String,
    pub text: String,
    /// When it was added, as Twitch writes it (RFC 3339); empty when not
    /// known.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub created_at: String,
    /// When it stops being blocked, for one added for a while.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<String>,
}

/// How a stream stands: how many watch it, and when it started.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct StreamStats {
    pub viewers: u32,
    pub started: Timestamp,
}

/// How many watched at one moment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ViewerSample {
    pub time: Timestamp,
    pub viewers: u32,
}

/// Something that changed while the stream ran: its title or category.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StreamMark {
    pub time: Timestamp,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub title: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub game: String,
}

/// How many have watched one stream, minute by minute, and what changed
/// meanwhile: what a viewer graph is drawn from. Kept by the engine, so
/// a frontend that attaches late draws the same, and by a frontend as
/// samples arrive.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ViewerHistory {
    /// When the stream these belong to started.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started: Option<Timestamp>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub samples: Vec<ViewerSample>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub marks: Vec<StreamMark>,
}

impl ViewerHistory {
    /// A day of samples a minute apart.
    pub const KEPT: usize = 24 * 60;
    const MARKS: usize = 100;

    /// Notes how many watch now. A stream that started at another time
    /// is another stream, and begins the history again.
    pub fn record(&mut self, at: Timestamp, stats: &StreamStats) {
        if self.started != Some(stats.started) {
            *self = ViewerHistory {
                started: Some(stats.started),
                ..ViewerHistory::default()
            };
        }
        self.samples.push(ViewerSample {
            time: at,
            viewers: stats.viewers,
        });
        if self.samples.len() > Self::KEPT {
            self.samples.remove(0);
        }
    }

    /// Notes a change of title or category, if the stream is being
    /// followed at all.
    pub fn mark(&mut self, at: Timestamp, title: &str, game: &str) {
        if self.started.is_none() {
            return;
        }
        self.marks.push(StreamMark {
            time: at,
            title: title.to_owned(),
            game: game.to_owned(),
        });
        if self.marks.len() > Self::MARKS {
            self.marks.remove(0);
        }
    }

    pub fn is_empty(&self) -> bool {
        self.samples.is_empty() && self.marks.is_empty()
    }

    /// The most that watched, and when.
    pub fn peak(&self) -> Option<ViewerSample> {
        self.samples
            .iter()
            .copied()
            .max_by_key(|s| (s.viewers, -s.time.0))
    }

    /// How many watched on average, sample by sample.
    pub fn average(&self) -> Option<u32> {
        if self.samples.is_empty() {
            return None;
        }
        let sum: u64 = self.samples.iter().map(|s| u64::from(s.viewers)).sum();
        Some((sum / self.samples.len() as u64) as u32)
    }
}

/// A line kept apart from its channel: lit up by a highlight rule, or
/// hidden by an ignore rule, with where it was said.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CollectedLine {
    pub channel: Channel,
    pub line: Line,
}

/// A channel the account follows, and its stream if it is live.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FollowedChannel {
    pub channel: Channel,
    #[serde(default)]
    pub live: Option<LiveStream>,
}

/// What a channel streams now.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LiveStream {
    pub title: String,
    /// The category; empty when none is set.
    pub category: String,
    pub stats: StreamStats,
}

impl Timestamp {
    /// Reads Twitch's times, `2026-10-14T18:01:02Z` with or without a
    /// fraction of a second. Nothing but UTC (`Z`) is accepted, which is
    /// all Twitch writes.
    pub fn parse_utc(text: &str) -> Option<Self> {
        let text = text.strip_suffix('Z')?;
        let (date, time) = text.split_once('T')?;
        let mut date = date.split('-').map(str::parse::<i64>);
        let (year, month, day) = (date.next()?.ok()?, date.next()?.ok()?, date.next()?.ok()?);
        let (time, fraction) = time.split_once('.').unwrap_or((time, ""));
        let mut time = time.split(':').map(str::parse::<i64>);
        let (hour, minute, second) = (time.next()?.ok()?, time.next()?.ok()?, time.next()?.ok()?);
        if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
            return None;
        }
        let millis = fraction
            .chars()
            .chain(std::iter::repeat('0'))
            .take(3)
            .collect::<String>()
            .parse::<i64>()
            .ok()?;
        // Days since 1970-01-01 in the proleptic Gregorian calendar
        // (Howard Hinnant's days_from_civil).
        let year = if month <= 2 { year - 1 } else { year };
        let era = year.div_euclid(400);
        let year_of_era = year - era * 400;
        let day_of_year = (153 * ((month + 9) % 12) + 2) / 5 + day - 1;
        let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
        let days = era * 146_097 + day_of_era - 719_468;
        Some(Timestamp(
            ((days * 24 + hour) * 60 + minute) * 60_000 + second * 1000 + millis,
        ))
    }
}

/// A bot of the user's own, as `[[bots]]` in the settings defines it: a
/// program the engine starts and talks to. See `docs/bots.md`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BotConfig {
    /// How it is listed and named in commands; unique.
    pub name: String,
    /// The program and its arguments, run as they are, not by a shell.
    pub command: Vec<String>,
    /// Where it runs; the home directory when empty.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub dir: String,
    /// The channels it hears and may speak in, by login; every joined
    /// channel when empty.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub channels: Vec<String>,
    /// Started with the engine rather than when asked.
    #[serde(default)]
    pub autostart: bool,
    /// The most lines it may say in 30 seconds. Twitch allows an account
    /// 20, or 100 in a channel it moderates or owns.
    #[serde(
        default = "BotConfig::default_rate",
        skip_serializing_if = "BotConfig::is_default_rate"
    )]
    pub rate: u32,
    /// Seconds it has to end once its input is closed, before it is
    /// killed.
    #[serde(
        default = "BotConfig::default_stop_after",
        skip_serializing_if = "BotConfig::is_default_stop_after"
    )]
    pub stop_after: u32,
    /// Whether it hears what other bots say. Never what it says itself.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub hear_bots: bool,
    /// Whether it hears the people the ignore rules hide.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub hear_ignored: bool,
}

impl Default for BotConfig {
    fn default() -> Self {
        Self {
            name: String::new(),
            command: Vec::new(),
            dir: String::new(),
            channels: Vec::new(),
            autostart: false,
            rate: Self::DEFAULT_RATE,
            stop_after: Self::DEFAULT_STOP_AFTER,
            hear_bots: false,
            hear_ignored: false,
        }
    }
}

impl BotConfig {
    pub const DEFAULT_RATE: u32 = 20;
    pub const MAX_RATE: u32 = 100;
    pub const DEFAULT_STOP_AFTER: u32 = 2;
    pub const MAX_STOP_AFTER: u32 = 60;

    fn default_rate() -> u32 {
        Self::DEFAULT_RATE
    }

    fn is_default_rate(rate: &u32) -> bool {
        *rate == Self::DEFAULT_RATE
    }

    fn default_stop_after() -> u32 {
        Self::DEFAULT_STOP_AFTER
    }

    fn is_default_stop_after(seconds: &u32) -> bool {
        *seconds == Self::DEFAULT_STOP_AFTER
    }

    /// Whether it hears and may speak in this channel.
    pub fn serves(&self, channel: &str) -> bool {
        self.channels.is_empty()
            || self
                .channels
                .iter()
                .any(|c| c.trim_start_matches('#').eq_ignore_ascii_case(channel))
    }
}

/// Whether a bot runs.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum BotState {
    #[default]
    Stopped,
    Running,
    /// It ended by itself. `code` is none when a signal ended it.
    Ended {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        code: Option<i32>,
    },
    /// It could not be started.
    Failed {
        error: String,
    },
    /// Another engine on this machine runs it already.
    Elsewhere,
}

/// A bot, how it is, and what it said lately.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BotStatus {
    pub config: BotConfig,
    #[serde(default)]
    pub state: BotState,
    /// The newest lines of its log, oldest first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub log: Vec<BotLogLine>,
}

/// A line in a bot's log: what the bot wrote to its standard error, with
/// no code, or what the engine says about it, with one for a frontend to
/// say it in the reader's language.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BotLogLine {
    pub time: Timestamp,
    /// English, or the bot's own words.
    pub text: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub code: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<(String, String)>,
}

#[cfg(test)]
mod time_tests {
    use super::Timestamp;

    #[test]
    fn twitch_times_are_read() {
        assert_eq!(
            Timestamp::parse_utc("1970-01-01T00:00:00Z"),
            Some(Timestamp(0))
        );
        // As VirusTotal dated a report, cross-checked with `date -d`.
        assert_eq!(
            Timestamp::parse_utc("2026-10-13T20:02:26Z"),
            Some(Timestamp(1_791_921_746_000))
        );
        assert_eq!(
            Timestamp::parse_utc("2026-10-13T20:02:26.474434Z"),
            Some(Timestamp(1_791_921_746_474))
        );
        assert_eq!(
            Timestamp::parse_utc("2024-02-29T12:00:00Z"),
            Some(Timestamp(1_709_208_000_000))
        );
        assert_eq!(Timestamp::parse_utc("2026-10-13 20:02:26"), None);
        assert_eq!(Timestamp::parse_utc("2026-13-01T00:00:00Z"), None);
    }
}

#[cfg(test)]
mod viewer_tests {
    use super::*;

    #[test]
    fn a_stream_is_followed_minute_by_minute_and_a_new_one_starts_again() {
        let mut history = ViewerHistory::default();
        history.mark(Timestamp(5), "too early", "");
        assert!(history.marks.is_empty(), "no stream known yet");
        let first = Timestamp(1_000);
        for (minute, viewers) in [(1, 100), (2, 300), (3, 200), (4, 300)] {
            history.record(
                Timestamp(minute * 60_000),
                &StreamStats {
                    viewers,
                    started: first,
                },
            );
        }
        history.mark(Timestamp(150_000), "New title", "Chess");
        assert_eq!(history.samples.len(), 4);
        // The first time it was that high, and the plain average.
        assert_eq!(history.peak().unwrap().time, Timestamp(120_000));
        assert_eq!(history.average(), Some(225));
        assert_eq!(history.marks.len(), 1);

        history.record(
            Timestamp(9_000_000),
            &StreamStats {
                viewers: 7,
                started: Timestamp(8_000_000),
            },
        );
        assert_eq!(history.samples.len(), 1, "another stream");
        assert!(history.marks.is_empty());

        for minute in 0..(ViewerHistory::KEPT as i64 + 10) {
            history.record(
                Timestamp(9_000_000 + minute),
                &StreamStats {
                    viewers: 1,
                    started: Timestamp(8_000_000),
                },
            );
        }
        assert_eq!(history.samples.len(), ViewerHistory::KEPT);
    }
}
