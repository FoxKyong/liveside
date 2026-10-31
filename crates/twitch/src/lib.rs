//! Twitch integration: OAuth (Device Code Flow), EventSub WebSocket for
//! receiving chat and events, Helix API for sending and everything else.
//!
//! Knows nothing about frontends; produces typed Twitch data for `liveside-core`.

pub mod auth;
pub mod emotes;
pub mod eventsub;
pub mod helix;
pub mod irc;
pub mod mapping;

/// Client ID of this application, registered at dev.twitch.tv as a public
/// client (no secret). Twitch calls Client IDs public and lets them be
/// embedded; it also wants one per application, so a fork sets its own as
/// `twitch.client_id` in the settings.
pub const CLIENT_ID: &str = "09uqs9v83lyzlsvof4fladb109f0iz";

/// Access this client asks for when logging in:
/// read chat, write chat, and moderate where the account may.
pub const SCOPES: &[&str] = &[
    "user:read:chat",
    "user:write:chat",
    // /me, which only IRC sends: Helix's Send Chat Message takes no
    // commands. IRC logs in with chat:read and sends with chat:edit.
    "chat:read",
    "chat:edit",
    "moderator:manage:banned_users",
    "moderator:manage:chat_messages",
    // Moderator commands: room modes, announcements, warnings.
    "moderator:manage:chat_settings",
    "moderator:manage:announcements",
    "moderator:manage:warnings",
    // Broadcaster commands: /mod, /unmod, /vip, /unvip.
    "channel:manage:moderators",
    "channel:manage:vips",
    // The channel's own title and category, and stream markers:
    // /title, /game, /marker.
    "channel:manage:broadcast",
    // Pointing at another channel: /shoutout, /raid.
    "moderator:manage:shoutouts",
    "channel:manage:raids",
    // Advertisements: /commercial.
    "channel:edit:commercial",
    // Hearing what happens in a channel besides chat: follows, Shield
    // Mode, shoutouts (as a moderator) and advertisements (as the
    // broadcaster). Channels going live and changing need no scope.
    "moderator:read:followers",
    "moderator:read:shield_mode",
    "moderator:read:shoutouts",
    "channel:read:ads",
    // The AutoMod queue: seeing what was caught, and deciding it.
    "moderator:manage:automod",
    // The other queue: people asking to be let back in.
    "moderator:manage:unban_requests",
    // The moderation log wants the whole moderator:read family; these two
    // are the ones no other command already asks for.
    "moderator:read:moderators",
    "moderator:read:vips",
    // Holding back a wave of abuse: /shield, /blockterm, /monitor.
    "moderator:manage:shield_mode",
    "moderator:manage:blocked_terms",
    "moderator:manage:suspicious_users",
    // And the matching read: manage lets this client decide about a
    // suspicious user, but Twitch refuses both suspicious_user
    // subscriptions without the read scope as well — checked against a
    // live token on 2026-10-11, where every other subscription was
    // enabled and these two alone were missing.
    "moderator:read:suspicious_users",
    // Polls in the channel's own name: starting one, stopping it, and
    // hearing the running total.
    "channel:manage:polls",
    // And predictions: starting one, closing betting, and paying it out.
    "channel:manage:predictions",
    // What people spend their channel points on, and answering it. Twitch
    // allows answering only to the application that created the reward,
    // so this covers rewards made from here; the rest are read only.
    "channel:manage:redemptions",
    // Clipping what just happened, wherever this account may: a clip is
    // made from the channel being watched, not only one's own.
    "clips:edit",
    // Saying when the stream is next, and calling one off.
    "channel:manage:schedule",
    // The money: what a broadcaster keeps the window open for. All of
    // these are only ever seen in one's own channel.
    "channel:read:subscriptions",
    "bits:read",
    "channel:read:hype_train",
    "channel:read:goals",
    "channel:read:charity",
    // Pushing the next advertisement break back, which is the one thing
    // anybody wants to do about advertisements from here.
    "channel:manage:ads",
    // Whispers: reading them and sending them. Twitch wants a verified
    // phone number on the sending account, which it says only when a
    // send is refused.
    "user:read:whispers",
    "user:manage:whispers",
    // The channels this account moderates: the natural answer to "what
    // should I join".
    "user:read:moderated_channels",
    // The channels this account follows, and which of them stream: the
    // other answer to it.
    "user:read:follows",
    // Which Twitch emotes this account may write: a subscriber's emotes
    // from every channel, follower and bits emotes, for the list to pick
    // from. Without it the list can only offer every emote of the channel
    // on screen, usable or not.
    "user:read:emotes",
];

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug)]
pub enum Error {
    /// Logging in failed or was refused.
    Auth(String),
    /// Twitch rejected the token: expired, revoked, or wrong scopes.
    Unauthorized(String),
    /// The account may not do this (not a moderator, banned, ...).
    Forbidden(String),
    /// Twitch answered, but not with what was asked for.
    Api(String),
    Network(String),
    /// Reading or writing the token file failed.
    Storage(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Auth(m)
            | Error::Unauthorized(m)
            | Error::Forbidden(m)
            | Error::Api(m)
            | Error::Network(m)
            | Error::Storage(m) => f.write_str(m),
        }
    }
}

impl std::error::Error for Error {}

impl From<reqwest::Error> for Error {
    fn from(e: reqwest::Error) -> Self {
        Error::Network(e.to_string())
    }
}

impl From<serde_json::Error> for Error {
    fn from(e: serde_json::Error) -> Self {
        Error::Api(format!("unexpected response: {e}"))
    }
}
