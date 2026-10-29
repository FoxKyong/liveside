//! Turning EventSub payloads into our own model.
//!
//! Field names and shapes follow Twitch's EventSub reference (checked
//! 2026-09-22). Unknown values must never break parsing: Twitch adds message
//! types, fragment types and notice types over time.

use liveside_proto::model::{
    Badge, Channel, ChatMessage, EmoteProvider, EmoteRef, Fragment, Line, LineContent, LineFlags,
    MessageKind, ReplyTo, RoomModes, SharedSource, User,
};
use liveside_proto::{Rgb, Timestamp, UserId};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct ChatMessageEvent {
    pub broadcaster_user_id: String,
    pub chatter_user_id: String,
    pub chatter_user_name: String,
    pub chatter_user_login: String,
    pub message_id: String,
    pub message: MessagePayload,
    #[serde(default)]
    pub message_type: Option<String>,
    #[serde(default)]
    pub badges: Vec<BadgePayload>,
    #[serde(default)]
    pub cheer: Option<CheerPayload>,
    #[serde(default)]
    pub color: String,
    #[serde(default)]
    pub reply: Option<ReplyPayload>,
    #[serde(default)]
    pub source_broadcaster_user_id: Option<String>,
    #[serde(default)]
    pub source_broadcaster_user_login: Option<String>,
    #[serde(default)]
    pub source_broadcaster_user_name: Option<String>,
    #[serde(default)]
    pub source_message_id: Option<String>,
    #[serde(default)]
    pub source_badges: Option<Vec<BadgePayload>>,
}

#[derive(Debug, Deserialize)]
pub struct MessagePayload {
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub fragments: Vec<FragmentPayload>,
}

#[derive(Debug, Deserialize)]
pub struct FragmentPayload {
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub emote: Option<EmotePayload>,
    #[serde(default)]
    pub cheermote: Option<CheermotePayload>,
    #[serde(default)]
    pub mention: Option<MentionPayload>,
    #[serde(default)]
    pub gif: Option<GifPayload>,
}

#[derive(Debug, Deserialize)]
pub struct EmotePayload {
    pub id: String,
    #[serde(default)]
    pub format: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub struct CheermotePayload {
    pub prefix: String,
    pub bits: u32,
    pub tier: u32,
}

#[derive(Debug, Deserialize)]
pub struct MentionPayload {
    pub user_id: String,
    pub user_name: String,
    pub user_login: String,
}

#[derive(Debug, Deserialize)]
pub struct GifPayload {
    pub id: String,
    pub url: String,
}

#[derive(Debug, Deserialize)]
pub struct BadgePayload {
    pub set_id: String,
    pub id: String,
    #[serde(default)]
    pub info: String,
}

#[derive(Debug, Deserialize)]
pub struct CheerPayload {
    pub bits: u32,
}

#[derive(Debug, Deserialize)]
pub struct ReplyPayload {
    pub parent_message_id: String,
    #[serde(default)]
    pub parent_message_body: String,
    pub parent_user_id: String,
    pub parent_user_name: String,
    pub parent_user_login: String,
    pub thread_message_id: String,
}

#[derive(Debug, Deserialize)]
pub struct MessageDeleteEvent {
    pub broadcaster_user_id: String,
    pub message_id: String,
}

#[derive(Debug, Deserialize)]
pub struct ChatClearEvent {
    pub broadcaster_user_id: String,
}

#[derive(Debug, Deserialize)]
pub struct ClearUserMessagesEvent {
    pub broadcaster_user_id: String,
    pub target_user_id: String,
}

/// The channel's chat restrictions changed. Field names differ from the
/// Helix ones for the same settings.
#[derive(Debug, Deserialize)]
pub struct ChatSettingsEvent {
    pub broadcaster_user_id: String,
    #[serde(default)]
    pub emote_mode: bool,
    #[serde(default)]
    pub follower_mode: bool,
    #[serde(default)]
    pub follower_mode_duration_minutes: Option<u32>,
    #[serde(default)]
    pub slow_mode: bool,
    #[serde(default)]
    pub slow_mode_wait_time_seconds: Option<u32>,
    #[serde(default)]
    pub subscriber_mode: bool,
    #[serde(default)]
    pub unique_chat_mode: bool,
}

impl ChatSettingsEvent {
    pub fn to_modes(&self) -> RoomModes {
        RoomModes {
            emote_only: self.emote_mode,
            followers_only: self
                .follower_mode
                .then(|| self.follower_mode_duration_minutes.unwrap_or(0)),
            subscribers_only: self.subscriber_mode,
            slow: self
                .slow_mode
                .then(|| self.slow_mode_wait_time_seconds.unwrap_or(0)),
            unique_chat: self.unique_chat_mode,
        }
    }
}

/// What happens in a channel besides chat: someone follows, the stream
/// starts, the title changes, an advertisement runs. One shape covers
/// them all, since every one of these events names the channel and,
/// between them, they carry only a handful of fields.
#[derive(Debug, Default, Deserialize)]
pub struct ChannelEvent {
    pub broadcaster_user_id: String,
    /// Who did it: the follower, the moderator who turned Shield Mode on,
    /// whoever the shoutout points at.
    #[serde(default)]
    pub user_name: String,
    #[serde(default)]
    pub moderator_user_name: String,
    #[serde(default)]
    pub to_broadcaster_user_name: String,
    #[serde(default)]
    pub from_broadcaster_user_name: String,
    /// The channel's own state, for `channel.update`.
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub category_name: String,
    /// Viewers a shoutout carried.
    #[serde(default)]
    pub viewer_count: Option<u32>,
    /// How long an advertisement runs. Twitch sends this as a string in
    /// its own examples and as a number in practice, so it is read as
    /// whatever arrives.
    #[serde(default)]
    pub duration_seconds: Option<serde_json::Value>,
}

impl ChannelEvent {
    pub fn channel(&self) -> UserId {
        UserId(self.broadcaster_user_id.clone())
    }

