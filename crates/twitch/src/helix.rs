//! The Helix REST API: sending chat, moderation, and looking users up.

use liveside_proto::model::{RoomModes, User};
use serde::Deserialize;
use serde_json::json;

use crate::{Error, Result};

const BASE: &str = "https://api.twitch.tv/helix";

/// Authenticated Helix client. Cheap to clone.
#[derive(Debug, Clone)]
pub struct Helix {
    http: reqwest::Client,
    client_id: String,
    access_token: String,
}

#[derive(Deserialize)]
struct Page<T> {
    data: Vec<T>,
}

/// What Twitch answers when `data` is one object rather than a list, as
/// the schedule is.
#[derive(Deserialize)]
struct One<T> {
    data: T,
}

#[derive(Deserialize)]
struct HelixUser {
    id: String,
    login: String,
    display_name: String,
    /// When the account was made. A fresh one in a wave of abuse is the
    /// thing a moderator looks at first.
    #[serde(default)]
    created_at: String,
}

#[derive(Deserialize)]
struct SendResult {
    is_sent: bool,
    drop_reason: Option<DropReason>,
}

#[derive(Deserialize)]
struct DropReason {
    #[allow(dead_code)]
    code: String,
    message: String,
}

/// What one person's card says. The last two are answered only where
/// this account may ask — a moderator of that channel.
#[derive(Debug, Clone)]
pub struct Card {
    pub user: User,
    /// Whether Twitch answered the question at all. It refuses both of
    /// these in a channel this account does not own or moderate.
    pub follows_known: bool,
    pub sub_known: bool,
    /// `1000`, `2000`, `3000`, or nothing: not subscribed, or we may not
    /// ask, which only the channel's own login may.
    pub sub_tier: Option<String>,
    pub sub_gift: bool,
    /// When the account was made.
    pub created_at: String,
    pub follows_since: Option<String>,
    /// Banned here at all, with the end of it when it is a timeout.
    pub banned: bool,
    pub banned_until: Option<String>,
}

