//! Requests from a frontend to the engine.

use serde::{Deserialize, Serialize};

use crate::ids::{Login, MessageId, UserId};

/// Something a frontend asks the engine to do. Every command is answered
/// with a [`crate::wire::ServerMessage::Response`]; its effects arrive as
/// ordinary events, visible to all attached frontends.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "command", rename_all = "snake_case")]
pub enum Command {
    /// Start logging in. Progress arrives as `Event::Login*`.
    Login,
    /// Forget the stored account and disconnect.
    Logout,

    /// Join a channel by login (with or without a leading `#`).
    Join {
        channel: Login,
    },
    Part {
        channel: UserId,
    },

    /// A line typed by the user, exactly as typed. The engine handles
    /// `/commands` and sends everything else as a chat message, so both
    /// frontends share one command language.
    Input {
        channel: UserId,
        text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reply_to: Option<MessageId>,
    },
    /// Send text as a chat message without command interpretation.
    SendMessage {
        channel: UserId,
        text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reply_to: Option<MessageId>,
    },

    DeleteMessage {
        channel: UserId,
        message: MessageId,
    },
    Timeout {
        channel: UserId,
        user: UserId,
        seconds: u32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
    Ban {
        channel: UserId,
        user: UserId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
    Unban {
        channel: UserId,
        user: UserId,
    },
    /// Start a poll in the channel. Twitch allows two to five choices
    /// and between fifteen seconds and three minutes.
    CreatePoll {
        channel: UserId,
        title: String,
        choices: Vec<String>,
        seconds: u32,
    },
    /// Stop the poll that is running: with the votes kept and shown, or
    /// hidden altogether.
    EndPoll {
        channel: UserId,
        /// True hides it rather than showing the result.
        archive: bool,
    },
    /// Start a prediction. Twitch allows two to ten outcomes and a
    /// betting window between thirty seconds and half an hour.
    CreatePrediction {
        channel: UserId,
        title: String,
        outcomes: Vec<String>,
        seconds: u32,
    },
    /// Close betting, pay out, or call the whole thing off.
    EndPrediction {
        channel: UserId,
        how: PredictionEnd,
    },
    /// Ask what the channel says about itself, answered with
    /// [`crate::Event::ChannelInfo`].
    ShowChannelInfo {
        channel: UserId,
    },
    /// Change it. Only the fields that are `Some` are touched, so a form
    /// can send what was edited and leave the rest alone.
    SetChannelInfo {
        channel: UserId,
        title: Option<String>,
        game: Option<String>,
        language: Option<String>,
        tags: Option<Vec<String>>,
        delay: Option<u32>,
        labels: Option<Vec<String>>,
    },
    /// What the channel has planned, answered with
    /// [`crate::Event::Schedule`].
    ShowSchedule {
        channel: UserId,
    },
    /// Add a broadcast to the schedule: when it starts, how long it
    /// runs, what it is called, and whether it is every week.
    AddSegment {
        channel: UserId,
        /// RFC3339, as Twitch wants it.
        start: String,
        minutes: u32,
        title: String,
        weekly: bool,
    },
    /// Call one off, or put it back on. A weekly broadcast loses only
    /// that one occurrence.
    CancelSegment {
        channel: UserId,
        segment: String,
        canceled: bool,
    },
    /// Take one off the schedule for good.
    DeleteSegment {
        channel: UserId,
        segment: String,
    },
    /// Say the broadcaster is away between two times, or is not.
    SetVacation {
        channel: UserId,
        away: Option<(String, String)>,
    },
    /// When the next advertisement break is due, and pushing it back.
    ShowAds {
        channel: UserId,
    },
    SnoozeAd {
        channel: UserId,
    },
    /// Whisper somebody. Twitch wants a verified phone number on this
    /// account before it will carry one.
    SendWhisper {
        user: Login,
        text: String,
    },
    /// Clip what just happened in the channel. Answered with a notice
    /// carrying the URL where the clip is trimmed and titled.
    CreateClip {
        channel: UserId,
    },
    /// What rewards the channel offers, answered with
    /// [`crate::Event::Rewards`].
    ShowRewards {
        channel: UserId,
    },
    /// Add a reward this client will then be able to answer for.
    CreateReward {
        channel: UserId,
        title: String,
        cost: u64,
        /// What to ask the viewer, or empty to ask nothing.
        prompt: String,
    },
    /// Change one: only the fields given are touched. Twitch allows this
    /// only for rewards this client made, like answering them.
    UpdateReward {
        channel: UserId,
        reward: String,
        title: Option<String>,
        cost: Option<u64>,
        prompt: Option<String>,
        enabled: Option<bool>,
        paused: Option<bool>,
    },
    /// Remove one. Twitch allows this only for rewards this client made.
    DeleteReward {
        channel: UserId,
        reward: String,
    },
    /// Answer a redemption: fulfilled, or refunded.
    AnswerRedemption {
        channel: UserId,
        reward: String,
        redemption: String,
        fulfil: bool,
    },
    /// Ask what is known about one person; answered with
    /// [`crate::Event::UserCard`].
    ShowUser {
        channel: UserId,
        user: Login,
    },
    /// Answer someone's request to be unbanned.
    ResolveUnban {
        channel: UserId,
        request: String,
        allow: bool,
    },
    /// Let a held message through, or throw it away.
    ResolveHeld {
        channel: UserId,
        message: MessageId,
        allow: bool,
    },

    /// Ask VirusTotal about a link from chat. The answer comes as
    /// [`crate::Event::LinkReport`], also for a link already checked, and
    /// the command itself succeeds at once. Without `scan` only an existing
    /// report is looked up; with it, a link VirusTotal does not know is
    /// submitted for scanning, which makes it visible to VirusTotal's
    /// customers.
    CheckLink {
        url: String,
        #[serde(default)]
        scan: bool,
    },

    /// The channels this account moderates, answered with
    /// [`crate::Event::ModeratedChannels`].
    ShowModeratedChannels,

    /// The highlight words and the highlight, ignore and filter rules,
    /// as just saved in the settings, with the address book their
    /// categories come from: they apply to what arrives from now on.
    /// Refused, and nothing changed, when a rule or a name cannot be read.
    SetRules {
        #[serde(default)]
        highlight_words: Vec<String>,
        #[serde(default)]
        highlight: Vec<String>,
        #[serde(default)]
        ignore: Vec<String>,
        #[serde(default)]
        filter: Vec<String>,
        /// Name (a login, or `#channel`) to its categories.
        #[serde(default)]
        addressbook: std::collections::BTreeMap<String, Vec<String>>,
        /// When a message counts as repeated.
        #[serde(default)]
        repeated: crate::model::RepeatedConfig,
    },

    /// The channels this account follows and which of them stream,
    /// answered with [`crate::Event::FollowedChannels`].
    ShowFollowedChannels,

    /// Follow a link's redirects from this machine, to say where it really
    /// leads, without opening it. The answer comes as
    /// [`crate::Event::LinkReport`] with the `Redirects` source. Every host
    /// on the way sees the reader's address, which the settings say.
    FollowLink {
        url: String,
    },

    /// Start one of the bots; see `docs/bots.md`.
    StartBot {
        name: String,
    },
    /// Stop it: its input is closed, and it is killed if it does not end.
    StopBot {
        name: String,
    },
    /// The bots, as just saved in the settings. Bots no longer in the
    /// list are stopped; one that changed takes the change when it next
    /// starts. Refused, and nothing changed, when two have one name or
    /// one has no command.
    SetBots {
        bots: Vec<crate::model::BotConfig>,
    },

    /// Stop the engine. In daemon mode this ends the daemon for all
    /// frontends; a frontend that only wants to leave just disconnects.
    Shutdown,
}

/// Why a command failed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandError {
    pub kind: ErrorKind,
    /// Human-readable detail in English, suitable for showing as it is.
    pub message: String,
    /// What this message is, e.g. `channel.unknown`, for a frontend to say
    /// it in the user's language. Empty when the text comes from Twitch,
    /// which is not ours to translate.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub code: String,
    /// Named values the translated text needs, e.g. `channel`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<(String, String)>,
}

impl CommandError {
    /// An error a frontend can translate: `code` names it, `args` fills it,
    /// and `message` is the English version for those that cannot.
    pub fn new(
        kind: ErrorKind,
        code: &str,
        args: &[(&str, &str)],
        message: impl Into<String>,
    ) -> Self {
        Self {
            kind,
            message: message.into(),
            code: code.to_owned(),
            args: args
                .iter()
                .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
                .collect(),
        }
    }