    /// The advertisement's length in seconds, whichever way it was sent.
    pub fn seconds(&self) -> u32 {
        match &self.duration_seconds {
            Some(serde_json::Value::Number(n)) => n.as_u64().unwrap_or(0) as u32,
            Some(serde_json::Value::String(s)) => s.parse().unwrap_or(0),
            _ => 0,
        }
    }
}

/// One moderator action in a channel, as `channel.moderate` reports it.
/// Twitch sends the kind in `action` and fills in exactly one of the
/// objects below; the rest are null.
#[derive(Debug, Deserialize)]
pub struct ModerateEvent {
    pub broadcaster_user_id: String,
    #[serde(default)]
    pub moderator_user_name: String,
    #[serde(default)]
    pub action: String,
    #[serde(default)]
    pub ban: Option<ModerateTarget>,
    #[serde(default)]
    pub unban: Option<ModerateTarget>,
    #[serde(default)]
    pub timeout: Option<ModerateTarget>,
    #[serde(default)]
    pub untimeout: Option<ModerateTarget>,
    #[serde(default)]
    pub delete: Option<ModerateTarget>,
    #[serde(default)]
    pub warn: Option<ModerateTarget>,
    #[serde(default)]
    pub r#mod: Option<ModerateTarget>,
    #[serde(default)]
    pub unmod: Option<ModerateTarget>,
    #[serde(default)]
    pub vip: Option<ModerateTarget>,
    #[serde(default)]
    pub unvip: Option<ModerateTarget>,
    #[serde(default)]
    pub raid: Option<ModerateTarget>,
    #[serde(default)]
    pub slow: Option<ModerateDuration>,
    #[serde(default)]
    pub followers: Option<ModerateDuration>,
}

/// How long a restriction makes people wait.
#[derive(Debug, Default, Deserialize)]
pub struct ModerateDuration {
    #[serde(default)]
    pub wait_time_seconds: Option<u32>,
    #[serde(default)]
    pub follow_duration_minutes: Option<u32>,
}

/// Whoever an action was aimed at, and what was said about it. The
/// objects differ in which fields they carry; the ones missing are simply
/// empty here.
#[derive(Debug, Default, Deserialize)]
pub struct ModerateTarget {
    #[serde(default)]
    pub user_name: String,
    #[serde(default)]
    pub reason: Option<String>,
    #[serde(default)]
    pub expires_at: Option<String>,
    /// What was deleted, for `delete`.
    #[serde(default)]
    pub message_body: Option<String>,
    /// How many came along, for `raid`.
    #[serde(default)]
    pub viewer_count: Option<u32>,
}

impl ModerateEvent {
    pub fn channel(&self) -> UserId {
        UserId(self.broadcaster_user_id.clone())
    }

