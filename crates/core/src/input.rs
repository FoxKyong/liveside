//! Interpretation of lines typed by the user (`Command::Input`).
//!
//! Pure functions, no state: the engine turns the result into backend
//! requests.

use liveside_proto::{CommandError, ErrorKind, Login, MessageId, UserId};

use crate::backend::{BackendRequest, PeopleList, Role, RoomModeChange, Suspicion, UserTarget};

/// Default timeout length, as in the Twitch web chat.
const DEFAULT_TIMEOUT_SECS: u32 = 600;

/// What `/delay` sets without a length: Twitch's own recommendation.
const DEFAULT_CHAT_DELAY_SECS: u32 = 2;

/// What `/commercial` runs without a length, as the Twitch website does.
const DEFAULT_COMMERCIAL_SECS: u32 = 30;

/// Twitch's maximum timeout (two weeks).
const MAX_TIMEOUT_SECS: u32 = 1_209_600;

/// Parses `text` typed in `channel`. `reply_to` applies only if the text
/// ends up being sent as a message.
/// Commands the engine understands in a channel, for frontends to offer
/// and complete. Aliases included, since either spelling works.
pub const COMMANDS: &[&str] = &[
    "login",
    "logout",
    "join",
    "bot",
    "bots",
    "me",
    "part",
    "leave",
    "close",
    "timeout",
    "ban",
    "unban",
    "untimeout",
    "delete",
    "warn",
    "clear",
    "announce",
    "slow",
    "slowoff",
    "followers",
    "followersoff",
    "subscribers",
    "subscribersoff",
    "emoteonly",
    "emoteonlyoff",
    "uniquechat",
    "uniquechatoff",
    "title",
    "game",
    "category",
    "shoutout",
    "so",
    "raid",
    "unraid",
    "marker",
    "commercial",
    "pin",
    "unpin",
    "shield",
    "shieldoff",
    "delay",
    "delayoff",
    "blockterm",
    "unblockterm",
    "blockedterms",
    "mods",
    "vips",
    "banned",
    "followerlist",
    "sublist",
    "monitor",
    "unmonitor",
    "restrict",
    "unrestrict",
    "mod",
    "unmod",
    "vip",
    "unvip",
];