    /// Text that is not ours: Twitch's own wording, passed through.
    pub fn passthrough(kind: ErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            code: String::new(),
            args: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorKind {
    NotLoggedIn,
    /// The account lacks the OAuth scope the action needs; logging in again
    /// with more access fixes it.
    MissingScope,
    NotJoined,
    InvalidArgument,
    /// Twitch refused the request (rate limit, permissions, ...).
    Twitch,
    Network,
    Internal,
    #[serde(other)]
    Unknown,
}

/// Result of a command as delivered to the frontend that sent it.
pub type CommandResult = Result<(), CommandError>;

/// What to do with the prediction that is running.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PredictionEnd {
    /// No more betting; the outcome is not known yet.
    Lock,
    /// This outcome was right: pay the people who took it.
    Resolve { outcome: String },
    /// Called off, with everybody's points returned.
    Cancel,
}

/// Splits what follows `/join`: channels separated by commas or spaces,
/// and `[category ...]` kept whole, since it has spaces of its own. The
/// engine expands a category from the address book; frontends split with
/// this too, so a `/join` typed anywhere means the same.
pub fn join_targets(args: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut depth = 0usize;
    for c in args.chars() {
        match c {
            '[' => {
                if depth == 0 && !current.is_empty() {
                    out.push(std::mem::take(&mut current));
                }
                depth += 1;
                current.push(c);
            }
            ']' if depth > 0 => {
                depth -= 1;
                current.push(c);
                if depth == 0 {
                    out.push(std::mem::take(&mut current));
                }
            }
            c if depth == 0 && (c == ',' || c.is_whitespace()) => {
                if !current.is_empty() {
                    out.push(std::mem::take(&mut current));
                }
            }
            c => current.push(c),
        }
    }
    if !current.trim().is_empty() {
        out.push(current.trim().to_owned());
    }
    out
}

#[cfg(test)]
mod join_tests {
    use super::join_targets;

    #[test]
    fn a_category_in_brackets_stays_one_target() {
        assert_eq!(
            join_targets("#a, b [vip #],c [mod]"),
            ["#a", "b", "[vip #]", "c", "[mod]"]
        );
        assert_eq!(join_targets("  "), Vec::<String>::new());
        assert_eq!(join_targets("[open"), ["[open"]);
    }
}