    /// The code a frontend words, and the values it fills in. Actions
    /// this version has never heard of still say who did what, because
    /// the action's own name is passed through.
    pub fn to_notice(&self) -> (&'static str, Vec<(&'static str, String)>) {
        let who = |target: &Option<ModerateTarget>| {
            target
                .as_ref()
                .map(|t| t.user_name.clone())
                .unwrap_or_default()
        };
        let because = |target: &Option<ModerateTarget>| match target
            .as_ref()
            .and_then(|t| t.reason.clone())
        {
            Some(reason) if !reason.is_empty() => format!(" ({reason})"),
            _ => String::new(),
        };
        let mut args = vec![("moderator", self.moderator_user_name.clone())];
        let code = match self.action.as_str() {
            "ban" => {
                args.push(("user", who(&self.ban)));
                args.push(("detail", because(&self.ban)));
                "mod.ban"
            }
            "unban" => {
                args.push(("user", who(&self.unban)));
                "mod.unban"
            }
            "timeout" => {
                args.push(("user", who(&self.timeout)));
                args.push(("detail", because(&self.timeout)));
                "mod.timeout"
            }
            "untimeout" => {
                args.push(("user", who(&self.untimeout)));
                "mod.untimeout"
            }
            "delete" => {
                args.push(("user", who(&self.delete)));
                args.push((
                    "text",
                    self.delete
                        .as_ref()
                        .and_then(|t| t.message_body.clone())
                        .unwrap_or_default(),
                ));
                "mod.delete"
            }
            "warn" => {
                args.push(("user", who(&self.warn)));
                args.push(("detail", because(&self.warn)));
                "mod.warn"
            }
            "mod" => {
                args.push(("user", who(&self.r#mod)));
                "mod.mod"
            }
            "unmod" => {
                args.push(("user", who(&self.unmod)));
                "mod.unmod"
            }
            "vip" => {
                args.push(("user", who(&self.vip)));
                "mod.vip"
            }
            "unvip" => {
                args.push(("user", who(&self.unvip)));
                "mod.unvip"
            }
            "raid" => {
                args.push(("user", who(&self.raid)));
                "mod.raid"
            }
            // The restrictions, which name no one. Twitch's tokens for
            // these are the same words our own commands use.
            "emoteonly" | "emoteonlyoff" | "subscribers" | "subscribersoff" | "uniquechat"
            | "uniquechatoff" | "followersoff" | "slowoff" | "clear" => {
                return (
                    match self.action.as_str() {
                        "emoteonly" => "mod.emoteonly",
                        "emoteonlyoff" => "mod.emoteonlyoff",
                        "subscribers" => "mod.subscribers",
                        "subscribersoff" => "mod.subscribersoff",
                        "uniquechat" => "mod.uniquechat",
                        "uniquechatoff" => "mod.uniquechatoff",
                        "followersoff" => "mod.followersoff",
                        "slowoff" => "mod.slowoff",
                        _ => "mod.clear",
                    },
                    args,
                );
            }
            "slow" => {
                let seconds = self
                    .slow
                    .as_ref()
                    .and_then(|slow| slow.wait_time_seconds)
                    .unwrap_or(0);
                args.push(("detail", format!(" ({seconds} s)")));
                "mod.slow"
            }
            "followers" => {
                let minutes = self
                    .followers
                    .as_ref()
                    .and_then(|followers| followers.follow_duration_minutes)
                    .unwrap_or(0);
                args.push(("detail", format!(" ({minutes} min)")));
                "mod.followers"
            }
            // Whatever Twitch adds next: the action's own name is better
            // than silence.
            other => {
                args.push(("action", other.to_owned()));
                "mod.other"
            }
        };
        (code, args)
    }
}

/// Someone asking to be let back into a channel, or the answer they got.
#[derive(Debug, Deserialize)]
pub struct UnbanRequestEvent {
    pub broadcaster_user_id: String,
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub user_id: String,
    #[serde(default)]
    pub user_login: String,
    #[serde(default)]
    pub user_name: String,
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub moderator_user_name: String,
}

impl UnbanRequestEvent {
    pub fn channel(&self) -> UserId {
        UserId(self.broadcaster_user_id.clone())
    }

    pub fn to_request(&self, time: Timestamp) -> liveside_proto::event::UnbanRequest {
        liveside_proto::event::UnbanRequest {
            id: self.id.clone(),
            user: User {
                id: self.user_id.clone().into(),
                login: self.user_login.clone().into(),
                display_name: self.user_name.clone(),
            },
            text: self.text.clone(),
            time,
        }
    }
}

/// A poll, as the three `channel.poll.*` events carry it. The begin and
/// progress events have no status of their own; only the end says how it
/// finished.
#[derive(Debug, Deserialize)]
pub struct PollEvent {
    pub broadcaster_user_id: String,
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub choices: Vec<PollChoicePayload>,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub ends_at: String,
}

#[derive(Debug, Deserialize)]
pub struct PollChoicePayload {
    pub id: String,
    pub title: String,
    /// Every vote cast for this choice. Twitch counts the ones paid for
    /// with channel points separately as well, but inside this total —
    /// adding them would count them twice.
    #[serde(default)]
    pub votes: u32,
}

impl PollEvent {
    /// What this says in the protocol's own terms. `finished` is false
    /// for begin and progress, where Twitch sends no status at all.
    pub fn poll(&self, finished: bool) -> liveside_proto::model::Poll {
        use liveside_proto::model::{Poll, PollChoice, PollStatus};
        Poll {
            id: self.id.clone(),
            title: self.title.clone(),
            choices: self
                .choices
                .iter()
                .map(|choice| PollChoice {
                    id: choice.id.clone(),
                    title: choice.title.clone(),
                    votes: choice.votes,
                })
                .collect(),
            status: match (finished, self.status.as_str()) {
                (false, _) => PollStatus::Active,
                (_, "archived") | (_, "ARCHIVED") => PollStatus::Archived,
                (_, "terminated") | (_, "TERMINATED") => PollStatus::Terminated,
                _ => PollStatus::Completed,
            },
            ends_at: self.ends_at.clone(),
        }
    }
}

/// Somebody spending channel points on a custom reward.
#[derive(Debug, Deserialize)]
pub struct RedemptionEvent {
    pub broadcaster_user_id: String,
    pub id: String,
    #[serde(default)]
    pub user_id: String,
    #[serde(default)]
    pub user_login: String,
    #[serde(default)]
    pub user_name: String,
    /// What they typed, for a reward that asks for something.
    #[serde(default)]
    pub user_input: String,
    /// `unfulfilled`, `fulfilled` or `canceled`.
    #[serde(default)]
    pub status: String,
    pub reward: RedeemedReward,
}

#[derive(Debug, Deserialize)]
pub struct RedeemedReward {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub cost: u64,
}

/// Somebody Twitch does not trust, and what they just said.
#[derive(Debug, Deserialize)]
pub struct SuspiciousEvent {
    pub broadcaster_user_id: String,
    #[serde(default)]
    pub user_id: String,
    #[serde(default)]
    pub user_login: String,
    #[serde(default)]
    pub user_name: String,
    /// Who decided, in `channel.suspicious_user.update`. The message
    /// event has no moderator: Twitch flagged the message itself.
    #[serde(default)]
    pub moderator_user_name: String,
    /// `none`, `active_monitoring` or `restricted`.
    #[serde(default)]
    pub low_trust_status: String,
    #[serde(default)]
    pub types: Vec<String>,
    /// `unknown`, `possible` or `likely`.
    #[serde(default)]
    pub ban_evasion_evaluation: String,
    #[serde(default)]
    pub shared_ban_channel_ids: Vec<String>,
    #[serde(default)]
    pub message: Option<SuspiciousMessageText>,
}

#[derive(Debug, Deserialize)]
pub struct SuspiciousMessageText {
    #[serde(default)]
    pub message_id: String,
}

impl SuspiciousEvent {
    pub fn channel(&self) -> UserId {
        UserId(self.broadcaster_user_id.clone())
    }

    pub fn suspicion(&self) -> liveside_proto::model::Suspicion {
        use liveside_proto::model::{Suspicion, SuspicionStatus};
        Suspicion {
            status: match self.low_trust_status.as_str() {
                "active_monitoring" => SuspicionStatus::Monitored,
                "restricted" => SuspicionStatus::Restricted,
                _ => SuspicionStatus::None,
            },
            reasons: self.types.clone(),
            evasion: self.ban_evasion_evaluation.clone(),
            shared_bans: self.shared_ban_channel_ids.len() as u32,
        }
    }
}

/// A whisper, as `user.whisper.message` carries it.
#[derive(Debug, Deserialize)]
pub struct WhisperEvent {
    #[serde(default)]
    pub from_user_id: String,
    #[serde(default)]
    pub from_user_login: String,
    #[serde(default)]
    pub from_user_name: String,
    #[serde(default)]
    pub whisper_id: String,
    #[serde(default)]
    pub whisper: Option<WhisperText>,
}

#[derive(Debug, Deserialize)]
pub struct WhisperText {
    #[serde(default)]
    pub text: String,
}

/// A Shared Chat session: which channels are in it.
#[derive(Debug, Deserialize)]
pub struct SharedChatEvent {
    pub broadcaster_user_id: String,
    #[serde(default)]
    pub host_broadcaster_user_name: String,
    #[serde(default)]
    pub participants: Vec<SharedParticipant>,
}

#[derive(Debug, Deserialize)]
pub struct SharedParticipant {
    #[serde(default)]
    pub broadcaster_user_name: String,
}

impl SharedChatEvent {
    pub fn channel(&self) -> UserId {
        UserId(self.broadcaster_user_id.clone())
    }

    /// Everybody in it but this channel, since the reader knows where
    /// they are.
    pub fn others(&self, me: &str) -> String {
        self.participants
            .iter()
            .map(|p| p.broadcaster_user_name.as_str())
            .filter(|name| !name.eq_ignore_ascii_case(me) && !name.is_empty())
            .collect::<Vec<_>>()
            .join(", ")
    }
}

/// What people pay with: a subscription, a gift, bits, or something
/// counted towards a Hype Train, a goal or a charity campaign. One shape
/// for all of them, because every field Twitch does not send is absent
/// rather than different.
#[derive(Debug, Deserialize)]
pub struct MoneyEvent {
    pub broadcaster_user_id: String,
    #[serde(default)]
    pub user_name: String,
    /// `1000`, `2000`, `3000`. Twitch counts in thousands.
    #[serde(default)]
    pub tier: String,
    #[serde(default)]
    pub is_gift: bool,
    #[serde(default)]
    pub is_anonymous: bool,
    /// Gifts in this one go, and in total from this person.
    #[serde(default)]
    pub total: u32,
    #[serde(default)]
    pub cumulative_months: u32,
    #[serde(default)]
    pub streak_months: Option<u32>,
    /// Bits cheered.
    #[serde(default)]
    pub bits: u64,
    /// What was said with it. Twitch sends a string for a cheer and an
    /// object for a resubscription, so it is read as whatever arrives.
    #[serde(default)]
    pub message: Option<serde_json::Value>,
    /// A Hype Train's level, and what it has got to.
    #[serde(default)]
    pub level: u32,
    #[serde(default)]
    pub total_hype: Option<u64>,
    /// A goal: what it is, and where it stands.
    #[serde(rename = "type", default)]
    pub goal_kind: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub current_amount: Option<serde_json::Value>,
    #[serde(default)]
    pub target_amount: Option<serde_json::Value>,
    #[serde(default)]
    pub is_achieved: bool,
    /// A charity campaign.
    #[serde(default)]
    pub charity_name: String,
    #[serde(default)]
    pub amount: Option<MoneyAmount>,
}

/// Money as Twitch counts it: whole units with the point written down
/// separately, so 12.34 EUR arrives as 1234 with two decimal places.
#[derive(Debug, Deserialize)]
pub struct MoneyAmount {
    #[serde(default)]
    pub value: i64,
    #[serde(default)]
    pub decimal_places: u32,
    #[serde(default)]
    pub currency: String,
}

impl MoneyEvent {
    pub fn channel(&self) -> UserId {
        UserId(self.broadcaster_user_id.clone())
    }

    /// The tier as a person says it: 1, 2 or 3.
    pub fn tier_number(&self) -> String {
        match self.tier.as_str() {
            "2000" => "2".to_owned(),
            "3000" => "3".to_owned(),
            _ => "1".to_owned(),
        }
    }

    /// What was said with it, whichever shape Twitch used.
    pub fn said(&self) -> String {
        match &self.message {
            Some(serde_json::Value::String(text)) => text.clone(),
            Some(serde_json::Value::Object(map)) => map
                .get("text")
                .and_then(|text| text.as_str())
                .unwrap_or_default()
                .to_owned(),
            _ => String::new(),
        }
    }

    /// An amount that may be a number or an object with a decimal point.
    pub fn money(value: Option<&serde_json::Value>) -> String {
        match value {
            Some(serde_json::Value::Number(n)) => n.to_string(),
            Some(serde_json::Value::Object(map)) => {
                let amount = map.get("value").and_then(|v| v.as_i64()).unwrap_or(0);
                let places = map
                    .get("decimal_places")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0) as u32;
                let currency = map
                    .get("currency")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default();
                let divisor = 10_i64.pow(places);
                match places {
                    0 => format!("{amount} {currency}").trim().to_owned(),
                    _ => format!(
                        "{}.{:0width$} {currency}",
                        amount / divisor,
                        (amount % divisor).abs(),
                        width = places as usize
                    )
                    .trim()
                    .to_owned(),
                }
            }
            _ => String::new(),
        }
    }
}

/// A prediction, as the four `channel.prediction.*` events carry it.
#[derive(Debug, Deserialize)]
pub struct PredictionEvent {
    pub broadcaster_user_id: String,
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub outcomes: Vec<PredictionOutcomePayload>,
    /// Only on the end event: `resolved` or `canceled`.
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub locks_at: String,
    #[serde(default)]
    pub winning_outcome_id: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct PredictionOutcomePayload {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub users: u32,
    #[serde(default)]
    pub channel_points: u64,
}

impl PredictionEvent {
    /// What this says in the protocol's own terms. `locked` and the
    /// status carry what the event kind means: begin and progress say
    /// nothing about state, lock says only that betting is closed, and
    /// end says how it finished.
    pub fn prediction(&self, locked: bool, finished: bool) -> liveside_proto::model::Prediction {
        use liveside_proto::model::{Prediction, PredictionOutcome, PredictionStatus};
        Prediction {
            id: self.id.clone(),
            title: self.title.clone(),
            outcomes: self
                .outcomes
                .iter()
                .map(|outcome| PredictionOutcome {
                    id: outcome.id.clone(),
                    title: outcome.title.clone(),
                    users: outcome.users,
                    points: outcome.channel_points,
                })
                .collect(),
            status: match (finished, locked, self.status.as_str()) {
                (true, _, "canceled") | (true, _, "CANCELED") => PredictionStatus::Canceled,
                (true, _, _) => PredictionStatus::Resolved,
                (_, true, _) => PredictionStatus::Locked,
                _ => PredictionStatus::Active,
            },
            locks_at: self.locks_at.clone(),
            winner: self.winning_outcome_id.clone(),
        }
    }
}

/// A message AutoMod or a blocked term caught, and what became of it.
#[derive(Debug, Deserialize)]
pub struct HoldEvent {
    pub broadcaster_user_id: String,
    pub user_id: String,
    pub user_login: String,
    pub user_name: String,
    #[serde(default)]
    pub message_id: String,
    #[serde(default)]
    pub message: Option<MessagePayload>,
    #[serde(default)]
    pub reason: String,
    #[serde(default)]
    pub automod: Option<AutomodDetail>,
    #[serde(default)]
    pub blocked_term: Option<BlockedTermDetail>,
    /// Only on an update: what was decided.
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub moderator_user_name: String,
}

#[derive(Debug, Deserialize)]
pub struct AutomodDetail {
    #[serde(default)]
    pub category: String,
    #[serde(default)]
    pub level: u8,
}

#[derive(Debug, Deserialize)]
pub struct BlockedTermDetail {
    #[serde(default)]
    pub terms_found: Vec<TermFound>,
}

#[derive(Debug, Deserialize)]
pub struct TermFound {
    #[serde(default)]
    pub term_id: String,
}

impl HoldEvent {
    /// What a frontend needs to judge the message by.
    pub fn to_held(&self, time: Timestamp) -> liveside_proto::event::HeldMessage {
        // Twitch says either which AutoMod category caught it or which
        // blocked term it matched; neither is always there.
        let detail = match (&self.automod, &self.blocked_term) {
            (Some(automod), _) if !automod.category.is_empty() => {
                format!("{} {}", automod.category, automod.level)
            }
            (_, Some(blocked)) if !blocked.terms_found.is_empty() => "blocked term".to_owned(),
            _ => String::new(),
        };
        liveside_proto::event::HeldMessage {
            id: self.message_id.clone().into(),
            sender: User {
                id: self.user_id.clone().into(),
                login: self.user_login.clone().into(),
                display_name: self.user_name.clone(),
            },
            text: self
                .message
                .as_ref()
                .map(|message| message.text.clone())
                .unwrap_or_default(),
            reason: self.reason.clone(),
            detail,
            time,
        }
    }
}

/// Subs, resubs, raids, announcements and the like.
#[derive(Debug, Deserialize)]
pub struct NotificationEvent {
    pub broadcaster_user_id: String,
    #[serde(default)]
    pub system_message: String,
    #[serde(default)]
    pub message: Option<MessagePayload>,
}

impl ChatMessageEvent {
    pub fn channel(&self) -> UserId {
        UserId(self.broadcaster_user_id.clone())
    }

    /// Builds the chat line. `own_user_id` marks our own messages.
    pub fn to_line(&self, time: Timestamp, own_user_id: Option<&str>) -> Line {
        let sender = User {
            id: self.chatter_user_id.clone().into(),
            login: self.chatter_user_login.clone().into(),
            display_name: self.chatter_user_name.clone(),
        };
        let message = ChatMessage {
            id: self.message_id.clone().into(),
            sender,
            color: Rgb::parse_hex(&self.color),
            badges: badges(&self.badges),
            fragments: without_action(fragments(&self.message)),
            // EventSub has no field for it: a `/me` comes as IRC's
            // CTCP ACTION, wrapped around the text (and only the text;
            // the fragments are without it).
            kind: if self.message.text.starts_with(ACTION) {
                MessageKind::Action
            } else {
                message_kind(self.message_type.as_deref())
            },
            bits: self.cheer.as_ref().map(|cheer| cheer.bits),
            reply: self.reply.as_ref().map(|reply| {
                Box::new(ReplyTo {
                    parent_id: reply.parent_message_id.clone().into(),
                    parent_user: User {
                        id: reply.parent_user_id.clone().into(),
                        login: reply.parent_user_login.clone().into(),
                        display_name: reply.parent_user_name.clone(),
                    },
                    parent_text: reply.parent_message_body.clone(),
                    thread_id: reply.thread_message_id.clone().into(),
                })
            }),
            shared_source: self.shared_source(),
        };
        let own = own_user_id.is_some_and(|id| id == self.chatter_user_id);
        Line {
            time,
            content: LineContent::Message(message),
            flags: LineFlags {
                own,
                ..Default::default()
            },
        }
    }

    /// Set only for messages that reached us through a Shared Chat session in
    /// another broadcaster's channel.
    fn shared_source(&self) -> Option<Box<SharedSource>> {
        let id = self.source_broadcaster_user_id.as_ref()?;
        Some(Box::new(SharedSource {
            channel: Channel {
                id: id.clone().into(),
                login: self
                    .source_broadcaster_user_login
                    .clone()
                    .unwrap_or_default()
                    .into(),
                display_name: self
                    .source_broadcaster_user_name
                    .clone()
                    .unwrap_or_default(),
            },
            message_id: self
                .source_message_id
                .clone()
                .unwrap_or_else(|| self.message_id.clone())
                .into(),
            badges: self
                .source_badges
                .as_deref()
                .map(badges)
                .unwrap_or_default(),
        }))
    }
}

impl NotificationEvent {
    pub fn to_line(&self, time: Timestamp) -> Line {
        Line {
            time,
            content: LineContent::UserNotice {
                system_text: self.system_message.clone(),
                // The user's own text, if they attached one. Rendered next to
                // the system message; no sender data is needed for that.
                message: self.message.as_ref().and_then(|message| {
                    let fragments = fragments(message);
                    if fragments.is_empty() {
                        return None;
                    }
                    Some(ChatMessage {
                        id: String::new().into(),
                        sender: User {
                            id: String::new().into(),
                            login: String::new().into(),
                            display_name: String::new(),
                        },
                        color: None,
                        badges: Vec::new(),
                        fragments,
                        kind: MessageKind::Text,
                        bits: None,
                        reply: None,
                        shared_source: None,
                    })
                }),
            },
            flags: LineFlags::default(),
        }
    }
}

fn badges(badges: &[BadgePayload]) -> Vec<Badge> {
    badges
        .iter()
        .map(|badge| Badge {
            set_id: badge.set_id.clone(),
            id: badge.id.clone(),
            info: badge.info.clone(),
            // Filled in by the backend, which holds the catalogue.
            image: String::new(),
            title: String::new(),
        })
        .collect()
}

/// How a `/me` message's text begins.
const ACTION: &str = "\u{1}ACTION ";

/// Takes the ACTION wrapper off the text, where it shows: the fragments
/// come without it, but text built from a payload without fragments, or a
/// payload Twitch changes, would show it.
fn without_action(mut fragments: Vec<Fragment>) -> Vec<Fragment> {
    if let Some(Fragment::Text { text }) = fragments.first_mut()
        && let Some(rest) = text.strip_prefix(ACTION)
    {
        *text = rest.to_owned();
    }
    if let Some(Fragment::Text { text }) = fragments.last_mut()
        && let Some(rest) = text.strip_suffix('\u{1}')
    {
        *text = rest.to_owned();
    }
    fragments.retain(|f| !matches!(f, Fragment::Text { text } if text.is_empty()));
    fragments
}

fn message_kind(kind: Option<&str>) -> MessageKind {
    match kind {
        Some("text") | None => MessageKind::Text,
        Some("channel_points_highlighted") => MessageKind::ChannelPointsHighlighted,
        Some("channel_points_sub_only") => MessageKind::ChannelPointsSubOnly,
        Some("user_intro") => MessageKind::UserIntro,
        Some("power_ups_message_effect") => MessageKind::PowerUpsMessageEffect,
        Some("power_ups_gigantified_emote") => MessageKind::PowerUpsGigantifiedEmote,
        Some(_) => MessageKind::Unknown,
    }
}

fn fragments(message: &MessagePayload) -> Vec<Fragment> {
    if message.fragments.is_empty() {
        if message.text.is_empty() {
            return Vec::new();
        }
        // Older payloads and notices may only carry plain text.
        return vec![Fragment::Text {
            text: message.text.clone(),
        }];
    }
    message
        .fragments
        .iter()
        .map(|fragment| match fragment.kind.as_str() {
            "emote" => match &fragment.emote {
                Some(emote) => Fragment::Emote {
                    emote: EmoteRef {
                        provider: EmoteProvider::Twitch,
                        id: emote.id.clone(),
                        code: fragment.text.clone(),
                        animated: emote.format.iter().any(|f| f == "animated"),
                        zero_width: false,
                    },
                },
                None => text(fragment),
            },
            "mention" => match &fragment.mention {
                Some(mention) => Fragment::Mention {
                    text: fragment.text.clone(),
                    user: User {
                        id: mention.user_id.clone().into(),
                        login: mention.user_login.clone().into(),
                        display_name: mention.user_name.clone(),
                    },
                },
                None => text(fragment),
            },
            "cheermote" => match &fragment.cheermote {
                Some(cheermote) => Fragment::Cheermote {
                    text: fragment.text.clone(),
                    prefix: cheermote.prefix.clone(),
                    bits: cheermote.bits,
                    tier: cheermote.tier,
                },
                None => text(fragment),
            },
            "gif" => match &fragment.gif {
                Some(gif) => Fragment::Gif {
                    text: fragment.text.clone(),
                    id: gif.id.clone(),
                    url: gif.url.clone(),
                },
                None => text(fragment),
            },
            // "text" and anything Twitch adds later.
            _ => text(fragment),
        })
        .collect()
}

fn text(fragment: &FragmentPayload) -> Fragment {
    Fragment::Text {
        text: fragment.text.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Shaped after the example in Twitch's EventSub reference.
    const MESSAGE: &str = r##"{
        "broadcaster_user_id": "1971641",
        "broadcaster_user_login": "streamer",
        "broadcaster_user_name": "Streamer",
        "chatter_user_id": "4145994",
        "chatter_user_login": "viewer32",
        "chatter_user_name": "Viewer32",
        "message_id": "cc106a89-1814-919d-454c-f4f2f970aae7",
        "message": {
            "text": "Hi @Streamer Kappa cheer100",
            "fragments": [
                { "type": "text", "text": "Hi ", "cheermote": null, "emote": null, "mention": null },
                { "type": "mention", "text": "@Streamer", "cheermote": null, "emote": null,
                  "mention": { "user_id": "1971641", "user_name": "Streamer", "user_login": "streamer" } },
                { "type": "text", "text": " ", "cheermote": null, "emote": null, "mention": null },
                { "type": "emote", "text": "Kappa", "cheermote": null, "mention": null,
                  "emote": { "id": "25", "emote_set_id": "0", "owner_id": "0", "format": ["static", "animated"] } },
                { "type": "text", "text": " ", "cheermote": null, "emote": null, "mention": null },
                { "type": "cheermote", "text": "cheer100", "emote": null, "mention": null,
                  "cheermote": { "prefix": "cheer", "bits": 100, "tier": 1 } }
            ]
        },
        "color": "#00FF7F",
        "badges": [
            { "set_id": "moderator", "id": "1", "info": "" },
            { "set_id": "subscriber", "id": "12", "info": "16" }
        ],
        "message_type": "text",
        "cheer": { "bits": 100 },
        "reply": {
            "parent_message_id": "parent-id",
            "parent_message_body": "hey",
            "parent_user_id": "1971641",
            "parent_user_name": "Streamer",
            "parent_user_login": "streamer",
            "thread_message_id": "thread-id",
            "thread_user_id": "1971641",
            "thread_user_name": "Streamer",
            "thread_user_login": "streamer"
        },
        "channel_points_custom_reward_id": null,
        "source_broadcaster_user_id": null
    }"##;

    fn parse(json: &str) -> ChatMessageEvent {
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn maps_a_full_message() {
        let event = parse(MESSAGE);
        assert_eq!(event.channel(), UserId("1971641".into()));
        let line = event.to_line(Timestamp(5), Some("4145994"));
        assert!(line.flags.own, "we sent it");

        let LineContent::Message(message) = line.content else {
            panic!("expected a message");
        };
        assert_eq!(message.plain_text(), "Hi @Streamer Kappa cheer100");
        assert_eq!(message.sender.display_name, "Viewer32");
        assert_eq!(message.color, Rgb::parse_hex("#00ff7f"));
        assert_eq!(message.bits, Some(100));
        assert_eq!(message.badges.len(), 2);
        assert_eq!(message.badges[1].info, "16");
        assert_eq!(message.reply.unwrap().parent_text, "hey");
        assert!(message.shared_source.is_none());

        match &message.fragments[3] {
            Fragment::Emote { emote } => {
                assert_eq!(emote.id, "25");
                assert_eq!(emote.code, "Kappa");
                assert!(emote.animated);
            }
            other => panic!("expected an emote, got {other:?}"),
        }
        match &message.fragments[5] {
            Fragment::Cheermote { prefix, bits, .. } => {
                assert_eq!(prefix, "cheer");
                assert_eq!(*bits, 100);
            }
            other => panic!("expected a cheermote, got {other:?}"),
        }
    }

    /// Shaped after an example posted on Twitch's developer forum when
    /// chat came to EventSub (2024-01-25): the text keeps the CTCP
    /// wrapper, the fragments do not.
    const ACTION_MESSAGE: &str = r##"{
        "broadcaster_user_id": "15185913",
        "broadcaster_user_login": "barrycarlyon",
        "broadcaster_user_name": "BarryCarlyon",
        "chatter_user_id": "15185913",
        "chatter_user_login": "barrycarlyon",
        "chatter_user_name": "BarryCarlyon",
        "message_id": "24b24337-e1df-457f-9672-be5f619b0028",
        "message": {"text": "\u0001ACTION chat test\u0001",
                    "fragments": [{"type": "text", "text": "chat test",
                                   "cheermote": null, "emote": null, "mention": null}]},
        "color": "#033700",
        "badges": [],
        "message_type": "text"
    }"##;

    #[test]
    fn a_me_message_is_an_action_without_its_wrapper() {
        let line = parse(ACTION_MESSAGE).to_line(Timestamp(5), None);
        let LineContent::Message(message) = line.content else {
            panic!("expected a message");
        };
        assert_eq!(message.kind, MessageKind::Action);
        assert_eq!(message.plain_text(), "chat test");
        // Were the wrapper in the fragments, it would still come off.
        let wrapped = without_action(vec![Fragment::Text {
            text: "\u{1}ACTION waves\u{1}".into(),
        }]);
        assert_eq!(
            wrapped,
            vec![Fragment::Text {
                text: "waves".into()
            }]
        );
        let line = parse(MESSAGE).to_line(Timestamp(5), None);
        let LineContent::Message(message) = line.content else {
            panic!("expected a message");
        };
        assert_eq!(
            message.kind,
            MessageKind::Text,
            "an ordinary message stays one"
        );
    }

    #[test]
    fn foreign_messages_are_not_own() {
        let line = parse(MESSAGE).to_line(Timestamp(5), Some("999"));
        assert!(!line.flags.own);
    }

    #[test]
    fn tolerates_unknown_and_missing_fields() {
        let json = r#"{
            "broadcaster_user_id": "1",
            "chatter_user_id": "2",
            "chatter_user_login": "someone",
            "chatter_user_name": "Someone",
            "message_id": "m1",
            "message": { "text": "hello", "fragments": [
                { "type": "future_thing", "text": "hello" }
            ]},
            "message_type": "brand_new_type",
            "color": "",
            "unexpected_field": { "nested": true }
        }"#;
        let event = parse(json);
        let line = event.to_line(Timestamp(0), None);
        let LineContent::Message(message) = line.content else {
            panic!("expected a message");
        };
        assert_eq!(message.kind, MessageKind::Unknown);
        assert_eq!(message.color, None, "empty colour means unset");
        assert_eq!(message.plain_text(), "hello");
        assert!(message.badges.is_empty());
    }

    #[test]
    fn shared_chat_source_is_kept() {
        let json = MESSAGE.replace(
            r#""source_broadcaster_user_id": null"#,
            r#""source_broadcaster_user_id": "42",
               "source_broadcaster_user_login": "other",
               "source_broadcaster_user_name": "Other",
               "source_message_id": "src-1",
               "source_badges": [{"set_id": "vip", "id": "1", "info": ""}]"#,
        );
        let line = parse(&json).to_line(Timestamp(0), None);
        let LineContent::Message(message) = line.content else {
            panic!("expected a message");
        };
        let source = message.shared_source.expect("shared source");
        assert_eq!(source.channel.login.as_str(), "other");
        assert_eq!(source.message_id.as_str(), "src-1");
        assert_eq!(source.badges[0].set_id, "vip");
    }

    #[test]
    fn chat_settings_become_room_modes() {
        let json = r#"{
            "broadcaster_user_id": "1971641",
            "broadcaster_user_login": "streamer",
            "broadcaster_user_name": "Streamer",
            "emote_mode": false,
            "follower_mode": true,
            "follower_mode_duration_minutes": 30,
            "slow_mode": true,
            "slow_mode_wait_time_seconds": 10,
            "subscriber_mode": false,
            "unique_chat_mode": true
        }"#;
        let event: ChatSettingsEvent = serde_json::from_str(json).unwrap();
        let modes = event.to_modes();
        assert_eq!(modes.followers_only, Some(30));
        assert_eq!(modes.slow, Some(10));
        assert!(modes.unique_chat && !modes.emote_only && !modes.subscribers_only);

        // Off means the duration fields are null.
        let json = r#"{
            "broadcaster_user_id": "1",
            "follower_mode": false,
            "follower_mode_duration_minutes": null,
            "slow_mode": false,
            "slow_mode_wait_time_seconds": null
        }"#;
        let modes = serde_json::from_str::<ChatSettingsEvent>(json)
            .unwrap()
            .to_modes();
        assert_eq!(modes, RoomModes::default());
    }