pub fn parse(
    channel: &UserId,
    text: &str,
    reply_to: Option<MessageId>,
) -> Result<Vec<BackendRequest>, CommandError> {
    let text = text.trim_end();
    if text.trim_start().is_empty() {
        return Err(invalid("message.empty", &[], "Empty message"));
    }

    let Some(rest) = text.strip_prefix('/') else {
        return Ok(vec![send(channel, text, reply_to)]);
    };
    // `//text` sends `/text` literally.
    if rest.starts_with('/') {
        return Ok(vec![send(channel, rest, reply_to)]);
    }

    let (name, args) = match rest.split_once(char::is_whitespace) {
        Some((name, args)) => (name, args.trim()),
        None => (rest, ""),
    };
    let channel = channel.clone();

    // Keep COMMANDS in step with the arms below: frontends complete from
    // that list, and a name only they know would just fail here.
    match name.to_ascii_lowercase().as_str() {
        // Not through Helix, which takes no commands: the backend sends
        // it over IRC.
        "me" if args.is_empty() => Err(invalid("usage.me", &[], "Usage: /me <what you do>")),
        "me" => Ok(vec![BackendRequest::SendAction {
            channel: channel.clone(),
            text: args.to_owned(),
        }]),
        // The engine answers these before the line gets here; a bot is
        // nothing the backend could be asked for.
        "bot" | "bots" => Err(invalid(
            "usage.bot",
            &[],
            "Usage: /bots, or /bot start|stop|log <name>",
        )),
        "join" => {
            let channels: Vec<_> = args
                .split(|c: char| c == ',' || c.is_whitespace())
                .filter(|s| !s.is_empty())
                .map(normalize_channel)
                .collect::<Result<_, _>>()?;
            if channels.is_empty() {
                return Err(invalid(
                    "usage.join",
                    &[],
                    "Usage: /join <channel>[,<channel>...]",
                ));
            }
            Ok(channels
                .into_iter()
                .map(|channel| BackendRequest::Join { channel })
                .collect())
        }
        // Logging in and out belong to the account, not to a channel, but
        // they are typed in whichever buffer happens to be on screen —
        // which in a graphical window is always a channel.
        "login" => Ok(vec![BackendRequest::Login]),
        "logout" => Ok(vec![BackendRequest::Logout]),
        "part" | "leave" | "close" => Ok(vec![BackendRequest::Part { channel }]),
        "timeout" => {
            let mut words = args.split_whitespace();
            let user = user_arg(words.next(), "Usage: /timeout <user> [duration] [reason]")?;
            let (seconds, reason_start) = match words.next() {
                Some(word) => match parse_duration(word) {
                    Some(secs) => (secs, 2),
                    None => (DEFAULT_TIMEOUT_SECS, 1),
                },
                None => (DEFAULT_TIMEOUT_SECS, 1),
            };
            if seconds == 0 || seconds > MAX_TIMEOUT_SECS {
                return Err(invalid(
                    "timeout.range",
                    &[],
                    "Timeout must be between 1 second and 2 weeks",
                ));
            }
            Ok(vec![BackendRequest::Timeout {
                channel,
                user,
                seconds,
                reason: rest_after(args, reason_start),
            }])
        }
        "ban" => {
            let user = user_arg(
                args.split_whitespace().next(),
                "Usage: /ban <user> [reason]",
            )?;
            Ok(vec![BackendRequest::Ban {
                channel,
                user,
                reason: rest_after(args, 1),
            }])
        }
        "warn" => {
            let (name, reason) = args.split_once(char::is_whitespace).unwrap_or((args, ""));
            let user = user_arg(
                Some(name).filter(|n| !n.is_empty()),
                "Usage: /warn <user> <reason>",
            )?;
            if reason.trim().is_empty() {
                // Twitch requires a reason: the user has to acknowledge it.
                return Err(invalid("usage.warn", &[], "Usage: /warn <user> <reason>"));
            }
            Ok(vec![BackendRequest::Warn {
                channel,
                user,
                reason: reason.trim().to_owned(),
            }])
        }
        "clear" => Ok(vec![BackendRequest::ClearChat { channel }]),
        "announce" => {
            // `/announce blue text` picks a colour, as on Twitch.
            let (color, text) = match args.split_once(char::is_whitespace) {
                Some((first, rest))
                    if matches!(
                        first.to_ascii_lowercase().as_str(),
                        "blue" | "green" | "orange" | "purple" | "primary"
                    ) =>
                {
                    (first.to_ascii_lowercase(), rest.trim())
                }
                _ => ("primary".to_owned(), args),
            };
            if text.is_empty() {
                return Err(invalid(
                    "usage.announce",
                    &[],
                    "Usage: /announce [blue|green|orange|purple] <text>",
                ));
            }
            Ok(vec![BackendRequest::Announce {
                channel,
                text: text.to_owned(),
                color,
            }])
        }
        "slow" => {
            let seconds = match args.split_whitespace().next() {
                None => 30, // Twitch's own default.
                Some(value) => value
                    .parse()
                    .map_err(|_| invalid("usage.slow", &[], "Usage: /slow [seconds]"))?,
            };
            Ok(vec![BackendRequest::SetRoomMode {
                channel,
                mode: RoomModeChange::Slow(Some(seconds)),
            }])
        }
        "slowoff" => Ok(vec![BackendRequest::SetRoomMode {
            channel,
            mode: RoomModeChange::Slow(None),
        }]),
        "followers" => {
            let minutes = match args.split_whitespace().next() {
                None => 0, // Any follower, however new.
                Some(value) => parse_minutes(value)?,
            };
            Ok(vec![BackendRequest::SetRoomMode {
                channel,
                mode: RoomModeChange::Followers(Some(minutes)),
            }])
        }
        "followersoff" => Ok(vec![BackendRequest::SetRoomMode {
            channel,
            mode: RoomModeChange::Followers(None),
        }]),
        "subscribers" => Ok(vec![BackendRequest::SetRoomMode {
            channel,
            mode: RoomModeChange::Subscribers(true),
        }]),
        "subscribersoff" => Ok(vec![BackendRequest::SetRoomMode {
            channel,
            mode: RoomModeChange::Subscribers(false),
        }]),
        "emoteonly" => Ok(vec![BackendRequest::SetRoomMode {
            channel,
            mode: RoomModeChange::EmoteOnly(true),
        }]),
        "emoteonlyoff" => Ok(vec![BackendRequest::SetRoomMode {
            channel,
            mode: RoomModeChange::EmoteOnly(false),
        }]),
        "uniquechat" | "r9kbeta" => Ok(vec![BackendRequest::SetRoomMode {
            channel,
            mode: RoomModeChange::UniqueChat(true),
        }]),
        "uniquechatoff" | "r9kbetaoff" => Ok(vec![BackendRequest::SetRoomMode {
            channel,
            mode: RoomModeChange::UniqueChat(false),
        }]),
        "mod" | "unmod" | "vip" | "unvip" => {
            let usage = format!("Usage: /{name} <user>");
            let user = user_arg(args.split_whitespace().next(), &usage)?;
            let on = !name.starts_with("un");
            let role = if name.ends_with("mod") {
                Role::Moderator
            } else {
                Role::Vip
            };
            Ok(vec![BackendRequest::SetRole {
                channel,
                user,
                role,
                on,
            }])
        }
        // The broadcaster's own things, which both frontends can type as
        // well as click: a command parsed here works in either.
        "clip" => Ok(vec![BackendRequest::CreateClip { channel }]),
        "channel" | "channelinfo" => Ok(vec![BackendRequest::ShowChannelInfo { channel }]),
        "rewards" => Ok(vec![BackendRequest::ShowRewards { channel }]),
        "schedule" => Ok(vec![BackendRequest::ShowSchedule { channel }]),
        "ads" => Ok(vec![BackendRequest::ShowAds { channel }]),
        "snooze" => Ok(vec![BackendRequest::SnoozeAd { channel }]),
        "user" | "who" => {
            let user = user_arg(args.split_whitespace().next(), "Usage: /user <name>")?;
            let login = match user {
                UserTarget::Login(login) => login,
                UserTarget::Id(id) => id.as_str().into(),
            };
            Ok(vec![BackendRequest::ShowUser {
                channel,
                user: login,
            }])
        }
        "newreward" => {
            let Some((cost, rest)) = args.trim().split_once(char::is_whitespace) else {
                return Err(invalid(
                    "usage.newreward",
                    &[],
                    "Usage: /newreward <cost> <name> [| question]",
                ));
            };
            let Ok(cost) = cost.parse::<u64>() else {
                return Err(invalid(
                    "usage.newreward",
                    &[],
                    "Usage: /newreward <cost> <name> [| question]",
                ));
            };
            let (title, prompt) = match rest.split_once('|') {
                Some((title, prompt)) => (title.trim(), prompt.trim()),
                None => (rest.trim(), ""),
            };
            if title.is_empty() {
                return Err(invalid(
                    "usage.newreward",
                    &[],
                    "Usage: /newreward <cost> <name> [| question]",
                ));
            }
            Ok(vec![BackendRequest::CreateReward {
                channel,
                title: title.to_owned(),
                cost,
                prompt: prompt.to_owned(),
            }])
        }
        // A poll and a prediction are written the same way: how long,
        // then the question and the choices, separated by bars.
        "poll" | "prediction" | "pred" => {
            let usage = match name {
                "poll" => "Usage: /poll <seconds> <question> | <choice> | <choice>",
                _ => "Usage: /prediction <seconds> <question> | <outcome> | <outcome>",
            };
            let code = match name {
                "poll" => "usage.poll",
                _ => "usage.prediction",
            };
            let Some((seconds, rest)) = args.trim().split_once(char::is_whitespace) else {
                return Err(invalid(code, &[], usage));
            };
            let Ok(seconds) = seconds.parse::<u32>() else {
                return Err(invalid(code, &[], usage));
            };
            let mut parts = rest.split('|').map(str::trim).filter(|p| !p.is_empty());
            let Some(title) = parts.next() else {
                return Err(invalid(code, &[], usage));
            };
            let rest: Vec<String> = parts.map(str::to_owned).collect();
            if rest.len() < 2 {
                return Err(invalid(code, &[], usage));
            }
            Ok(vec![match name {
                "poll" => BackendRequest::CreatePoll {
                    channel,
                    title: title.to_owned(),
                    choices: rest,
                    seconds,
                },
                _ => BackendRequest::CreatePrediction {
                    channel,
                    title: title.to_owned(),
                    outcomes: rest,
                    seconds,
                },
            }])
        }
        // Pausing a reward is what somebody stepping away from the
        // stream wants, and it is worth a command of its own: by name,
        // because a Twitch id is not something anybody types.
        "pausereward" | "resumereward" => {
            if args.trim().is_empty() {
                return Err(invalid(
                    "usage.pausereward",
                    &[],
                    "Usage: /pausereward <name of the reward>",
                ));
            }
            Ok(vec![BackendRequest::UpdateReward {
                channel,
                reward: args.trim().to_owned(),
                title: None,
                cost: None,
                prompt: None,
                enabled: None,
                paused: Some(name == "pausereward"),
            }])
        }
        "rewardcost" => {
            let Some((cost, reward)) = args.trim().split_once(char::is_whitespace) else {
                return Err(invalid(
                    "usage.rewardcost",
                    &[],
                    "Usage: /rewardcost <points> <name of the reward>",
                ));
            };
            let Ok(cost) = cost.parse::<u64>() else {
                return Err(invalid(
                    "usage.rewardcost",
                    &[],
                    "Usage: /rewardcost <points> <name of the reward>",
                ));
            };
            Ok(vec![BackendRequest::UpdateReward {
                channel,
                reward: reward.trim().to_owned(),
                title: None,
                cost: Some(cost),
                prompt: None,
                enabled: None,
                paused: None,
            }])
        }
        "endpoll" | "cancelpoll" => Ok(vec![BackendRequest::EndPoll {
            channel,
            archive: name == "cancelpoll",
        }]),
        "lock" | "cancelprediction" => Ok(vec![BackendRequest::EndPrediction {
            channel,
            how: match name == "lock" {
                true => liveside_proto::PredictionEnd::Lock,
                false => liveside_proto::PredictionEnd::Cancel,
            },
        }]),
        "unban" | "untimeout" => {
            let user = user_arg(args.split_whitespace().next(), "Usage: /unban <user>")?;
            Ok(vec![BackendRequest::Unban { channel, user }])
        }
        "title" => {
            if args.is_empty() {
                return Err(invalid("usage.title", &[], "Usage: /title <new title>"));
            }
            // Twitch's own limit, said before the request is made.
            if args.chars().count() > 140 {
                return Err(invalid(
                    "title.too_long",
                    &[],
                    "A stream title is at most 140 characters",
                ));
            }
            Ok(vec![BackendRequest::SetChannelInfo {
                channel,
                title: Some(args.to_owned()),
                game: None,
                language: None,
                tags: None,
                delay: None,
                labels: None,
            }])
        }
        "game" | "category" => {
            if args.is_empty() {
                return Err(invalid("usage.game", &[], "Usage: /game <category>"));
            }
            Ok(vec![BackendRequest::SetChannelInfo {
                channel,
                title: None,
                game: Some(args.to_owned()),
                language: None,
                tags: None,
                delay: None,
                labels: None,
            }])
        }
        "shoutout" | "so" => {
            let to = user_arg(args.split_whitespace().next(), "Usage: /shoutout <channel>")?;
            Ok(vec![BackendRequest::Shoutout { channel, to }])
        }
        "raid" => {
            let to = user_arg(args.split_whitespace().next(), "Usage: /raid <channel>")?;
            Ok(vec![BackendRequest::Raid { channel, to }])
        }
        "unraid" => Ok(vec![BackendRequest::CancelRaid { channel }]),
        "marker" => {
            // Twitch's own limit on the note that goes with a marker.
            if args.chars().count() > 140 {
                return Err(invalid(
                    "marker.too_long",
                    &[],
                    "A marker description is at most 140 characters",
                ));
            }
            Ok(vec![BackendRequest::Marker {
                channel,
                description: (!args.is_empty()).then(|| args.to_owned()),
            }])
        }
        "commercial" => {
            let seconds = match args.split_whitespace().next() {
                Some(word) => parse_duration(word).ok_or_else(|| {
                    invalid("usage.commercial", &[], "Usage: /commercial [seconds]")
                })?,
                None => DEFAULT_COMMERCIAL_SECS,
            };
            if !(1..=180).contains(&seconds) {
                return Err(invalid(
                    "commercial.range",
                    &[],
                    "A commercial runs for up to 180 seconds",
                ));
            }
            Ok(vec![BackendRequest::Commercial { channel, seconds }])
        }
        "shield" => Ok(vec![BackendRequest::ShieldMode { channel, on: true }]),
        "shieldoff" => Ok(vec![BackendRequest::ShieldMode { channel, on: false }]),
        "delay" => {
            let seconds = match args.split_whitespace().next() {
                Some(word) => word
                    .parse::<u32>()
                    .map_err(|_| invalid("usage.delay", &[], "Usage: /delay [2|4|6]"))?,
                None => DEFAULT_CHAT_DELAY_SECS,
            };
            // Twitch offers exactly these three; anything else is refused
            // with a bare 400.
            if ![2, 4, 6].contains(&seconds) {
                return Err(invalid(
                    "delay.range",
                    &[],
                    "The chat delay is 2, 4 or 6 seconds",
                ));
            }
            Ok(vec![BackendRequest::SetRoomMode {
                channel,
                mode: RoomModeChange::NonModeratorDelay(Some(seconds)),
            }])
        }
        "delayoff" => Ok(vec![BackendRequest::SetRoomMode {
            channel,
            mode: RoomModeChange::NonModeratorDelay(None),
        }]),
        "blockterm" | "unblockterm" => {
            let on = name == "blockterm";
            let usage = format!("Usage: /{name} <word or phrase>");
            if args.is_empty() {
                return Err(invalid(
                    "usage.blockterm",
                    &[("usage", &usage)],
                    usage.clone(),
                ));
            }
            // Twitch's own limits on a term.
            if args.chars().count() < 2 || args.chars().count() > 500 {
                return Err(invalid(
                    "blockterm.range",
                    &[],
                    "A blocked term is between 2 and 500 characters",
                ));
            }
            Ok(vec![BackendRequest::BlockTerm {
                channel,
                text: args.to_owned(),
                on,
            }])
        }
        "blockedterms" => Ok(vec![BackendRequest::ShowBlockedTerms { channel }]),
        // Not /followers and /subscribers, which are the chat modes.
        "mods" | "vips" | "banned" | "followerlist" | "sublist" => {
            let list = match name {
                "mods" => PeopleList::Moderators,
                "vips" => PeopleList::Vips,
                "followerlist" => PeopleList::Followers,
                "sublist" => PeopleList::Subscribers,
                _ => PeopleList::Banned,
            };
            Ok(vec![BackendRequest::ShowPeople { channel, list }])
        }
        "monitor" | "restrict" | "unmonitor" | "unrestrict" => {
            let usage = format!("Usage: /{name} <user>");
            let user = user_arg(args.split_whitespace().next(), &usage)?;
            let status = match name {
                "monitor" => Suspicion::Monitored,
                "restrict" => Suspicion::Restricted,
                _ => Suspicion::None,
            };
            Ok(vec![BackendRequest::SetSuspicious {
                channel,
                user,
                status,
            }])
        }
        "pin" => {
            let mut words = args.split_whitespace();
            let Some(id) = words.next() else {
                return Err(invalid(
                    "usage.pin",
                    &[],
                    "Usage: /pin <message id> [seconds]",
                ));
            };
            let seconds = match words.next() {
                Some(word) => {
                    let seconds = parse_duration(word).ok_or_else(|| {
                        invalid("usage.pin", &[], "Usage: /pin <message id> [seconds]")
                    })?;
                    // Twitch's own limits; saying so beats a refusal that
                    // only says "400".
                    if !(30..=1800).contains(&seconds) {
                        return Err(invalid(
                            "pin.range",
                            &[],
                            "A pinned message lasts between 30 seconds and 30 minutes",
                        ));
                    }
                    Some(seconds)
                }
                None => None,
            };
            Ok(vec![BackendRequest::Pin {
                channel,
                message: id.into(),
                seconds,
            }])
        }
        "unpin" => Ok(vec![BackendRequest::Unpin { channel }]),
        "delete" => match args.split_whitespace().next() {
            Some(id) => Ok(vec![BackendRequest::Delete {
                channel,
                message: id.into(),
            }]),
            None => Err(invalid("usage.delete", &[], "Usage: /delete <message id>")),
        },
        _ => Err(invalid(
            "command.unknown",
            &[("command", name)],
            format!("Unknown command /{name}"),
        )),
    }
}