/// A poll as Helix answers it. The same shape comes back from creating,
/// ending and asking for one.
#[derive(Debug, Clone, Deserialize)]
pub struct HelixPoll {
    pub id: String,
    pub title: String,
    pub choices: Vec<HelixPollChoice>,
    /// `ACTIVE`, `COMPLETED`, `TERMINATED`, `ARCHIVED`, ...
    pub status: String,
    #[serde(default)]
    pub ends_at: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct HelixPollChoice {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub votes: u32,
}

/// What a channel says about itself.
#[derive(Debug, Clone, Deserialize)]
pub struct ChannelInformation {
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub game_id: String,
    #[serde(default)]
    pub game_name: String,
    #[serde(default)]
    pub broadcaster_language: String,
    #[serde(default)]
    pub tags: Vec<String>,
    /// Seconds the broadcast is buffered. Partners only; zero otherwise.
    #[serde(default)]
    pub delay: u32,
    #[serde(default)]
    pub content_classification_labels: Vec<String>,
}

/// A channel's schedule as Helix answers it.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct HelixSchedule {
    #[serde(default)]
    pub segments: Vec<HelixSegment>,
    #[serde(default)]
    pub vacation: Option<HelixVacation>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct HelixSegment {
    pub id: String,
    pub start_time: String,
    #[serde(default)]
    pub end_time: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub canceled_until: Option<String>,
    #[serde(default)]
    pub is_recurring: bool,
    #[serde(default)]
    pub category: Option<SegmentCategory>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SegmentCategory {
    #[serde(default)]
    pub name: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct HelixVacation {
    pub start_time: String,
    pub end_time: String,
}

/// Who is in a Shared Chat session.
#[derive(Debug, Clone, Deserialize)]
pub struct SharedSession {
    #[serde(default)]
    pub participants: Vec<SharedSessionParticipant>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SharedSessionParticipant {
    #[serde(default)]
    pub broadcaster_id: String,
}

/// Whether a Hype Train is running, and how far it has got.
#[derive(Debug, Clone, Deserialize)]
pub struct HypeTrainStatus {
    #[serde(default)]
    pub current: Option<HypeTrainCurrent>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct HypeTrainCurrent {
    #[serde(default)]
    pub level: u32,
}

/// A channel that is streaming, as Get Streams lists it.
#[derive(Debug, Clone, Deserialize)]
pub struct LiveStream {
    pub user_id: String,
    #[serde(default)]
    pub viewer_count: u32,
    /// When it started, as Twitch writes it (RFC 3339, UTC).
    #[serde(default)]
    pub started_at: String,
}

/// What the advertisement schedule says.
///
/// The times are read as whatever arrives: the documentation says
/// RFC3339 strings, and Twitch sends the number `0` when nothing is
/// scheduled — and a unix timestamp when something is. A struct that
/// believed the documentation failed to decode the answer at all.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct AdSchedule {
    #[serde(default)]
    pub next_ad_at: Option<serde_json::Value>,
    #[serde(default)]
    pub last_ad_at: Option<serde_json::Value>,
    /// How long the next break runs, in seconds.
    #[serde(default)]
    pub duration: u32,
    /// How many times it can still be pushed back, and when that count
    /// goes up again.
    #[serde(default)]
    pub snooze_count: u32,
    #[serde(default)]
    pub snooze_refresh_at: Option<serde_json::Value>,
}

impl AdSchedule {
    /// When the next break is, as a sentence can use it: empty when
    /// there is none, which Twitch writes as a zero rather than as
    /// nothing at all.
    pub fn next_at(&self) -> String {
        match &self.next_ad_at {
            Some(serde_json::Value::String(at)) if !at.is_empty() => at.clone(),
            Some(serde_json::Value::Number(n)) => match n.as_i64().unwrap_or(0) {
                0 => String::new(),
                // Unix seconds, which is what arrives in practice. The
                // number is passed on as it is; putting a date together
                // is the frontend's job, and it has the clock for it.
                seconds => seconds.to_string(),
            },
            _ => String::new(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct ShieldStatus {
    #[serde(default)]
    pub is_active: bool,
}

/// What creating a clip answers with.
#[derive(Debug, Clone, Deserialize)]
pub struct HelixClip {
    pub id: String,
    #[serde(default)]
    pub edit_url: String,
}

/// A channel point reward as Helix answers it.
#[derive(Debug, Clone, Deserialize)]
pub struct HelixReward {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub cost: u64,
    #[serde(default)]
    pub prompt: String,
    #[serde(default)]
    pub is_enabled: bool,
    /// A paused reward stays on the list and cannot be redeemed, which
    /// is what somebody stepping away from the stream wants.
    #[serde(default)]
    pub is_paused: bool,
}

/// One redemption of one.
#[derive(Debug, Clone, Deserialize)]
pub struct HelixRedemption {
    pub id: String,
    #[serde(default)]
    pub user_id: String,
    #[serde(default)]
    pub user_login: String,
    #[serde(default)]
    pub user_name: String,
    #[serde(default)]
    pub user_input: String,
    #[serde(default)]
    pub status: String,
    pub reward: RedemptionReward,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RedemptionReward {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub cost: u64,
}

/// A prediction as Helix answers it.
#[derive(Debug, Clone, Deserialize)]
pub struct HelixPrediction {
    pub id: String,
    pub title: String,
    pub outcomes: Vec<HelixPredictionOutcome>,
    /// `ACTIVE`, `LOCKED`, `RESOLVED`, `CANCELED`.
    pub status: String,
    #[serde(default)]
    pub locked_at: String,
    #[serde(default)]
    pub winning_outcome_id: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct HelixPredictionOutcome {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub users: u32,
    #[serde(default)]
    pub channel_points: u64,
}

/// One subscriber, as the channel's own list has them.
#[derive(Debug, Clone, Deserialize)]
pub struct SubscriptionEntry {
    #[serde(default)]
    pub tier: String,
    #[serde(default)]
    pub is_gift: bool,
    #[serde(default)]
    pub user_name: String,
    /// Who gave it, when it was a gift.
    #[serde(default)]
    pub gifter_name: String,
}

/// Somebody who follows a channel, and since when.
#[derive(Debug, Clone, Deserialize)]
pub struct Follower {
    pub followed_at: String,
    #[serde(default)]
    pub user_name: String,
}

/// A page of a list that also says how long the whole list is.
#[derive(Deserialize)]
struct Counted<T> {
    data: Vec<T>,
    #[serde(default)]
    pagination: Option<Cursor>,
    #[serde(default)]
    total: u64,
    /// Subscriptions only: what they are worth towards the channel's
    /// standing with Twitch.
    #[serde(default)]
    points: Option<u64>,
}

/// A long list read up to a limit, with how long it is in all.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Partial<T> {
    pub items: Vec<T>,
    pub total: u64,
    pub points: Option<u64>,
}

/// Somebody a list names.
#[derive(Debug, Clone, Deserialize)]
pub struct NamedUser {
    pub user_name: String,
}

/// Somebody banned, and when their timeout ends if it is one.
#[derive(Debug, Clone, Deserialize)]
pub struct BannedUser {
    pub user_name: String,
    #[serde(default)]
    pub expires_at: Option<String>,
}

/// A request to be let back into a channel, as Helix reports it.
#[derive(Debug, Clone, Deserialize)]
pub struct UnbanRequestData {
    pub id: String,
    pub user_id: String,
    pub user_login: String,
    pub user_name: String,
    #[serde(default)]
    pub text: String,
}

/// One set of chat badges (`subscriber`, `moderator`, ...) and the
/// versions in it.
#[derive(Debug, Clone, Deserialize)]
pub struct BadgeSet {
    pub set_id: String,
    pub versions: Vec<BadgeVersion>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct BadgeVersion {
    pub id: String,
    /// 36×36, which is a good size for a line of chat on any screen.
    pub image_url_2x: String,
    #[serde(default)]
    pub title: String,
}

/// A game or category, as Twitch knows it.
#[derive(Debug, Clone, Deserialize)]
pub struct Game {
    pub id: String,
    pub name: String,
}

/// A channel the account moderates.
#[derive(Debug, Clone, Deserialize)]
pub struct ModeratedChannel {
    pub broadcaster_id: String,
    pub broadcaster_login: String,
    pub broadcaster_name: String,
}

/// A channel the account follows: the same three names, from another
/// list.
pub type FollowedChannel = ModeratedChannel;

/// A followed channel that streams now, with what it streams.
#[derive(Debug, Clone, Deserialize)]
pub struct FollowedStream {
    pub user_id: String,
    pub user_login: String,
    pub user_name: String,
    /// The category; empty when none is set.
    #[serde(default)]
    pub game_name: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub viewer_count: u32,
    /// When it started, as Twitch writes it (RFC 3339, UTC).
    #[serde(default)]
    pub started_at: String,
}

/// A blocked word or phrase, with the id Twitch removes it by.
#[derive(Debug, Clone, Deserialize)]
pub struct BlockedTerm {
    pub id: String,
    pub text: String,
    /// When it was added, as Twitch writes it (RFC 3339).
    #[serde(default)]
    pub created_at: String,
    /// When it stops being blocked, for a term added for a while.
    #[serde(default)]
    pub expires_at: Option<String>,
}

#[derive(Deserialize)]
struct Paged<T> {
    data: Vec<T>,
    #[serde(default)]
    pagination: Option<Cursor>,
}

#[derive(Deserialize)]
struct Cursor {
    #[serde(default)]
    cursor: Option<String>,
}

#[derive(Deserialize)]
struct Marker {
    position_seconds: u32,
}

#[derive(Deserialize)]
struct Commercial {
    #[serde(default)]
    message: String,
}

#[derive(Deserialize)]
struct HelixError {
    message: Option<String>,
    error: Option<String>,
}

/// Chat settings as Helix names them. EventSub uses different names for
/// the same thing, which is why this is not shared with `mapping`.
#[derive(Deserialize)]
struct ChatSettings {
    #[serde(default)]
    emote_mode: bool,
    #[serde(default)]
    follower_mode: bool,
    #[serde(default)]
    follower_mode_duration: Option<u32>,
    #[serde(default)]
    slow_mode: bool,
    #[serde(default)]
    slow_mode_wait_time: Option<u32>,
    #[serde(default)]
    subscriber_mode: bool,
    #[serde(default)]
    unique_chat_mode: bool,
}

/// An EventSub subscription as Helix reports it.
#[derive(Debug, Clone, Deserialize)]
pub struct Subscription {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: String,
}

impl Helix {
    pub fn new(
        http: reqwest::Client,
        client_id: impl Into<String>,
        access_token: impl Into<String>,
    ) -> Self {
        Self {
            http,
            client_id: client_id.into(),
            access_token: access_token.into(),
        }
    }

    fn request(&self, method: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
        self.http
            .request(method, format!("{BASE}{path}"))
            .header("Client-Id", &self.client_id)
            .bearer_auth(&self.access_token)
    }

    /// Looks up users by login. Unknown logins are simply missing from the
    /// result, which is how a "no such channel" is detected.
    pub async fn users_by_login(&self, logins: &[&str]) -> Result<Vec<User>> {
        let query: Vec<(&str, &str)> = logins.iter().map(|login| ("login", *login)).collect();
        let response = self
            .request(reqwest::Method::GET, "/users")
            .query(&query)
            .send()
            .await?;
        let page: Page<HelixUser> = parse(response).await?;
        Ok(page
            .data
            .into_iter()
            .map(|user| User {
                id: user.id.into(),
                login: user.login.into(),
                display_name: user.display_name,
            })
            .collect())
    }

    /// The channel's current chat restrictions. Needs no special scope,
    /// which is why the moderator-only delay settings are not requested.
    pub async fn chat_settings(&self, broadcaster_id: &str) -> Result<RoomModes> {
        let response = self
            .request(reqwest::Method::GET, "/chat/settings")
            .query(&[("broadcaster_id", broadcaster_id)])
            .send()
            .await?;
        let page: Page<ChatSettings> = parse(response).await?;
        let settings = page
            .data
            .into_iter()
            .next()
            .ok_or_else(|| Error::Api("no chat settings returned".into()))?;
        Ok(RoomModes {
            emote_only: settings.emote_mode,
            followers_only: settings
                .follower_mode
                .then(|| settings.follower_mode_duration.unwrap_or(0)),
            subscribers_only: settings.subscriber_mode,
            slow: settings
                .slow_mode
                .then(|| settings.slow_mode_wait_time.unwrap_or(0)),
            unique_chat: settings.unique_chat_mode,
        })
    }

    /// Changes one or more chat restrictions. Only the fields given are
    /// touched, so two moderators changing different settings do not undo
    /// each other.
    pub async fn update_chat_settings(
        &self,
        broadcaster_id: &str,
        moderator_id: &str,
        change: &serde_json::Value,
    ) -> Result<()> {
        let response = self
            .request(reqwest::Method::PATCH, "/chat/settings")
            .query(&[
                ("broadcaster_id", broadcaster_id),
                ("moderator_id", moderator_id),
            ])
            .json(change)
            .send()
            .await?;
        // The answer repeats the settings; the caller learns the truth from
        // the EventSub notification that follows.
        parse::<Page<serde_json::Value>>(response).await.map(|_| ())
    }

    /// Removes every message in the channel.
    pub async fn clear_chat(&self, broadcaster_id: &str, moderator_id: &str) -> Result<()> {
        let response = self
            .request(reqwest::Method::DELETE, "/moderation/chat")
            .query(&[
                ("broadcaster_id", broadcaster_id),
                ("moderator_id", moderator_id),
            ])
            .send()
            .await?;
        empty(response).await
    }

    /// Sends an announcement: the highlighted message a moderator can post.
    /// `color` is `blue`, `green`, `orange`, `purple` or `primary`.
    pub async fn announce(
        &self,
        broadcaster_id: &str,
        moderator_id: &str,
        message: &str,
        color: &str,
    ) -> Result<()> {
        let response = self
            .request(reqwest::Method::POST, "/chat/announcements")
            .query(&[
                ("broadcaster_id", broadcaster_id),
                ("moderator_id", moderator_id),
            ])
            .json(&json!({ "message": message, "color": color }))
            .send()
            .await?;
        empty(response).await
    }

    /// Warns a user: they must acknowledge it before they can chat again.
    pub async fn warn(
        &self,
        broadcaster_id: &str,
        moderator_id: &str,
        user_id: &str,
        reason: &str,
    ) -> Result<()> {
        let response = self
            .request(reqwest::Method::POST, "/moderation/warnings")
            .query(&[
                ("broadcaster_id", broadcaster_id),
                ("moderator_id", moderator_id),
            ])
            .json(&json!({ "data": { "user_id": user_id, "reason": reason } }))
            .send()
            .await?;
        // The answer describes the warning; nothing here needs it.
        parse::<Page<serde_json::Value>>(response).await.map(|_| ())
    }

    /// Gives or takes moderator status. Only the broadcaster may do this.
    pub async fn set_moderator(&self, broadcaster_id: &str, user_id: &str, on: bool) -> Result<()> {
        let method = if on {
            reqwest::Method::POST
        } else {
            reqwest::Method::DELETE
        };
        let response = self
            .request(method, "/moderation/moderators")
            .query(&[("broadcaster_id", broadcaster_id), ("user_id", user_id)])
            .send()
            .await?;
        empty(response).await
    }

    /// Gives or takes VIP status. Only the broadcaster may do this.
    pub async fn set_vip(&self, broadcaster_id: &str, user_id: &str, on: bool) -> Result<()> {
        let method = if on {
            reqwest::Method::POST
        } else {
            reqwest::Method::DELETE
        };
        let response = self
            .request(method, "/channels/vips")
            .query(&[("broadcaster_id", broadcaster_id), ("user_id", user_id)])
            .send()
            .await?;
        empty(response).await
    }

    /// Sends a chat message. Twitch can accept the request but still drop the
    /// message (AutoMod, slow mode, ...), which is reported as an error here.
    pub async fn send_message(
        &self,
        broadcaster_id: &str,
        sender_id: &str,
        message: &str,
        reply_to: Option<&str>,
    ) -> Result<()> {
        let mut body = json!({
            "broadcaster_id": broadcaster_id,
            "sender_id": sender_id,
            "message": message,
        });
        if let Some(reply_to) = reply_to {
            body["reply_parent_message_id"] = json!(reply_to);
        }
        let response = self
            .request(reqwest::Method::POST, "/chat/messages")
            .json(&body)
            .send()
            .await?;
        let page: Page<SendResult> = parse(response).await?;
        match page.data.into_iter().next() {
            Some(result) if result.is_sent => Ok(()),
            Some(result) => Err(Error::Api(
                result
                    .drop_reason
                    .map(|reason| reason.message)
                    .unwrap_or_else(|| "Twitch dropped the message".into()),
            )),
            None => Err(Error::Api("Twitch returned no result".into())),
        }
    }

    /// Bans a user, or times them out when `seconds` is given.
    pub async fn ban(
        &self,
        broadcaster_id: &str,
        moderator_id: &str,
        user_id: &str,
        seconds: Option<u32>,
        reason: Option<&str>,
    ) -> Result<()> {
        let mut data = json!({ "user_id": user_id });
        if let Some(seconds) = seconds {
            data["duration"] = json!(seconds);
        }
        if let Some(reason) = reason {
            data["reason"] = json!(reason);
        }
        let response = self
            .request(reqwest::Method::POST, "/moderation/bans")
            .query(&[
                ("broadcaster_id", broadcaster_id),
                ("moderator_id", moderator_id),
            ])
            .json(&json!({ "data": data }))
            .send()
            .await?;
        empty(response).await
    }

    pub async fn unban(
        &self,
        broadcaster_id: &str,
        moderator_id: &str,
        user_id: &str,
    ) -> Result<()> {
        let response = self
            .request(reqwest::Method::DELETE, "/moderation/bans")
            .query(&[
                ("broadcaster_id", broadcaster_id),
                ("moderator_id", moderator_id),
                ("user_id", user_id),
            ])
            .send()
            .await?;
        empty(response).await
    }

    pub async fn delete_message(
        &self,
        broadcaster_id: &str,
        moderator_id: &str,
        message_id: &str,
    ) -> Result<()> {
        let response = self
            .request(reqwest::Method::DELETE, "/moderation/chat")
            .query(&[
                ("broadcaster_id", broadcaster_id),
                ("moderator_id", moderator_id),
                ("message_id", message_id),
            ])
            .send()
            .await?;
        empty(response).await
    }

    /// The id of a game or category by its exact name, for setting the
    /// category a channel is in.
    pub async fn game_by_name(&self, name: &str) -> Result<Option<Game>> {
        let response = self
            .request(reqwest::Method::GET, "/games")
            .query(&[("name", name)])
            .send()
            .await?;
        let page: Page<Game> = parse(response).await?;
        Ok(page.data.into_iter().next())
    }

    /// What the channel says about itself right now: the title, what is
    /// being played, the language, the tags, how long the broadcast is
    /// buffered, and which content labels are set.
    pub async fn channel_information(&self, broadcaster_id: &str) -> Result<ChannelInformation> {
        let response = self
            .request(reqwest::Method::GET, "/channels")
            .query(&[("broadcaster_id", broadcaster_id)])
            .send()
            .await?;
        let page: Page<ChannelInformation> = parse(response).await?;
        page.data
            .into_iter()
            .next()
            .ok_or_else(|| Error::Api("Twitch knows no such channel".to_owned()))
    }

    /// Changes what the channel says it is streaming. Twitch only lets the
    /// broadcaster do this, whatever moderator status the account has.
    pub async fn update_channel(
        &self,
        broadcaster_id: &str,
        change: &serde_json::Value,
    ) -> Result<()> {
        let response = self
            .request(reqwest::Method::PATCH, "/channels")
            .query(&[("broadcaster_id", broadcaster_id)])
            .json(change)
            .send()
            .await?;
        empty(response).await
    }

    /// What can be learned about one person: when the account was made,
    /// since when they follow this channel, and whether they are banned
    /// from it. The last two are answered only where the account may ask
    /// — a moderator of that channel — and are `None` otherwise.
    pub async fn user_card(&self, login: &str, broadcaster_id: &str) -> Result<Card> {
        let response = self
            .request(reqwest::Method::GET, "/users")
            .query(&[("login", login)])
            .send()
            .await?;
        let page: Page<HelixUser> = parse(response).await?;
        let found = page
            .data
            .into_iter()
            .next()
            .ok_or_else(|| Error::Api(format!("no user {login}")))?;
        let banned = self.banned_until(broadcaster_id, &found.id).await;
        // A refusal is not an answer: Twitch says "no permission for the
        // broadcaster" in a channel this account neither owns nor
        // moderates, and reporting that as "not following" would be a
        // statement about somebody that nobody made.
        let subscription = self.subscription(broadcaster_id, &found.id).await;
        let follows = self.follows_since(broadcaster_id, &found.id).await;
        Ok(Card {
            sub_known: subscription.is_ok(),
            sub_tier: subscription
                .as_ref()
                .ok()
                .and_then(|s| s.as_ref().map(|(tier, _)| tier.clone())),
            sub_gift: subscription
                .as_ref()
                .ok()
                .and_then(|s| s.as_ref().map(|(_, gift)| *gift))
                .unwrap_or(false),
            created_at: found.created_at.clone(),
            follows_known: follows.is_ok(),
            follows_since: follows.ok().flatten(),
            banned: matches!(banned, Ok(Some(_))),
            banned_until: banned.ok().flatten().flatten(),
            user: User {
                id: found.id.into(),
                login: found.login.into(),
                display_name: found.display_name,
            },
        })
    }

    /// Whether somebody subscribes to this channel, and at what tier.
    /// Twitch takes this only from the channel's own login, so it is
    /// asked for a card and passed over where it refuses.
    async fn subscription(
        &self,
        broadcaster_id: &str,
        user_id: &str,
    ) -> Result<Option<(String, bool)>> {
        let response = self
            .request(reqwest::Method::GET, "/subscriptions")
            .query(&[("broadcaster_id", broadcaster_id), ("user_id", user_id)])
            .send()
            .await?;
        let page: Page<SubscriptionEntry> = parse(response).await?;
        Ok(page
            .data
            .into_iter()
            .next()
            .map(|entry| (entry.tier, entry.is_gift)))
    }

    /// Since when somebody follows a channel, if they do.
    async fn follows_since(&self, broadcaster_id: &str, user_id: &str) -> Result<Option<String>> {
        let response = self
            .request(reqwest::Method::GET, "/channels/followers")
            .query(&[("broadcaster_id", broadcaster_id), ("user_id", user_id)])
            .send()
            .await?;
        let page: Page<Follower> = parse(response).await?;
        Ok(page.data.into_iter().next().map(|f| f.followed_at))
    }

    /// Whether somebody is banned here, and until when if it is a
    /// timeout. The outer `None` means we could not ask.
    async fn banned_until(
        &self,
        broadcaster_id: &str,
        user_id: &str,
    ) -> Result<Option<Option<String>>> {
        let response = self
            .request(reqwest::Method::GET, "/moderation/banned")
            .query(&[("broadcaster_id", broadcaster_id), ("user_id", user_id)])
            .send()
            .await?;
        let page: Page<BannedUser> = parse(response).await?;
        Ok(page
            .data
            .into_iter()
            .next()
            .map(|user| user.expires_at.filter(|at| !at.is_empty())))
    }

    /// The people allowed to moderate the channel.
    pub async fn moderators(&self, broadcaster_id: &str) -> Result<Vec<String>> {
        self.names("/moderation/moderators", broadcaster_id).await
    }

    /// Its VIPs.
    pub async fn vips(&self, broadcaster_id: &str) -> Result<Vec<String>> {
        self.names("/channels/vips", broadcaster_id).await
    }

    /// Everyone banned or in a timeout, newest first as Twitch sends them.
    /// A timeout says when it runs out; a ban says nothing.
    pub async fn banned(&self, broadcaster_id: &str) -> Result<Vec<(String, Option<String>)>> {
        let mut out = Vec::new();
        let mut cursor: Option<String> = None;
        loop {
            let mut query = vec![("broadcaster_id", broadcaster_id), ("first", "100")];
            if let Some(cursor) = &cursor {
                query.push(("after", cursor));
            }
            let response = self
                .request(reqwest::Method::GET, "/moderation/banned")
                .query(&query)
                .send()
                .await?;
            let page: Paged<BannedUser> = parse(response).await?;
            let empty = page.data.is_empty();
            out.extend(
                page.data
                    .into_iter()
                    .map(|user| (user.user_name, user.expires_at.filter(|at| !at.is_empty()))),
            );
            cursor = page.pagination.and_then(|p| p.cursor);
            if cursor.is_none() || empty || out.len() >= 400 {
                return Ok(out);
            }
        }
    }

    /// The channel's followers, the newest first, up to `limit`, and how
    /// many there are. Twitch names them only to the channel and its
    /// moderators; anyone else is told the number and an empty list.
    pub async fn followers(&self, broadcaster_id: &str, limit: usize) -> Result<Partial<Follower>> {
        self.counted("/channels/followers", broadcaster_id, limit)
            .await
    }

    /// Who subscribes, up to `limit`, with how many there are and the
    /// points they make. Only the channel's own login may ask.
    pub async fn subscribers(
        &self,
        broadcaster_id: &str,
        limit: usize,
    ) -> Result<Partial<SubscriptionEntry>> {
        self.counted("/subscriptions", broadcaster_id, limit).await
    }

    /// A list that says how long it is, read page by page up to `limit`.
    async fn counted<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        broadcaster_id: &str,
        limit: usize,
    ) -> Result<Partial<T>> {
        let mut out = Partial {
            items: Vec::new(),
            total: 0,
            points: None,
        };
        let mut cursor: Option<String> = None;
        loop {
            let mut query = vec![("broadcaster_id", broadcaster_id), ("first", "100")];
            if let Some(cursor) = &cursor {
                query.push(("after", cursor));
            }
            let response = self
                .request(reqwest::Method::GET, path)
                .query(&query)
                .send()
                .await?;
            let page: Counted<T> = parse(response).await?;
            let empty = page.data.is_empty();
            out.total = page.total;
            out.points = page.points.or(out.points);
            out.items.extend(page.data);
            cursor = page.pagination.and_then(|p| p.cursor);
            if cursor.is_none() || empty || out.items.len() >= limit {
                out.items.truncate(limit);
                return Ok(out);
            }
        }
    }

    /// The shape both name lists share.
    async fn names(&self, path: &str, broadcaster_id: &str) -> Result<Vec<String>> {
        let mut out = Vec::new();
        let mut cursor: Option<String> = None;
        loop {
            let mut query = vec![("broadcaster_id", broadcaster_id), ("first", "100")];
            if let Some(cursor) = &cursor {
                query.push(("after", cursor));
            }
            let response = self
                .request(reqwest::Method::GET, path)
                .query(&query)
                .send()
                .await?;
            let page: Paged<NamedUser> = parse(response).await?;
            let empty = page.data.is_empty();
            out.extend(page.data.into_iter().map(|user| user.user_name));
            cursor = page.pagination.and_then(|p| p.cursor);
            if cursor.is_none() || empty || out.len() >= 400 {
                return Ok(out);
            }
        }
    }

    /// The requests to be unbanned that are still waiting for an answer.
    pub async fn unban_requests(
        &self,
        broadcaster_id: &str,
        moderator_id: &str,
    ) -> Result<Vec<UnbanRequestData>> {
        let response = self
            .request(reqwest::Method::GET, "/moderation/unban_requests")
            .query(&[
                ("broadcaster_id", broadcaster_id),
                ("moderator_id", moderator_id),
                ("status", "pending"),
                ("first", "100"),
            ])
            .send()
            .await?;
        let page: Page<UnbanRequestData> = parse(response).await?;
        Ok(page.data)
    }

    /// Approves or denies one of them.
    pub async fn resolve_unban_request(
        &self,
        broadcaster_id: &str,
        moderator_id: &str,
        request_id: &str,
        allow: bool,
    ) -> Result<()> {
        let status = if allow { "approved" } else { "denied" };
        let response = self
            .request(reqwest::Method::PATCH, "/moderation/unban_requests")
            .query(&[
                ("broadcaster_id", broadcaster_id),
                ("moderator_id", moderator_id),
                ("unban_request_id", request_id),
                ("status", status),
            ])
            .send()
            .await?;
        parse::<Page<serde_json::Value>>(response).await.map(|_| ())
    }

    /// Lets a message AutoMod is holding through, or throws it away.
    pub async fn resolve_held(
        &self,
        moderator_id: &str,
        message_id: &str,
        allow: bool,
    ) -> Result<()> {
        let action = if allow { "ALLOW" } else { "DENY" };
        let response = self
            .request(reqwest::Method::POST, "/moderation/automod/message")
            .json(&json!({
                "user_id": moderator_id,
                "msg_id": message_id,
                "action": action,
            }))
            .send()
            .await?;
        empty(response).await
    }

    /// The chat badges of a channel, or Twitch's global ones when no
    /// channel is given. Keyed by set and version, as badges arrive on a
    /// message.
    pub async fn badges(&self, broadcaster_id: Option<&str>) -> Result<Vec<BadgeSet>> {
        let path = match broadcaster_id {
            Some(_) => "/chat/badges",
            None => "/chat/badges/global",
        };
        let mut request = self.request(reqwest::Method::GET, path);
        if let Some(id) = broadcaster_id {
            request = request.query(&[("broadcaster_id", id)]);
        }
        let page: Page<BadgeSet> = parse(request.send().await?).await?;
        Ok(page.data)
    }

    /// Which of these channels are streaming right now, in one call.
    /// Twitch answers with a stream only while one is running, so a
    /// channel missing from the answer is offline.
    /// Asked again after every reconnection: `stream.offline` says a
    /// stream ended, and a client that was not listening at the time
    /// would otherwise show a green dot for days.
    pub async fn live_among(&self, broadcaster_ids: &[String]) -> Result<Vec<LiveStream>> {
        if broadcaster_ids.is_empty() {
            return Ok(Vec::new());
        }
        let query: Vec<(&str, &str)> = broadcaster_ids
            .iter()
            .map(|id| ("user_id", id.as_str()))
            .collect();
        let response = self
            .request(reqwest::Method::GET, "/streams")
            .query(&query)
            .send()
            .await?;
        let page: Page<LiveStream> = parse(response).await?;
        Ok(page.data)
    }

    /// Turns Shield Mode on or off: Twitch's panic button, which applies
    /// the overrides the broadcaster set up for a raid of abuse.
    pub async fn set_shield_mode(
        &self,
        broadcaster_id: &str,
        moderator_id: &str,
        on: bool,
    ) -> Result<()> {
        let response = self
            .request(reqwest::Method::PUT, "/moderation/shield_mode")
            .query(&[
                ("broadcaster_id", broadcaster_id),
                ("moderator_id", moderator_id),
            ])
            .json(&json!({ "is_active": on }))
            .send()
            .await?;
        parse::<Page<serde_json::Value>>(response).await.map(|_| ())
    }

    /// The channel's blocked words and phrases, every page of them.
    pub async fn blocked_terms(
        &self,
        broadcaster_id: &str,
        moderator_id: &str,
    ) -> Result<Vec<BlockedTerm>> {
        let mut all = Vec::new();
        let mut cursor: Option<String> = None;
        loop {
            let mut query = vec![
                ("broadcaster_id", broadcaster_id),
                ("moderator_id", moderator_id),
                ("first", "100"),
            ];
            if let Some(cursor) = &cursor {
                query.push(("after", cursor));
            }
            let response = self
                .request(reqwest::Method::GET, "/moderation/blocked_terms")
                .query(&query)
                .send()
                .await?;
            let page: Paged<BlockedTerm> = parse(response).await?;
            let empty = page.data.is_empty();
            all.extend(page.data);
            cursor = page.pagination.and_then(|p| p.cursor);
            // A cursor that comes back on an empty page would loop forever.
            if cursor.is_none() || empty {
                return Ok(all);
            }
        }
    }

    /// The channels a user moderates, every page of them. `user_id` must be
    /// the account the token belongs to.
    pub async fn moderated_channels(&self, user_id: &str) -> Result<Vec<ModeratedChannel>> {
        let mut all = Vec::new();
        let mut cursor: Option<String> = None;
        loop {
            let mut query = vec![("user_id", user_id), ("first", "100")];
            if let Some(cursor) = &cursor {
                query.push(("after", cursor));
            }
            let response = self
                .request(reqwest::Method::GET, "/moderation/channels")
                .query(&query)
                .send()
                .await?;
            let page: Paged<ModeratedChannel> = parse(response).await?;
            let empty = page.data.is_empty();
            all.extend(page.data);
            cursor = page.pagination.and_then(|p| p.cursor);
            // A cursor that comes back on an empty page would loop forever.
            if cursor.is_none() || empty {
                return Ok(all);
            }
        }
    }

    /// The channels a user follows, every page of them, the most recently
    /// followed first. `user_id` must be the account the token belongs to.
    pub async fn followed_channels(&self, user_id: &str) -> Result<Vec<FollowedChannel>> {
        self.every_page("/channels/followed", user_id).await
    }

    /// Which of the channels a user follows stream now, the most watched
    /// first. `user_id` must be the account the token belongs to.
    pub async fn followed_streams(&self, user_id: &str) -> Result<Vec<FollowedStream>> {
        self.every_page("/streams/followed", user_id).await
    }

    /// Every page of a list about one user, a hundred at a time.
    async fn every_page<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        user_id: &str,
    ) -> Result<Vec<T>> {
        let mut all = Vec::new();
        let mut cursor: Option<String> = None;
        loop {
            let mut query = vec![("user_id", user_id), ("first", "100")];
            if let Some(cursor) = &cursor {
                query.push(("after", cursor));
            }
            let response = self
                .request(reqwest::Method::GET, path)
                .query(&query)
                .send()
                .await?;
            let page: Paged<T> = parse(response).await?;
            let empty = page.data.is_empty();
            all.extend(page.data);
            cursor = page.pagination.and_then(|p| p.cursor);
            // A cursor that comes back on an empty page would loop forever.
            if cursor.is_none() || empty {
                return Ok(all);
            }
        }
    }

    /// Blocks a word or phrase. A `*` at either end stands for anything.
    pub async fn add_blocked_term(
        &self,
        broadcaster_id: &str,
        moderator_id: &str,
        text: &str,
    ) -> Result<()> {
        let response = self
            .request(reqwest::Method::POST, "/moderation/blocked_terms")
            .query(&[
                ("broadcaster_id", broadcaster_id),
                ("moderator_id", moderator_id),
            ])
            .json(&json!({ "text": text }))
            .send()
            .await?;
        parse::<Page<serde_json::Value>>(response).await.map(|_| ())
    }

    /// Unblocks a term, which Twitch identifies by id rather than by text.
    pub async fn remove_blocked_term(
        &self,
        broadcaster_id: &str,
        moderator_id: &str,
        id: &str,
    ) -> Result<()> {
        let response = self
            .request(reqwest::Method::DELETE, "/moderation/blocked_terms")
            .query(&[
                ("broadcaster_id", broadcaster_id),
                ("moderator_id", moderator_id),
                ("id", id),
            ])
            .send()
            .await?;
        empty(response).await
    }

    /// Watches or restricts a chatter: monitored messages are flagged for
    /// moderators, restricted ones are held until a moderator lets them
    /// through. `None` lifts both.
    pub async fn set_suspicious(
        &self,
        broadcaster_id: &str,
        moderator_id: &str,
        user_id: &str,
        status: Option<&str>,
    ) -> Result<()> {
        let query = [
            ("broadcaster_id", broadcaster_id),
            ("moderator_id", moderator_id),
        ];
        let response = match status {
            Some(status) => {
                self.request(reqwest::Method::POST, "/moderation/suspicious_users")
                    .query(&query)
                    .json(&json!({ "user_id": user_id, "status": status }))
                    .send()
                    .await?
            }
            None => {
                self.request(reqwest::Method::DELETE, "/moderation/suspicious_users")
                    .query(&query)
                    .query(&[("user_id", user_id)])
                    .send()
                    .await?
            }
        };
        parse::<Page<serde_json::Value>>(response).await.map(|_| ())
    }

    /// Sends a shoutout to another broadcaster: the pop-up with a follow
    /// button that a channel shows for someone it wants to point at.
    pub async fn shoutout(
        &self,
        from_broadcaster_id: &str,
        to_broadcaster_id: &str,
        moderator_id: &str,
    ) -> Result<()> {
        let response = self
            .request(reqwest::Method::POST, "/chat/shoutouts")
            .query(&[
                ("from_broadcaster_id", from_broadcaster_id),
                ("to_broadcaster_id", to_broadcaster_id),
                ("moderator_id", moderator_id),
            ])
            .send()
            .await?;
        empty(response).await
    }

    /// Starts a raid: Twitch then counts down and moves the viewers over.
    pub async fn start_raid(
        &self,
        from_broadcaster_id: &str,
        to_broadcaster_id: &str,
    ) -> Result<()> {
        let response = self
            .request(reqwest::Method::POST, "/raids")
            .query(&[
                ("from_broadcaster_id", from_broadcaster_id),
                ("to_broadcaster_id", to_broadcaster_id),
            ])
            .send()
            .await?;
        parse::<Page<serde_json::Value>>(response).await.map(|_| ())
    }

    /// Calls off a raid that has not left yet.
    pub async fn cancel_raid(&self, broadcaster_id: &str) -> Result<()> {
        let response = self
            .request(reqwest::Method::DELETE, "/raids")
            .query(&[("broadcaster_id", broadcaster_id)])
            .send()
            .await?;
        empty(response).await
    }

    /// Marks this moment in the stream, for cutting a highlight later.
    /// Answers with how far into the stream the marker landed.
    pub async fn create_marker(&self, user_id: &str, description: Option<&str>) -> Result<u32> {
        let mut body = json!({ "user_id": user_id });
        if let Some(description) = description {
            body["description"] = serde_json::Value::from(description);
        }
        let response = self
            .request(reqwest::Method::POST, "/streams/markers")
            .json(&body)
            .send()
            .await?;
        let page: Page<Marker> = parse(response).await?;
        Ok(page.data.first().map(|m| m.position_seconds).unwrap_or(0))
    }

    /// Starts a poll. Twitch takes it only from the channel's own login,
    /// allows two to five choices, and runs it for 15 to 180 seconds.
    pub async fn create_poll(
        &self,
        broadcaster_id: &str,
        title: &str,
        choices: &[String],
        seconds: u32,
    ) -> Result<HelixPoll> {
        let choices: Vec<serde_json::Value> = choices
            .iter()
            .map(|title| json!({ "title": title }))
            .collect();
        let response = self
            .request(reqwest::Method::POST, "/polls")
            .json(&json!({
                "broadcaster_id": broadcaster_id,
                "title": title,
                "choices": choices,
                "duration": seconds,
            }))
            .send()
            .await?;
        let page: Page<HelixPoll> = parse(response).await?;
        page.data
            .into_iter()
            .next()
            .ok_or_else(|| Error::Api("Twitch started no poll".to_owned()))
    }

    /// Stops the poll that is running: `TERMINATED` shows the result,
    /// `ARCHIVED` hides it.
    pub async fn end_poll(
        &self,
        broadcaster_id: &str,
        id: &str,
        archive: bool,
    ) -> Result<HelixPoll> {
        let status = if archive { "ARCHIVED" } else { "TERMINATED" };
        let response = self
            .request(reqwest::Method::PATCH, "/polls")
            .json(&json!({
                "broadcaster_id": broadcaster_id,
                "id": id,
                "status": status,
            }))
            .send()
            .await?;
        let page: Page<HelixPoll> = parse(response).await?;
        page.data
            .into_iter()
            .next()
            .ok_or_else(|| Error::Api("Twitch ended no poll".to_owned()))
    }

    /// The polls the channel has run lately, newest first. Asked at
    /// startup so that a client joining mid-poll shows it.
    pub async fn polls(&self, broadcaster_id: &str) -> Result<Vec<HelixPoll>> {
        let response = self
            .request(reqwest::Method::GET, "/polls")
            .query(&[("broadcaster_id", broadcaster_id), ("first", "1")])
            .send()
            .await?;
        let page: Page<HelixPoll> = parse(response).await?;
        Ok(page.data)
    }

    /// Starts a prediction. The channel's own login only, two to ten
    /// outcomes, and a betting window of 30 to 1800 seconds.
    pub async fn create_prediction(
        &self,
        broadcaster_id: &str,
        title: &str,
        outcomes: &[String],
        seconds: u32,
    ) -> Result<HelixPrediction> {
        let outcomes: Vec<serde_json::Value> = outcomes
            .iter()
            .map(|title| json!({ "title": title }))
            .collect();
        let response = self
            .request(reqwest::Method::POST, "/predictions")
            .json(&json!({
                "broadcaster_id": broadcaster_id,
                "title": title,
                "outcomes": outcomes,
                "prediction_window": seconds,
            }))
            .send()
            .await?;
        let page: Page<HelixPrediction> = parse(response).await?;
        page.data
            .into_iter()
            .next()
            .ok_or_else(|| Error::Api("Twitch started no prediction".to_owned()))
    }

    /// Closes betting, pays out, or calls the whole thing off. Twitch
    /// takes `LOCKED`, `RESOLVED` (with the winning outcome) and
    /// `CANCELED`, and gives the broadcaster a day to resolve a locked
    /// one before cancelling it itself.
    pub async fn end_prediction(
        &self,
        broadcaster_id: &str,
        id: &str,
        status: &str,
        winning_outcome: Option<&str>,
    ) -> Result<HelixPrediction> {
        let mut body = json!({
            "broadcaster_id": broadcaster_id,
            "id": id,
            "status": status,
        });
        if let Some(outcome) = winning_outcome {
            body["winning_outcome_id"] = serde_json::Value::from(outcome);
        }
        let response = self
            .request(reqwest::Method::PATCH, "/predictions")
            .json(&body)
            .send()
            .await?;
        let page: Page<HelixPrediction> = parse(response).await?;
        page.data
            .into_iter()
            .next()
            .ok_or_else(|| Error::Api("Twitch ended no prediction".to_owned()))
    }

    /// The predictions the channel has run lately, newest first. Asked
    /// when joining, so that one already running shows up.
    pub async fn predictions(&self, broadcaster_id: &str) -> Result<Vec<HelixPrediction>> {
        let response = self
            .request(reqwest::Method::GET, "/predictions")
            .query(&[("broadcaster_id", broadcaster_id), ("first", "1")])
            .send()
            .await?;
        let page: Page<HelixPrediction> = parse(response).await?;
        Ok(page.data)
    }

    /// The rewards a channel offers. `only_ours` asks for the ones this
    /// application created, which are the only ones it may answer for.
    pub async fn custom_rewards(
        &self,
        broadcaster_id: &str,
        only_ours: bool,
    ) -> Result<Vec<HelixReward>> {
        let response = self
            .request(reqwest::Method::GET, "/channel_points/custom_rewards")
            .query(&[
                ("broadcaster_id", broadcaster_id),
                (
                    "only_manageable_rewards",
                    if only_ours { "true" } else { "false" },
                ),
            ])
            .send()
            .await?;
        let page: Page<HelixReward> = parse(response).await?;
        Ok(page.data)
    }

    /// Adds a reward. Created by this application, so its redemptions are
    /// answerable here — which rewards made anywhere else are not.
    pub async fn create_custom_reward(
        &self,
        broadcaster_id: &str,
        title: &str,
        cost: u64,
        prompt: &str,
    ) -> Result<HelixReward> {
        let mut body = json!({
            "broadcaster_id": broadcaster_id,
            "title": title,
            "cost": cost,
        });
        // A reward asks something only when told to, and Twitch rejects a
        // prompt without the flag.
        if !prompt.is_empty() {
            body["prompt"] = serde_json::Value::from(prompt);
            body["is_user_input_required"] = serde_json::Value::Bool(true);
        }
        let response = self
            .request(reqwest::Method::POST, "/channel_points/custom_rewards")
            .json(&body)
            .send()
            .await?;
        let page: Page<HelixReward> = parse(response).await?;
        page.data
            .into_iter()
            .next()
            .ok_or_else(|| Error::Api("Twitch created no reward".to_owned()))
    }

    /// Changes one of this application's rewards. Only the fields in
    /// `change` are touched; Twitch leaves the rest alone.
    pub async fn update_custom_reward(
        &self,
        broadcaster_id: &str,
        id: &str,
        change: &serde_json::Value,
    ) -> Result<HelixReward> {
        let response = self
            .request(reqwest::Method::PATCH, "/channel_points/custom_rewards")
            .query(&[("broadcaster_id", broadcaster_id), ("id", id)])
            .json(change)
            .send()
            .await?;
        let page: Page<HelixReward> = parse(response).await?;
        page.data
            .into_iter()
            .next()
            .ok_or_else(|| Error::Api("Twitch changed no reward".to_owned()))
    }

    /// Removes one of this application's rewards.
    pub async fn delete_custom_reward(&self, broadcaster_id: &str, id: &str) -> Result<()> {
        let response = self
            .request(reqwest::Method::DELETE, "/channel_points/custom_rewards")
            .query(&[("broadcaster_id", broadcaster_id), ("id", id)])
            .send()
            .await?;
        empty(response).await
    }

    /// What is waiting to be answered for one reward, oldest first.
    pub async fn redemptions(
        &self,
        broadcaster_id: &str,
        reward_id: &str,
    ) -> Result<Vec<HelixRedemption>> {
        let response = self
            .request(
                reqwest::Method::GET,
                "/channel_points/custom_rewards/redemptions",
            )
            .query(&[
                ("broadcaster_id", broadcaster_id),
                ("reward_id", reward_id),
                ("status", "UNFULFILLED"),
                ("sort", "OLDEST"),
                ("first", "50"),
            ])
            .send()
            .await?;
        let page: Page<HelixRedemption> = parse(response).await?;
        Ok(page.data)
    }

    /// Answers one: fulfilled, or refunded. Twitch takes this only from
    /// the application that created the reward.
    pub async fn answer_redemption(
        &self,
        broadcaster_id: &str,
        reward_id: &str,
        id: &str,
        fulfil: bool,
    ) -> Result<HelixRedemption> {
        let status = if fulfil { "FULFILLED" } else { "CANCELED" };
        let response = self
            .request(
                reqwest::Method::PATCH,
                "/channel_points/custom_rewards/redemptions",
            )
            .query(&[
                ("broadcaster_id", broadcaster_id),
                ("reward_id", reward_id),
                ("id", id),
            ])
            .json(&json!({ "status": status }))
            .send()
            .await?;
        let page: Page<HelixRedemption> = parse(response).await?;
        page.data
            .into_iter()
            .next()
            .ok_or_else(|| Error::Api("Twitch answered no redemption".to_owned()))
    }

    /// Clips the last ninety seconds or so of the stream. Twitch
    /// publishes the last thirty of them with a default title; the edit
    /// URL in the answer is where that is changed, and it is good for a
    /// day or until the clip is published.
    ///
    /// Making one is asynchronous: this answers before the clip exists.
    pub async fn create_clip(&self, broadcaster_id: &str) -> Result<(String, String)> {
        let response = self
            .request(reqwest::Method::POST, "/clips")
            .query(&[("broadcaster_id", broadcaster_id)])
            .send()
            .await?;
        let page: Page<HelixClip> = parse(response).await?;
        page.data
            .into_iter()
            .next()
            .map(|clip| (clip.id, clip.edit_url))
            .ok_or_else(|| Error::Api("Twitch made no clip".to_owned()))
    }

    /// The channel's streaming schedule: what is planned next, and
    /// whether the broadcaster is away. Twitch answers with the segments
    /// from now on, so this is "what is coming", not a calendar.
    pub async fn schedule(&self, broadcaster_id: &str) -> Result<HelixSchedule> {
        let response = self
            .request(reqwest::Method::GET, "/schedule")
            .query(&[("broadcaster_id", broadcaster_id), ("first", "10")])
            .send()
            .await?;
        // A channel with no schedule at all answers 404, which is an
        // answer rather than a failure.
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(HelixSchedule::default());
        }
        let page: One<HelixSchedule> = parse(response).await?;
        Ok(page.data)
    }

    /// Adds a broadcast to the schedule, once or every week.
    pub async fn add_segment(
        &self,
        broadcaster_id: &str,
        start: &str,
        timezone: &str,
        minutes: u32,
        title: &str,
        weekly: bool,
    ) -> Result<()> {
        let mut body = json!({
            "start_time": start,
            "timezone": timezone,
            "duration": minutes.to_string(),
            "is_recurring": weekly,
        });
        if !title.is_empty() {
            body["title"] = serde_json::Value::from(title);
        }
        let response = self
            .request(reqwest::Method::POST, "/schedule/segment")
            .query(&[("broadcaster_id", broadcaster_id)])
            .json(&body)
            .send()
            .await?;
        let _: One<HelixSchedule> = parse(response).await?;
        Ok(())
    }

    /// Calls off one broadcast, or puts it back on. A recurring one is
    /// not removed by this: that single occurrence is cancelled.
    pub async fn cancel_segment(
        &self,
        broadcaster_id: &str,
        id: &str,
        canceled: bool,
    ) -> Result<()> {
        let response = self
            .request(reqwest::Method::PATCH, "/schedule/segment")
            .query(&[("broadcaster_id", broadcaster_id), ("id", id)])
            .json(&json!({ "is_canceled": canceled }))
            .send()
            .await?;
        let _: One<HelixSchedule> = parse(response).await?;
        Ok(())
    }

    /// Removes a broadcast from the schedule for good.
    pub async fn delete_segment(&self, broadcaster_id: &str, id: &str) -> Result<()> {
        let response = self
            .request(reqwest::Method::DELETE, "/schedule/segment")
            .query(&[("broadcaster_id", broadcaster_id), ("id", id)])
            .send()
            .await?;
        empty(response).await
    }

    /// Says the broadcaster is away between two times, or that they are
    /// not away after all.
    pub async fn set_vacation(
        &self,
        broadcaster_id: &str,
        away: Option<(&str, &str)>,
        timezone: &str,
    ) -> Result<()> {
        let mut query: Vec<(&str, String)> = vec![
            ("broadcaster_id", broadcaster_id.to_owned()),
            ("is_vacation_enabled", away.is_some().to_string()),
        ];
        if let Some((start, end)) = away {
            query.push(("vacation_start_time", start.to_owned()));
            query.push(("vacation_end_time", end.to_owned()));
            query.push(("timezone", timezone.to_owned()));
        }
        let response = self
            .request(reqwest::Method::PATCH, "/schedule/settings")
            .query(&query)
            .send()
            .await?;
        empty(response).await
    }

    /// When the next advertisement break is due, how long it runs, and
    /// how many times it can still be pushed back.
    pub async fn ad_schedule(&self, broadcaster_id: &str) -> Result<AdSchedule> {
        let response = self
            .request(reqwest::Method::GET, "/channels/ads")
            .query(&[("broadcaster_id", broadcaster_id)])
            .send()
            .await?;
        let page: Page<AdSchedule> = parse(response).await?;
        Ok(page.data.into_iter().next().unwrap_or_default())
    }

    /// Pushes the next automatic break back by five minutes, if there is
    /// a snooze left.
    pub async fn snooze_ad(&self, broadcaster_id: &str) -> Result<AdSchedule> {
        let response = self
            .request(reqwest::Method::POST, "/channels/ads/schedule/snooze")
            .query(&[("broadcaster_id", broadcaster_id)])
            .send()
            .await?;
        let page: Page<AdSchedule> = parse(response).await?;
        Ok(page.data.into_iter().next().unwrap_or_default())
    }

    /// Whether Shield Mode is on right now. The two EventSub events say
    /// when it changes; this is what it was before we were listening.
    pub async fn shield_mode(&self, broadcaster_id: &str, moderator_id: &str) -> Result<bool> {
        let response = self
            .request(reqwest::Method::GET, "/moderation/shield_mode")
            .query(&[
                ("broadcaster_id", broadcaster_id),
                ("moderator_id", moderator_id),
            ])
            .send()
            .await?;
        let page: Page<ShieldStatus> = parse(response).await?;
        Ok(page.data.into_iter().next().is_some_and(|s| s.is_active))
    }

    /// Sends a whisper. Twitch refuses this unless the sending account
    /// has a verified phone number, and refuses it again if the other
    /// person has never whispered us and has whispers turned off.
    pub async fn send_whisper(&self, from: &str, to: &str, text: &str) -> Result<()> {
        let response = self
            .request(reqwest::Method::POST, "/whispers")
            .query(&[("from_user_id", from), ("to_user_id", to)])
            .json(&json!({ "message": text }))
            .send()
            .await?;
        empty(response).await
    }

    /// The Shared Chat session a channel is in, if it is in one. Asked
    /// on joining, since the two events only say when one starts or
    /// ends.
    pub async fn shared_chat(&self, broadcaster_id: &str) -> Result<Vec<String>> {
        let response = self
            .request(reqwest::Method::GET, "/shared_chat/session")
            .query(&[("broadcaster_id", broadcaster_id)])
            .send()
            .await?;
        // A channel in no session answers with an empty list, and some
        // channels answer 404 instead.
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(Vec::new());
        }
        let page: Page<SharedSession> = parse(response).await?;
        Ok(page
            .data
            .into_iter()
            .next()
            .map(|session| {
                session
                    .participants
                    .into_iter()
                    .map(|p| p.broadcaster_id)
                    .collect()
            })
            .unwrap_or_default())
    }

    /// Whether a Hype Train is running right now, and at what level.
    pub async fn hype_train(&self, broadcaster_id: &str) -> Result<Option<u32>> {
        let response = self
            .request(reqwest::Method::GET, "/hypetrain/status")
            .query(&[("broadcaster_id", broadcaster_id)])
            .send()
            .await?;
        let page: Page<HypeTrainStatus> = parse(response).await?;
        Ok(page
            .data
            .into_iter()
            .next()
            .and_then(|status| status.current)
            .map(|current| current.level))
    }

    /// Runs an advertisement. Partners and affiliates only, and only the
    /// broadcaster: not their moderators. Answers with Twitch's own word
    /// on what it managed to serve.
    pub async fn start_commercial(&self, broadcaster_id: &str, seconds: u32) -> Result<String> {
        let response = self
            .request(reqwest::Method::POST, "/channels/commercial")
            .json(&json!({ "broadcaster_id": broadcaster_id, "length": seconds }))
            .send()
            .await?;
        let page: Page<Commercial> = parse(response).await?;
        Ok(page
            .data
            .into_iter()
            .next()
            .map(|c| c.message)
            .unwrap_or_default())
    }

    /// Pins a message to the top of the chat room. Twitch keeps one
    /// moderator-pinned message per channel, so this replaces whatever is
    /// pinned. Without a duration it stays until the stream ends.
    pub async fn pin_message(
        &self,
        broadcaster_id: &str,
        moderator_id: &str,
        message_id: &str,
        duration_seconds: Option<u32>,
    ) -> Result<()> {
        let duration = duration_seconds.map(|s| s.to_string());
        let mut query = vec![
            ("broadcaster_id", broadcaster_id),
            ("moderator_id", moderator_id),
            ("message_id", message_id),
        ];
        if let Some(duration) = &duration {
            query.push(("duration_seconds", duration));
        }
        let response = self
            .request(reqwest::Method::PUT, "/chat/pins")
            .query(&query)
            .send()
            .await?;
        empty(response).await
    }

    /// Takes the pinned message down.
    pub async fn unpin_message(&self, broadcaster_id: &str, moderator_id: &str) -> Result<()> {
        let response = self
            .request(reqwest::Method::DELETE, "/chat/pins")
            .query(&[
                ("broadcaster_id", broadcaster_id),
                ("moderator_id", moderator_id),
            ])
            .send()
            .await?;
        empty(response).await
    }

    /// Subscribes to an EventSub type, delivered over the given WebSocket
    /// session.
    pub async fn subscribe(
        &self,
        kind: &str,
        version: &str,
        condition: serde_json::Value,
        session_id: &str,
    ) -> Result<Subscription> {
        let body = json!({
            "type": kind,
            "version": version,
            "condition": condition,
            "transport": { "method": "websocket", "session_id": session_id },
        });
        let response = self
            .request(reqwest::Method::POST, "/eventsub/subscriptions")
            .json(&body)
            .send()
            .await?;
        let page: Page<Subscription> = parse(response).await?;
        page.data
            .into_iter()
            .next()
            .ok_or_else(|| Error::Api(format!("no subscription returned for {kind}")))
    }

    pub async fn unsubscribe(&self, id: &str) -> Result<()> {
        let response = self
            .request(reqwest::Method::DELETE, "/eventsub/subscriptions")
            .query(&[("id", id)])
            .send()
            .await?;
        empty(response).await
    }
}

async fn parse<T: serde::de::DeserializeOwned>(response: reqwest::Response) -> Result<T> {
    if response.status().is_success() {
        return Ok(response.json().await?);
    }
    Err(api_error(response).await)
}

async fn empty(response: reqwest::Response) -> Result<()> {
    if response.status().is_success() {
        return Ok(());
    }
    Err(api_error(response).await)
}

/// Turns a failed response into an error, keeping Twitch's own message and
/// separating "your token is no good" from everything else.
async fn api_error(response: reqwest::Response) -> Error {
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    let message = match serde_json::from_str::<HelixError>(&body) {
        Ok(error) => error
            .message
            .filter(|m| !m.is_empty())
            .or(error.error)
            .unwrap_or_else(|| format!("HTTP {status}")),
        Err(_) if body.is_empty() => format!("HTTP {status}"),
        Err(_) => format!("HTTP {status}: {body}"),
    };
    match status.as_u16() {
        401 => Error::Unauthorized(message),
        403 => Error::Forbidden(message),
        429 => Error::Api(format!("rate limited: {message}")),
        _ => Error::Api(message),
    }
}

#[cfg(test)]
mod counted_tests {
    use super::*;

    #[test]
    fn a_counted_page_reads_as_twitch_sends_it() {
        let followers: Counted<Follower> = serde_json::from_str(
            r#"{"total": 8,
                "data": [{"user_id": "11111", "user_name": "UserDisplayName",
                          "user_login": "userloginname", "followed_at": "2022-05-24T22:22:08Z"}],
                "pagination": {"cursor": "eyJiIjpudWxsLCJhIjp7Ik"}}"#,
        )
        .unwrap();
        assert_eq!(followers.total, 8);
        assert_eq!(followers.data[0].user_name, "UserDisplayName");
        assert_eq!(followers.data[0].followed_at, "2022-05-24T22:22:08Z");
        assert!(followers.pagination.unwrap().cursor.is_some());

        // Not a moderator: the number and nobody named.
        let counted: Counted<Follower> =
            serde_json::from_str(r#"{"total": 8, "data": [], "pagination": {}}"#).unwrap();
        assert_eq!((counted.total, counted.data.len()), (8, 0));

        let subs: Counted<SubscriptionEntry> = serde_json::from_str(
            r#"{"data": [{"broadcaster_id": "141981764", "gifter_id": "12826",
                          "gifter_login": "twitch", "gifter_name": "Twitch",
                          "is_gift": true, "tier": "1000", "plan_name": "Channel Subscription",
                          "user_id": "527115020", "user_name": "twitchgaming",
                          "user_login": "twitchgaming"}],
                "pagination": {"cursor": "xxxx"}, "total": 13, "points": 13}"#,
        )
        .unwrap();
        assert_eq!((subs.total, subs.points), (13, Some(13)));
        let sub = &subs.data[0];
        assert_eq!(
            (
                sub.user_name.as_str(),
                sub.tier.as_str(),
                sub.is_gift,
                sub.gifter_name.as_str()
            ),
            ("twitchgaming", "1000", true, "Twitch")
        );
    }
}