    #[test]
    fn a_moderator_action_says_who_did_what_to_whom() {
        let ban: ModerateEvent = serde_json::from_value(serde_json::json!({
            "broadcaster_user_id": "1337",
            "moderator_user_name": "TheMod",
            "action": "ban",
            "ban": { "user_name": "Spammer", "reason": "spam" },
            "timeout": null,
        }))
        .expect("a ban");
        let (code, args) = ban.to_notice();
        assert_eq!(code, "mod.ban");
        let value = |name: &str| {
            args.iter()
                .find(|(key, _)| *key == name)
                .map(|(_, v)| v.clone())
                .unwrap_or_default()
        };
        assert_eq!(value("moderator"), "TheMod");
        assert_eq!(value("user"), "Spammer");
        assert_eq!(value("detail"), " (spam)");

        // A restriction, which names nobody.
        let emotes: ModerateEvent = serde_json::from_value(serde_json::json!({
            "broadcaster_user_id": "1337",
            "moderator_user_name": "TheMod",
            "action": "emoteonly",
        }))
        .expect("a room mode");
        assert_eq!(emotes.to_notice().0, "mod.emoteonly");

        // One that carries how long people wait.
        let slow: ModerateEvent = serde_json::from_value(serde_json::json!({
            "broadcaster_user_id": "1337",
            "moderator_user_name": "TheMod",
            "action": "slow",
            "slow": { "wait_time_seconds": 30 },
        }))
        .expect("slow mode");
        let (code, args) = slow.to_notice();
        assert_eq!(code, "mod.slow");
        assert!(args.iter().any(|(k, v)| *k == "detail" && v == " (30 s)"));

        // An action this version has never heard of still names itself.
        let future: ModerateEvent = serde_json::from_value(serde_json::json!({
            "broadcaster_user_id": "1337",
            "moderator_user_name": "TheMod",
            "action": "something_twitch_added_later",
        }))
        .expect("an unknown action");
        assert_eq!(future.to_notice().0, "mod.other");
    }