/// Normalizes a channel as typed (`#Name`, `name`, twitch.tv URL) to a login.
/// What follows `/join` in a typed line, when it is one.
/// What follows `/bot`, or `list` for `/bots`: bots are the engine's to
/// run, not the backend's, so the engine answers these itself.
pub fn bot_args(text: &str) -> Option<&str> {
    let rest = text.trim().strip_prefix('/')?;
    let (name, args) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
    if name.eq_ignore_ascii_case("bots") {
        Some("list")
    } else if name.eq_ignore_ascii_case("bot") {
        Some(args.trim())
    } else {
        None
    }
}

pub fn join_args(text: &str) -> Option<&str> {
    let rest = text.trim().strip_prefix('/')?;
    let (name, args) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
    name.eq_ignore_ascii_case("join").then_some(args)
}

pub fn normalize_channel(input: &str) -> Result<Login, CommandError> {
    let s = input.trim();
    let s = s
        .strip_prefix("https://")
        .or_else(|| s.strip_prefix("http://"))
        .unwrap_or(s);
    let s = s.strip_prefix("www.").unwrap_or(s);
    let s = s.strip_prefix("twitch.tv/").unwrap_or(s);
    let s = s.trim_start_matches('#').trim_end_matches('/');
    let login = s.to_ascii_lowercase();
    if is_valid_login(&login) {
        Ok(Login(login))
    } else {
        Err(invalid(
            "channel.invalid_name",
            &[("name", input)],
            format!("Invalid channel name {input:?}"),
        ))
    }
}