    #[test]
    fn a_held_message_says_who_wrote_it_and_why_it_was_caught() {
        let automod: HoldEvent = serde_json::from_value(serde_json::json!({
            "broadcaster_user_id": "1337",
            "user_id": "4242",
            "user_login": "baduser",
            "user_name": "BadUser",
            "message_id": "bad-message-id",
            "message": { "text": "this is a bad message", "fragments": [] },
            "reason": "automod",
            "automod": { "category": "aggressive", "level": 2 },
            "blocked_term": null,
        }))
        .expect("a held message");
        let held = automod.to_held(Timestamp::now());
        assert_eq!(held.sender.display_name, "BadUser");
        assert_eq!(held.text, "this is a bad message");
        assert_eq!(held.detail, "aggressive 2");

        // The other half of the queue: a term the channel blocked itself.
        let term: HoldEvent = serde_json::from_value(serde_json::json!({
            "broadcaster_user_id": "1337",
            "user_id": "4242",
            "user_login": "baduser",
            "user_name": "BadUser",
            "message_id": "another",
            "message": { "text": "spoilers ahead", "fragments": [] },
            "reason": "blocked_term",
            "automod": null,
            "blocked_term": { "terms_found": [{ "term_id": "123" }] },
        }))
        .expect("a held message");
        assert_eq!(term.to_held(Timestamp::now()).detail, "blocked term");
    }

    #[test]
    fn a_moderator_deciding_about_a_suspicious_user_is_read() {
        // The payload exactly as the subscription type reference prints
        // it, names and all.
        let update: SuspiciousEvent = serde_json::from_value(serde_json::json!({
            "broadcaster_user_id": "1050263435",
            "broadcaster_user_name": "77f111cbb75341449f5",
            "broadcaster_user_login": "77f111cbb75341449f5",
            "moderator_user_id": "1050263436",
            "moderator_user_name": "29087e59dfc441968f6",
            "moderator_user_login": "29087e59dfc441968f6",
            "user_id": "1050263437",
            "user_name": "06fbcc75952245c5a87",
            "user_login": "06fbcc75952245c5a87",
            "low_trust_status": "restricted",
        }))
        .expect("a suspicious user update");
        assert_eq!(update.channel().as_str(), "1050263435");
        assert_eq!(update.moderator_user_name, "29087e59dfc441968f6");
        assert_eq!(update.user_name, "06fbcc75952245c5a87");
        assert_eq!(
            update.suspicion().status,
            liveside_proto::model::SuspicionStatus::Restricted
        );
        // Letting somebody go is the same event with nothing to show.
        let cleared: SuspiciousEvent = serde_json::from_value(serde_json::json!({
            "broadcaster_user_id": "1050263435",
            "moderator_user_name": "a_moderator",
            "user_name": "somebody",
            "low_trust_status": "none",
        }))
        .expect("a lifted suspicion");
        assert_eq!(
            cleared.suspicion().status,
            liveside_proto::model::SuspicionStatus::None
        );
        // And the message event carries no moderator at all: Twitch
        // flagged it, nobody decided anything.
        let message: SuspiciousEvent = serde_json::from_value(serde_json::json!({
            "broadcaster_user_id": "1050263432",
            "user_id": "1050263434",
            "user_name": "4a46e2cf2e2f4d6a9e6",
            "low_trust_status": "active_monitoring",
            "types": ["ban_evader"],
            "ban_evasion_evaluation": "likely",
            "shared_ban_channel_ids": ["100", "200"],
            "message": {"message_id": "101010", "text": "bad stuff"},
        }))
        .expect("a suspicious message");
        assert!(message.moderator_user_name.is_empty());
        assert_eq!(message.suspicion().shared_bans, 2);
    }