/// Twitch logins: 1–25 characters of `a-z`, `0-9`, `_`.
fn is_valid_login(s: &str) -> bool {
    (1..=25).contains(&s.len())
        && s.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

/// Followers-only takes a duration: bare minutes, or `30m`, `2h`, `7d`,
/// the way /timeout takes one.
fn parse_minutes(value: &str) -> Result<u32, CommandError> {
    let usage = "Usage: /followers [minutes|30m|2h|7d]";
    let (number, unit) = value.split_at(
        value
            .find(|c: char| !c.is_ascii_digit())
            .unwrap_or(value.len()),
    );
    let number: u32 = number
        .parse()
        .map_err(|_| invalid("usage.followers", &[], usage))?;
    let minutes = match unit.to_ascii_lowercase().as_str() {
        "" | "m" => number,
        "h" => number.saturating_mul(60),
        "d" => number.saturating_mul(60 * 24),
        "w" => number.saturating_mul(60 * 24 * 7),
        _ => return Err(invalid("usage.followers", &[], usage)),
    };
    // Twitch's limit is three months.
    if minutes > 3 * 30 * 24 * 60 {
        return Err(invalid(
            "followers.too_long",
            &[],
            "Followers-only is at most 3 months",
        ));
    }
    Ok(minutes)
}

fn user_arg(word: Option<&str>, usage: &str) -> Result<UserTarget, CommandError> {
    let word = word.ok_or_else(|| invalid("usage.user_missing", &[("usage", usage)], usage))?;
    let login = word.trim_start_matches('@').to_ascii_lowercase();
    if is_valid_login(&login) {
        Ok(UserTarget::Login(Login(login)))
    } else {
        Err(invalid(
            "user.invalid_name",
            &[("name", word)],
            format!("Invalid user name {word:?}"),
        ))
    }
}

/// `600`, `30s`, `10m`, `2h`, `1d`, `1w` → seconds.
fn parse_duration(word: &str) -> Option<u32> {
    let (digits, unit) = match word.find(|c: char| !c.is_ascii_digit()) {
        Some(i) => word.split_at(i),
        None => (word, ""),
    };
    let n: u32 = digits.parse().ok()?;
    let factor = match unit {
        "" | "s" => 1,
        "m" => 60,
        "h" => 3600,
        "d" => 86_400,
        "w" => 604_800,
        _ => return None,
    };
    n.checked_mul(factor)
}

/// Everything after the first `skip` words, or `None` if nothing is left.
fn rest_after(args: &str, skip: usize) -> Option<String> {
    let mut rest = args;
    for _ in 0..skip {
        rest = rest.trim_start();
        rest = match rest.find(char::is_whitespace) {
            Some(i) => &rest[i..],
            None => "",
        };
    }
    let rest = rest.trim();
    (!rest.is_empty()).then(|| rest.to_owned())
}

fn send(channel: &UserId, text: &str, reply_to: Option<MessageId>) -> BackendRequest {
    BackendRequest::Send {
        channel: channel.clone(),
        text: text.to_owned(),
        reply_to,
    }
}

/// A complaint about what was typed, which a frontend can translate.
fn invalid(code: &str, args: &[(&str, &str)], message: impl Into<String>) -> CommandError {
    CommandError::new(ErrorKind::InvalidArgument, code, args, message)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ch() -> UserId {
        "1001".into()
    }

    fn one(text: &str) -> BackendRequest {
        let mut v = parse(&ch(), text, None).unwrap();
        assert_eq!(v.len(), 1, "{text}");
        v.remove(0)
    }

    fn err(text: &str) -> String {
        parse(&ch(), text, None).unwrap_err().message
    }

    fn login(s: &str) -> UserTarget {
        UserTarget::Login(s.into())
    }

    #[test]
    fn plain_text_is_sent() {
        assert_eq!(
            parse(&ch(), "hello there  ", Some("p1".into())).unwrap(),
            vec![BackendRequest::Send {
                channel: ch(),
                text: "hello there".into(),
                reply_to: Some("p1".into()),
            }]
        );
        assert_eq!(
            one("//shrug"),
            BackendRequest::Send {
                channel: ch(),
                text: "/shrug".into(),
                reply_to: None,
            }
        );
        assert_eq!(err("   "), "Empty message");
    }

    #[test]
    fn logging_in_and_out_works_from_a_channel_too() {
        // The window has no buffer without a channel, so a command that
        // only worked outside one could not be typed there at all.
        assert_eq!(one("/login"), BackendRequest::Login);
        assert_eq!(one("/logout"), BackendRequest::Logout);
    }

    #[test]
    fn join_accepts_lists_and_urls() {
        let joins = parse(&ch(), "/join #Foo, bar https://www.twitch.tv/Baz_1/", None).unwrap();
        let names: Vec<_> = joins
            .iter()
            .map(|r| match r {
                BackendRequest::Join { channel } => channel.as_str(),
                other => panic!("{other:?}"),
            })
            .collect();
        assert_eq!(names, ["foo", "bar", "baz_1"]);
        assert!(err("/join").starts_with("Usage"));
        assert!(err("/join bad-name").starts_with("Invalid channel"));
    }

    #[test]
    fn timeout_variants() {
        assert_eq!(
            one("/timeout @Viewer"),
            BackendRequest::Timeout {
                channel: ch(),
                user: login("viewer"),
                seconds: 600,
                reason: None,
            }
        );
        assert_eq!(
            one("/TIMEOUT viewer 10m  stop   spamming "),
            BackendRequest::Timeout {
                channel: ch(),
                user: login("viewer"),
                seconds: 600,
                reason: Some("stop   spamming".into()),
            }
        );
        // A non-duration second word starts the reason.
        assert_eq!(
            one("/timeout viewer spam bot"),
            BackendRequest::Timeout {
                channel: ch(),
                user: login("viewer"),
                seconds: 600,
                reason: Some("spam bot".into()),
            }
        );
        assert_eq!(
            one("/timeout viewer 1w"),
            BackendRequest::Timeout {
                channel: ch(),
                user: login("viewer"),
                seconds: 604_800,
                reason: None,
            }
        );
        assert!(err("/timeout viewer 3w").contains("2 weeks"));
        assert!(err("/timeout viewer 0").contains("2 weeks"));
        assert!(err("/timeout").starts_with("Usage"));
    }

    #[test]
    fn ban_unban_delete_part() {
        assert_eq!(
            one("/ban viewer being rude"),
            BackendRequest::Ban {
                channel: ch(),
                user: login("viewer"),
                reason: Some("being rude".into()),
            }
        );
        assert_eq!(
            one("/untimeout viewer"),
            BackendRequest::Unban {
                channel: ch(),
                user: login("viewer"),
            }
        );
        assert_eq!(
            one("/delete abc-123"),
            BackendRequest::Delete {
                channel: ch(),
                message: "abc-123".into(),
            }
        );
        assert_eq!(one("/part"), BackendRequest::Part { channel: ch() });
        assert_eq!(err("/frobnicate x"), "Unknown command /frobnicate");
    }

    #[test]
    fn durations() {
        assert_eq!(parse_duration("45"), Some(45));
        assert_eq!(parse_duration("45s"), Some(45));
        assert_eq!(parse_duration("2h"), Some(7200));
        assert_eq!(parse_duration("1d"), Some(86_400));
        assert_eq!(parse_duration("h"), None);
        assert_eq!(parse_duration("5x"), None);
        assert_eq!(parse_duration("99999999999"), None);
    }
    #[test]
    fn room_modes_change_one_setting_at_a_time() {
        let channel = UserId::from("1001");
        let mode = |text: &str| match parse(&channel, text, None).unwrap().as_slice() {
            [BackendRequest::SetRoomMode { mode, .. }] => mode.clone(),
            other => panic!("expected one room mode change, got {other:?}"),
        };
        assert_eq!(mode("/slow"), RoomModeChange::Slow(Some(30)));
        assert_eq!(mode("/slow 5"), RoomModeChange::Slow(Some(5)));
        assert_eq!(mode("/slowoff"), RoomModeChange::Slow(None));
        assert_eq!(mode("/followers"), RoomModeChange::Followers(Some(0)));
        assert_eq!(mode("/followers 30m"), RoomModeChange::Followers(Some(30)));
        assert_eq!(mode("/followers 2h"), RoomModeChange::Followers(Some(120)));
        assert_eq!(mode("/followersoff"), RoomModeChange::Followers(None));
        assert_eq!(mode("/subscribers"), RoomModeChange::Subscribers(true));
        assert_eq!(mode("/subscribersoff"), RoomModeChange::Subscribers(false));
        assert_eq!(mode("/emoteonly"), RoomModeChange::EmoteOnly(true));
        assert_eq!(mode("/uniquechat"), RoomModeChange::UniqueChat(true));
        // Chatty and Twitch both know this one under its old name.
        assert_eq!(mode("/r9kbetaoff"), RoomModeChange::UniqueChat(false));

        assert!(parse(&channel, "/slow abc", None).is_err());
        assert!(
            parse(&channel, "/followers 99d", None).is_err(),
            "over the limit"
        );
    }

    #[test]
    fn warnings_and_announcements() {
        let channel = UserId::from("1001");
        match parse(&channel, "/warn bob spamming links", None)
            .unwrap()
            .as_slice()
        {
            [BackendRequest::Warn { user, reason, .. }] => {
                assert_eq!(user, &UserTarget::Login("bob".into()));
                assert_eq!(reason, "spamming links");
            }
            other => panic!("{other:?}"),
        }
        // Twitch makes the user acknowledge the reason, so it is required.
        assert!(parse(&channel, "/warn bob", None).is_err());

        match parse(&channel, "/announce purple stream starts now", None)
            .unwrap()
            .as_slice()
        {
            [BackendRequest::Announce { text, color, .. }] => {
                assert_eq!(text, "stream starts now");
                assert_eq!(color, "purple");
            }
            other => panic!("{other:?}"),
        }
        // A first word that is not a colour is part of the text.
        match parse(&channel, "/announce hello there", None)
            .unwrap()
            .as_slice()
        {
            [BackendRequest::Announce { text, color, .. }] => {
                assert_eq!(text, "hello there");
                assert_eq!(color, "primary");
            }
            other => panic!("{other:?}"),
        }
        assert!(parse(&channel, "/announce", None).is_err());
    }

    #[test]
    fn roles_are_given_and_taken() {
        let channel = UserId::from("1001");
        let role = |text: &str| match parse(&channel, text, None).unwrap().as_slice() {
            [BackendRequest::SetRole { role, on, user, .. }] => (*role, *on, user.clone()),
            other => panic!("{other:?}"),
        };
        assert_eq!(
            role("/mod bob"),
            (Role::Moderator, true, UserTarget::Login("bob".into()))
        );
        assert!(!role("/unmod bob").1, "unmod takes it away");
        assert_eq!(role("/vip bob").0, Role::Vip);
        assert_eq!(
            role("/unvip bob"),
            (Role::Vip, false, UserTarget::Login("bob".into()))
        );
        assert!(parse(&channel, "/mod", None).is_err());
    }

    #[test]
    fn what_a_broadcaster_types_is_parsed_once_for_both_frontends() {
        // These used to live in the terminal frontend, where the window
        // could not reach them: typing /clip there answered "unknown
        // command" while the menu item worked.
        assert_eq!(one("/clip"), BackendRequest::CreateClip { channel: ch() });
        assert_eq!(
            one("/channel"),
            BackendRequest::ShowChannelInfo { channel: ch() }
        );
        assert_eq!(
            one("/rewards"),
            BackendRequest::ShowRewards { channel: ch() }
        );
        assert_eq!(
            one("/newreward 250 Hydrate! | What should I drink?"),
            BackendRequest::CreateReward {
                channel: ch(),
                title: "Hydrate!".into(),
                cost: 250,
                prompt: "What should I drink?".into(),
            }
        );
        assert_eq!(
            one("/poll 60 What next? | This | That"),
            BackendRequest::CreatePoll {
                channel: ch(),
                title: "What next?".into(),
                choices: vec!["This".into(), "That".into()],
                seconds: 60,
            }
        );
        assert_eq!(
            one("/prediction 120 Will I? | Yes | No"),
            BackendRequest::CreatePrediction {
                channel: ch(),
                title: "Will I?".into(),
                outcomes: vec!["Yes".into(), "No".into()],
                seconds: 120,
            }
        );
        assert_eq!(
            one("/lock"),
            BackendRequest::EndPrediction {
                channel: ch(),
                how: liveside_proto::PredictionEnd::Lock,
            }
        );
        // A poll wants two choices; one is a mistake worth saying.
        assert!(parse(&ch(), "/poll 60 What next? | Only this", None).is_err());
    }

    #[test]
    fn the_title_and_the_category_are_set_separately() {
        assert_eq!(
            one("/title Rust and coffee"),
            BackendRequest::SetChannelInfo {
                channel: ch(),
                title: Some("Rust and coffee".into()),
                game: None,
                // A command touches one field; the form in the window is
                // what sets the rest.
                language: None,
                tags: None,
                delay: None,
                labels: None,
            }
        );
        assert_eq!(
            one("/game Software and Game Development"),
            BackendRequest::SetChannelInfo {
                channel: ch(),
                title: None,
                game: Some("Software and Game Development".into()),
                language: None,
                tags: None,
                delay: None,
                labels: None,
            }
        );
        // The same command by the name the Twitch website uses.
        assert_eq!(one("/category Just Chatting"), one("/game Just Chatting"));
        assert!(err("/title").starts_with("Usage"));
        assert!(err("/game").starts_with("Usage"));
        let long = "x".repeat(141);
        assert!(err(&format!("/title {long}")).contains("140"));
    }

    #[test]
    fn the_stream_commands_take_a_channel_or_a_length() {
        assert_eq!(
            one("/shoutout @Someone"),
            BackendRequest::Shoutout {
                channel: ch(),
                to: login("someone"),
            }
        );
        assert_eq!(one("/so someone"), one("/shoutout someone"));
        assert_eq!(
            one("/raid someone"),
            BackendRequest::Raid {
                channel: ch(),
                to: login("someone"),
            }
        );
        assert_eq!(one("/unraid"), BackendRequest::CancelRaid { channel: ch() });
        assert_eq!(
            one("/marker"),
            BackendRequest::Marker {
                channel: ch(),
                description: None,
            }
        );
        assert_eq!(
            one("/marker good bit"),
            BackendRequest::Marker {
                channel: ch(),
                description: Some("good bit".into()),
            }
        );
        // Without a length, what the Twitch website runs.
        assert_eq!(
            one("/commercial"),
            BackendRequest::Commercial {
                channel: ch(),
                seconds: 30,
            }
        );
        assert_eq!(
            one("/commercial 3m"),
            BackendRequest::Commercial {
                channel: ch(),
                seconds: 180,
            }
        );
        assert!(err("/commercial 5m").contains("180"));
        assert!(err("/shoutout").starts_with("Usage"));
    }

    #[test]
    fn me_is_an_action_for_the_backend() {
        assert_eq!(
            one("/me waves at chat"),
            BackendRequest::SendAction {
                channel: ch(),
                text: "waves at chat".into(),
            }
        );
        assert_eq!(parse(&ch(), "/me", None).unwrap_err().code, "usage.me");
    }

    #[test]
    fn the_lists_each_ask_for_their_own() {
        assert_eq!(
            one("/mods"),
            BackendRequest::ShowPeople {
                channel: ch(),
                list: PeopleList::Moderators,
            }
        );
        assert_eq!(
            one("/vips"),
            BackendRequest::ShowPeople {
                channel: ch(),
                list: PeopleList::Vips,
            }
        );
        assert_eq!(
            one("/banned"),
            BackendRequest::ShowPeople {
                channel: ch(),
                list: PeopleList::Banned,
            }
        );
        assert_eq!(
            one("/followerlist"),
            BackendRequest::ShowPeople {
                channel: ch(),
                list: PeopleList::Followers,
            }
        );
        assert_eq!(
            one("/sublist"),
            BackendRequest::ShowPeople {
                channel: ch(),
                list: PeopleList::Subscribers,
            }
        );
        // The chat modes keep their names.
        assert!(!matches!(
            one("/followers"),
            BackendRequest::ShowPeople { .. }
        ));
    }

    #[test]
    fn the_protection_commands_go_where_they_should() {
        assert_eq!(
            one("/shield"),
            BackendRequest::ShieldMode {
                channel: ch(),
                on: true,
            }
        );
        assert_eq!(
            one("/shieldoff"),
            BackendRequest::ShieldMode {
                channel: ch(),
                on: false,
            }
        );
        // The chat delay is a chat setting like the other room modes.
        assert_eq!(
            one("/delay"),
            BackendRequest::SetRoomMode {
                channel: ch(),
                mode: RoomModeChange::NonModeratorDelay(Some(2)),
            }
        );
        assert_eq!(
            one("/delay 6"),
            BackendRequest::SetRoomMode {
                channel: ch(),
                mode: RoomModeChange::NonModeratorDelay(Some(6)),
            }
        );
        assert_eq!(
            one("/delayoff"),
            BackendRequest::SetRoomMode {
                channel: ch(),
                mode: RoomModeChange::NonModeratorDelay(None),
            }
        );
        // Twitch offers exactly three lengths.
        assert!(err("/delay 5").contains("2, 4"));

        assert_eq!(
            one("/blockterm some spoiler"),
            BackendRequest::BlockTerm {
                channel: ch(),
                text: "some spoiler".into(),
                on: true,
            }
        );
        assert_eq!(
            one("/unblockterm some spoiler"),
            BackendRequest::BlockTerm {
                channel: ch(),
                text: "some spoiler".into(),
                on: false,
            }
        );
        assert!(err("/blockterm x").contains("2 and 500"));
        assert_eq!(
            one("/blockedterms"),
            BackendRequest::ShowBlockedTerms { channel: ch() }
        );

        assert_eq!(
            one("/monitor @viewer"),
            BackendRequest::SetSuspicious {
                channel: ch(),
                user: login("viewer"),
                status: Suspicion::Monitored,
            }
        );
        assert_eq!(
            one("/restrict viewer"),
            BackendRequest::SetSuspicious {
                channel: ch(),
                user: login("viewer"),
                status: Suspicion::Restricted,
            }
        );
        // Both lifts are the same thing to Twitch: no status at all.
        assert_eq!(one("/unmonitor viewer"), one("/unrestrict viewer"));
    }

    #[test]
    fn pinning_takes_a_message_and_an_optional_time() {
        assert_eq!(
            one("/pin abc-123"),
            BackendRequest::Pin {
                channel: ch(),
                message: "abc-123".into(),
                seconds: None,
            }
        );
        assert_eq!(
            one("/pin abc-123 5m"),
            BackendRequest::Pin {
                channel: ch(),
                message: "abc-123".into(),
                seconds: Some(300),
            }
        );
        assert_eq!(one("/unpin"), BackendRequest::Unpin { channel: ch() });
        // Twitch's own limits, said before the request is made.
        assert!(err("/pin abc-123 10").contains("30 seconds"));
        assert!(err("/pin abc-123 1h").contains("30 minutes"));
        assert!(err("/pin").starts_with("Usage"));
    }

    #[test]
    fn every_command_in_the_list_is_understood() {
        // The list drives completion in the frontends, so a name in it that
        // the parser rejects would be a trap.
        let channel = UserId::from("1001");
        for name in COMMANDS {
            let text = format!("/{name}");
            let result = parse(&channel, &text, None);
            if let Err(e) = &result {
                assert_ne!(
                    e.message,
                    format!("Unknown command /{name}"),
                    "{name} is offered but not understood"
                );
            }
        }
    }
}