    #[test]
    fn channel_events_are_read_whatever_shape_twitch_sends() {
        let follow: ChannelEvent = serde_json::from_value(serde_json::json!({
            "user_id": "1234",
            "user_login": "cool_user",
            "user_name": "Cool_User",
            "broadcaster_user_id": "1337",
            "followed_at": "2020-07-15T18:16:11.17106713Z",
        }))
        .expect("a follow");
        assert_eq!(follow.channel().as_str(), "1337");
        assert_eq!(follow.user_name, "Cool_User");

        // Twitch writes the length of an advertisement as a string in its
        // own examples and as a number in practice.
        let as_string: ChannelEvent = serde_json::from_value(serde_json::json!({
            "broadcaster_user_id": "1337",
            "duration_seconds": "60",
        }))
        .expect("an ad break");
        let as_number: ChannelEvent = serde_json::from_value(serde_json::json!({
            "broadcaster_user_id": "1337",
            "duration_seconds": 60,
        }))
        .expect("an ad break");
        assert_eq!(as_string.seconds(), 60);
        assert_eq!(as_number.seconds(), 60);

        let shoutout: ChannelEvent = serde_json::from_value(serde_json::json!({
            "broadcaster_user_id": "1337",
            "from_broadcaster_user_name": "SandySanderman",
            "viewer_count": 860,
        }))
        .expect("a shoutout");
        assert_eq!(shoutout.viewer_count, Some(860));
        // A field another event carries and this one does not is simply
        // empty, rather than a parse failure that loses the whole line.
        assert!(shoutout.title.is_empty());
    }

    #[test]
    fn notification_becomes_a_user_notice() {
        let json = r#"{
            "broadcaster_user_id": "1",
            "chatter_user_id": "2",
            "notice_type": "resub",
            "system_message": "Viewer32 subscribed for 16 months!",
            "message": { "text": "still here", "fragments": [
                {"type": "text", "text": "still here"}
            ]}
        }"#;
        let event: NotificationEvent = serde_json::from_str(json).unwrap();
        let line = event.to_line(Timestamp(0));
        let LineContent::UserNotice {
            system_text,
            message,
        } = line.content
        else {
            panic!("expected a user notice");
        };
        assert!(system_text.contains("subscribed"));
        assert_eq!(message.unwrap().plain_text(), "still here");
    }
}
