//! What the frontend says, in the languages it knows.
//!
//! The text lives in a file per language, `crates/client/lang/<code>.toml`,
//! built into the program, so that a translator works on a plain list
//! rather than on Rust. Every piece of text is a field of [`Strings`]; a
//! line a language lacks is shown in English, and the tests find it first.
//! Values with `{name}` placeholders are filled by [`fill`].
//!
//! Only text this frontend owns lives here. Notices and errors made by the
//! engine arrive as finished sentences over the protocol, which is a design
//! mistake for a client that may serve several frontends at once; see
//! `docs/architecture.md`.

use std::collections::HashMap;
use std::sync::OnceLock;

/// A language the interface can be shown in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Language {
    #[default]
    English,
    Czech,
    Spanish,
    Russian,
    Ukrainian,
    Esperanto,
    Greek,
    Korean,
    Japanese,
    ChineseSimplified,
    ChineseTraditional,
    Thai,
    Vietnamese,
    Arabic,
}

impl Language {
    /// A language's code (`en`, `cs`, …), or `auto` to follow the
    /// environment.
    pub fn parse(name: &str) -> Option<Self> {
        let name = name.trim().to_ascii_lowercase();
        match name.as_str() {
            "auto" => Some(Self::detect()),
            // Written before there were codes for every language.
            "english" => Some(Self::English),
            "cz" | "czech" | "cestina" => Some(Self::Czech),
            _ => Self::ALL
                .into_iter()
                .find(|language| language.code().eq_ignore_ascii_case(&name)),
        }
    }

    /// The usual environment variables, most specific first.
    pub fn detect() -> Self {
        for name in ["LC_ALL", "LC_MESSAGES", "LANG"] {
            if let Ok(value) = std::env::var(name)
                && !value.is_empty()
            {
                return Self::from_locale(&value);
            }
        }
        Self::English
    }

    fn from_locale(locale: &str) -> Self {
        let mut parts = locale.split(['_', '.', '@']);
        let code = parts.next().unwrap_or("");
        // Chinese is written two ways, and the region says which: Taiwan,
        // Hong Kong and Macau read the Traditional characters.
        if code == "zh" {
            return match parts.next() {
                Some("TW" | "HK" | "MO" | "Hant") => Self::ChineseTraditional,
                _ => Self::ChineseSimplified,
            };
        }
        Self::ALL
            .into_iter()
            .find(|language| language.code() == code)
            .unwrap_or(Self::English)
    }

    /// Everything this language says, read from its file the first time
    /// it is asked for.
    pub fn strings(self) -> &'static Strings {
        static LOADED: [OnceLock<Strings>; Language::ALL.len()] =
            [const { OnceLock::new() }; Language::ALL.len()];
        let fallback = (self != Self::English).then(|| Self::English.strings());
        LOADED[self as usize].get_or_init(|| {
            // The files are built in and the tests read every one of them,
            // so one that does not parse never leaves the workshop.
            let file = toml::from_str(self.file())
                .unwrap_or_else(|error| panic!("lang/{}.toml: {error}", self.code()));
            Strings::from_file(file, fallback)
        })
    }

    /// The code the settings and the file's name use.
    pub fn code(self) -> &'static str {
        match self {
            Self::English => "en",
            Self::Czech => "cs",
            Self::Spanish => "es",
            Self::Russian => "ru",
            Self::Ukrainian => "uk",
            Self::Esperanto => "eo",
            Self::Greek => "el",
            Self::Korean => "ko",
            Self::Japanese => "ja",
            Self::ChineseSimplified => "zh-Hans",
            Self::ChineseTraditional => "zh-Hant",
            Self::Thai => "th",
            Self::Vietnamese => "vi",
            Self::Arabic => "ar",
        }
    }

    /// Every language there is, English first, in the order of the
    /// variants: [`Language::strings`] counts on it.
    pub const ALL: [Self; 14] = [
        Self::English,
        Self::Czech,
        Self::Spanish,
        Self::Russian,
        Self::Ukrainian,
        Self::Esperanto,
        Self::Greek,
        Self::Korean,
        Self::Japanese,
        Self::ChineseSimplified,
        Self::ChineseTraditional,
        Self::Thai,
        Self::Vietnamese,
        Self::Arabic,
    ];

    fn file(self) -> &'static str {
        match self {
            Self::English => include_str!("../lang/en.toml"),
            Self::Czech => include_str!("../lang/cs.toml"),
            Self::Spanish => include_str!("../lang/es.toml"),
            Self::Russian => include_str!("../lang/ru.toml"),
            Self::Ukrainian => include_str!("../lang/uk.toml"),
            Self::Esperanto => include_str!("../lang/eo.toml"),
            Self::Greek => include_str!("../lang/el.toml"),
            Self::Korean => include_str!("../lang/ko.toml"),
            Self::Japanese => include_str!("../lang/ja.toml"),
            Self::ChineseSimplified => include_str!("../lang/zh-Hans.toml"),
            Self::ChineseTraditional => include_str!("../lang/zh-Hant.toml"),
            Self::Thai => include_str!("../lang/th.toml"),
            Self::Vietnamese => include_str!("../lang/vi.toml"),
            Self::Arabic => include_str!("../lang/ar.toml"),
        }
    }
}

/// Fills `{name}` placeholders: `fill(s.joined, &[("channel", "abc")])`.
pub fn fill(pattern: &str, values: &[(&str, &str)]) -> String {
    let mut out = pattern.to_owned();
    for (name, value) in values {
        out = out.replace(&format!("{{{name}}}"), value);
    }
    out
}

/// A count with its noun: `one` for exactly one, `many` for every other
/// number. Languages that say a count another way give both the same.
pub fn counted(one: &str, many: &str, count: usize) -> String {
    let pattern = if count == 1 { one } else { many };
    fill(pattern, &[("count", &count.to_string())])
}

/// Declares [`Strings`] with a field per key of the files' `[ui]` table.
macro_rules! catalogue {
    ($($(#[$note:meta])* $key:ident,)*) => {
        /// Every sentence the frontend can show.
        pub struct Strings {
            $($(#[$note])* pub $key: &'static str,)*
            /// The language's name in itself, as it is listed where it is
            /// chosen.
            pub name: &'static str,
            /// Translated by a machine and not checked by anyone yet.
            pub machine: bool,
            /// How it is listed where it is chosen: its name, and for a
            /// machine translation a note saying so, in the language
            /// itself, since that is who reads it.
            pub listed: &'static str,
            /// Written right to left, so a window is laid out from the
            /// right. A terminal shows it as it can: one that does
            /// bidirectional text itself shows it right.
            pub rtl: bool,
            engine: HashMap<String, &'static str>,
        }

        /// The keys of `[ui]`, as `en.toml` lists them.
        #[cfg(test)]
        const KEYS: &[&str] = &[$(stringify!($key)),*];

        impl Strings {
            /// Takes each line from `ui`, or from `fallback` where `ui`
            /// lacks it.
            fn from_file(file: File, fallback: Option<&Strings>) -> Self {
                let mut ui = file.ui;
                let mut take = |key: &str, fallback: Option<&'static str>| {
                    ui.remove(key)
                        .map(String::leak)
                        .map(|text| &*text)
                        .or(fallback)
                        .unwrap_or_default()
                };
                Self {
                    $($key: take(stringify!($key), fallback.map(|f| f.$key)),)*
                    listed: match (&file.language.note, file.language.machine) {
                        (Some(note), true) => format!("{} ({note})", file.language.name).leak(),
                        _ => file.language.name.clone().leak(),
                    },
                    name: file.language.name.leak(),
                    machine: file.language.machine,
                    rtl: file.language.rtl,
                    engine: file
                        .engine
                        .into_iter()
                        .map(|(code, text)| (code, &*text.leak()))
                        .collect(),
                }
            }
        }
    };
}

catalogue! {
    // Header and status line.
    status_buffer,
    connected,
    connecting,
    offline,
    reconnecting,
    not_logged_in,
    /// Beside the turning wheel that says the client is waiting for an
    /// answer from Twitch.
    working,
    /// Said in the status buffer as the connection comes and goes.
    conn_server,
    conn_rejected,
    conn_lost,
    conn_trying,
    conn_backend,
    conn_retry,
    // Chat area.
    scrolled_back,
    /// On the button that returns to the newest line, with how many
    /// arrived while the user was reading back.
    catch_up,
    active_count,
    /// The same for exactly one, where a language says it otherwise.
    active_count_one,
    more_users,
    /// Before the first line of a new day.
    day_line,
    monday,
    tuesday,
    wednesday,
    thursday,
    friday,
    saturday,
    sunday,
    // Things the frontend says by itself.
    not_in_channel,
    /// The same in the message line of a window that is logged in,
    /// where logging in is no advice.
    not_in_channel_logged_in,
    reply_needs_channel,
    nothing_to_reply_to,
    moderation_needs_channel,
    no_message_selected,
    connection_closed,
    /// A window attached to a daemon that went away, and came back.
    engine_lost,
    engine_back,
    privacy_title,
    /// On the button that opens that report from the settings.
    privacy_open,
    /// The button beside it that opens the About window, and the line
    /// under the name there that says what Liveside is.
    about_open,
    about_summary,
    privacy_unknown,
    privacy_network,
    privacy_disk,
    privacy_on,
    privacy_off,
    // Settings screen.
    settings_title,
    settings_keys,
    settings_editing_keys,
    settings_restart,
    settings_saved,
    /// When nothing that needs a restart was changed.
    settings_saved_live,
    settings_save_failed,
    settings_none,
    row_reopen,
    row_reopen_help,
    row_highlight,
    row_highlight_help,
    row_theme,
    row_theme_help,
    theme_system,
    theme_light,
    theme_dark,
    row_animated,
    row_animated_help,
    section_general,
    section_security,
    section_streamlink,
    row_streamlink_command,
    row_streamlink_command_help,
    row_streamlink_quality,
    row_streamlink_quality_help,
    row_streamlink_player,
    row_streamlink_arguments,
    row_streamlink_arguments_help,
    streamlink_closed_at_once,
    row_streamlink_player_help,
    streamlink_watch,
    streamlink_started,
    streamlink_failed,
    streamlink_missing,
    streamlink_needs_channel,
    // Notifications and sounds.
    section_notify,
    section_rules,
    section_commands,
    section_stream_chat,
    row_stream_chat_seconds,
    row_stream_chat_seconds_help,
    row_stream_chat_background,
    row_stream_chat_background_help,
    row_stream_chat_font,
    row_stream_chat_font_help,
    stream_chat_bad_colour,
    stream_chat_open,
    stream_chat_title,
    row_hotkeys,
    row_hotkeys_help,
    hotkeys_count,
    hotkeys_count_one,
    hotkeys_empty,
    hotkeys_line_error,
    row_commands,
    row_commands_help,
    commands_count,
    commands_count_one,
    commands_empty,
    commands_line_error,
    row_highlight_rules,
    row_highlight_rules_help,
    row_ignore_rules,
    row_ignore_rules_help,
    row_filter_rules,
    row_filter_rules_help,
    rules_count,
    rules_count_one,
    rules_keys,
    rules_empty,
    rule_error,
    rules_list_highlight,
    rules_list_ignore,
    rules_list_filter,
    row_repeated_times,
    row_repeated_times_help,
    row_repeated_similarity,
    row_repeated_similarity_help,
    row_repeated_minutes,
    row_repeated_minutes_help,
    row_repeated_only,
    row_repeated_only_help,
    repeated_only_error,
    row_addressbook,
    row_addressbook_help,
    addressbook_count,
    addressbook_count_one,
    addressbook_empty,
    addressbook_line_error,
    ab_usage,
    ab_entry,
    ab_entry_none,
    ab_not_in,
    ab_removed,
    ab_info,
    ab_info_empty,
    ab_renamed,
    ab_category_removed,
    collected_open,
    collected_title,
    collected_highlighted,
    collected_ignored,
    collected_highlighted_none,
    collected_ignored_none,
    action_ignore,
    ignored_user,
    ignored_already,
    unignored_user,
    unignored_none,
    ignore_usage,
    unignore_usage,
    ignore_failed,
    row_notify_when,
    row_notify_when_help,
    notify_when_unseen,
    notify_when_unfocused,
    notify_when_always,
    row_notify_desktop,
    row_notify_desktop_help,
    row_notify_close_after,
    row_notify_close_after_help,
    row_notify_sound,
    row_notify_sound_help,
    row_notify_bell,
    row_notify_bell_help,
    row_notify_mention,
    row_notify_mention_help,
    row_notify_whisper,
    row_notify_whisper_help,
    row_notify_live,
    row_notify_followed,
    row_notify_followed_help,
    row_notify_live_help,
    row_notify_moderation,
    row_notify_moderation_help,
    row_notify_mention_sound,
    row_notify_whisper_sound,
    row_notify_live_sound,
    row_notify_moderation_sound,
    row_notify_file_help,
    notify_mention,
    notify_highlight,
    notify_whisper,
    notify_live,
    notify_live_body,
    notify_followed,
    notify_open,
    notify_followed_many,
    followed_started,
    notify_held,
    notify_unban,
    notify_desktop_failed,
    notify_sound_missing,
    notify_sound_failed,
    notify_no_player,
    /// In an empty sound file field.
    notify_sound_system,
    row_links,
    row_links_help,
    links_ask,
    links_open,
    links_off,
    links_terminal_ask,
    links_terminal_open,
    link_title,
    link_host,
    link_punycode,
    link_open,
    link_copy,
    link_cancel,
    link_tell,
    link_menu_check_tell,
    link_tell_failed,
    chat_link_heading,
    chat_link_foreign,
    chat_link_plain,
    chat_vt_clean,
    chat_vt_dangerous,
    chat_vt_warn,
    chat_vt_doubtful,
    chat_vt_ends,
    chat_vt_unknown,
    link_open_failed,
    link_no_browser,
    link_menu_open,
    link_menu_copy,
    row_virustotal,
    row_virustotal_help,
    row_virustotal_key,
    row_virustotal_key_help,
    key_set,
    key_not_set,
    key_new,
    vt_checking,
    vt_scanning,
    vt_unknown,
    vt_clean,
    vt_dangerous,
    vt_doubtful,
    vt_ends,
    link_menu_check,
    link_menu_follow,
    follow_checking,
    follow_leads,
    follow_stays,
    row_resolver,
    row_resolver_help,
    resolver_off,
    resolver_local,
    link_usage,
    search_placeholder,
    blocked_open,
    stream_stats,
    graph_title,
    graph_open,
    graph_summary,
    graph_point,
    graph_none,
    graph_marks,
    graph_keys,
    moderated_open,
    moderated_title,
    moderated_none,
    moderated_line,
    moderated_join,
    moderated_joined,
    followed_open,
    followed_title,
    followed_none,
    followed_heading,
    followed_offline,
    followed_live,
    followed_offline_heading,
    followed_refresh,
    followed_no_title,
    blocked_title,
    blocked_term,
    blocked_added,
    blocked_expires,
    blocked_never,
    blocked_remove,
    blocked_add,
    blocked_add_hint,
    blocked_empty,
    search_prompt,
    search_count,
    search_none,
    search_older,
    search_newer,
    link_none,
    link_menu_scan,
    row_user_list,
    row_user_list_help,
    row_user_list_active,
    row_user_list_irc,
    row_emotes,
    row_emotes_help,
    row_log,
    row_log_help,
    row_history,
    row_history_help,
    row_scrollback,
    row_scrollback_help,
    row_language,
    row_language_help,
    language_auto,
    on,
    off,
    // The graphical frontend.
    window_title,
    channels_heading,
    send_placeholder,
    popout_open,
    dock_open,
    dock_here,
    dock_close,
    channel_clear_here,
    /// Last in the same menu: what /part does.
    channel_leave,
    bots_button,
    bots_said,
    bots_title,
    bots_empty,
    bots_add,
    bots_edit,
    bots_remove,
    bots_start,
    bots_stop,
    bots_confirm_remove,
    bots_running,
    bots_stopped,
    bots_finished,
    bots_crashed,
    bots_killed,
    bots_failed,
    bots_elsewhere,
    bots_all_channels,
    bots_autostart_note,
    bots_log,
    bots_log_none,
    bot_new_title,
    bot_edit_title,
    bot_name,
    bot_command,
    bot_command_hint,
    bot_dir,
    bot_dir_hint,
    bot_channels,
    bot_channels_hint,
    bot_autostart,
    bot_save,
    bot_rate,
    bot_rate_note,
    bot_stop_after,
    bot_stop_after_note,
    bot_hear_bots,
    bot_hear_bots_note,
    bot_hear_ignored,
    bots_rate_shown,
    bots_hears_bots,
    bots_hears_ignored,
    bot_no_name,
    bot_no_command,
    bot_bad_command,
    bot_save_failed,
    popout_here,
    popout_back,
    popout_show,
    popout_title,
    /// The same line with nobody logged in: Twitch has no anonymous
    /// chat, so there is nothing to type but a command.
    send_logged_out,
    join_placeholder,
    join_button,
    /// The button in the channel panel, and the title of the small
    /// window it opens to ask for the name.
    join_open,
    join_title,
    login_button,
    /// The same button once there is an account to leave.
    logout_button,
    settings_button,
    settings_save,
    /// Shows or hides the list of people active in the channel.
    users_button,
    // Moderation.
    moderate_button,
    /// Heading of the moderation part of a message's menu.
    menu_for_message,
    /// Wraps a refusal that Twitch worded itself, which is not ours to
    /// translate, so that at least the reason for it is in the language.
    refused_by_twitch,
    /// Shown while the device-code login is waiting.
    login_pending,
    /// What became of an attempt to log in or out. These are said where
    /// the command was typed: a command that quietly works looks broken.
    logged_in_as,
    logged_out,
    login_failed,
    action_copy,
    action_copy_message,
    action_reply,
    action_delete,
    action_pin,
    /// Opens what is known about whoever wrote the message.
    action_card,
    /// Lets somebody back into the channel, from the list of the banned.
    action_unban,
    /// The AutoMod queue: what was caught, and the two decisions.
    held_message,
    held_allow,
    held_deny,
    held_approved,
    held_denied,
    held_expired,
    nothing_held,
    /// Requests to be let back in.
    unban_request,
    unban_approved,
    unban_denied,
    nothing_requested,
    /// The moderating panel: what waits, and what was done.
    review_button,
    review_queue,
    review_log,
    review_empty,
    /// One person's card.
    /// The lists of people a channel keeps.
    /// A poll: while it runs, and when it is over.
    poll_started,
    poll_standing,
    poll_result,
    poll_stopped,
    poll_hidden,
    poll_none,
    poll_usage,
    poll_open,
    poll_end,
    poll_cancel,
    poll_title,
    poll_choice,
    poll_minutes,
    poll_create,
    poll_votes,
    /// A prediction: betting, locked, paid out.
    pred_started,
    pred_standing,
    pred_locked,
    pred_paid,
    pred_canceled,
    pred_none,
    pred_usage,
    pred_payout_usage,
    pred_open,
    pred_lock,
    pred_cancel,
    pred_pay,
    pred_title,
    pred_outcome,
    pred_seconds,
    pred_create,
    pred_points,
    pred_waiting,
    /// What /prediction says the state is while betting is open.
    pred_betting,
    /// Channel point rewards and what people spend them on.
    reward_list,
    reward_none,
    reward_theirs,
    reward_queue,
    reward_queue_empty,
    reward_waiting,
    reward_done,
    reward_refunded,
    reward_fulfil,
    reward_refund,
    reward_usage,
    reward_new,
    reward_new_usage,
    reward_gone,
    /// The window that lists them and adds one.
    reward_window,
    reward_open,
    reward_name,
    reward_prompt,
    reward_add,
    reward_remove,
    reward_cost,
    /// Changing one that is already there.
    reward_edit,
    reward_pause,
    reward_resume,
    reward_paused,
    reward_off,
    reward_save,
    /// Clipping what just happened.
    clip_make,
    /// Whispers.
    /// Why Twitch does not trust somebody, said on their message.
    suspicion_evader,
    suspicion_shared,
    suspicion_flagged,
    suspicion_monitored,
    suspicion_restricted,
    list_separator,
    whisper_usage,
    whisper_with,
    whisper_send,
    /// What the channel says about itself: the form behind /title.
    channel_open,
    channel_window,
    channel_title,
    channel_game,
    channel_language,
    channel_tags,
    channel_tags_help,
    channel_delay,
    channel_delay_help,
    channel_labels,
    channel_save,
    channel_said,
    /// When the channel has set neither a title nor a category, which a
    /// bare dash would say worse than nothing.
    channel_said_nothing,
    /// The schedule: what the channel says it will be streaming.
    schedule_open,
    schedule_window,
    schedule_none,
    schedule_line,
    schedule_weekly,
    schedule_canceled,
    schedule_away,
    schedule_cancel,
    schedule_restore,
    schedule_delete,
    schedule_add,
    schedule_start,
    schedule_minutes,
    schedule_title,
    schedule_every_week,
    schedule_vacation_on,
    schedule_vacation_off,
    list_mods,
    list_vips,
    list_banned,
    list_empty,
    list_followers,
    list_followers_unnamed,
    list_subscribers,
    list_shown,
    list_since,
    list_tier,
    list_tier_gift,
    list_more,
    followers_open,
    subscribers_open,
    card_user,
    card_created,
    card_follows,
    card_not_following,
    card_subscribed,
    card_sub_gift,
    card_not_subscribed,
    card_banned,
    card_timed_out,
    card_messages,
    card_unknown,
    /// Room-wide items: the pinned message and Twitch's panic button.
    action_unpin,
    shield_on,
    shield_off,
    action_timeout,
    action_ban,
    confirm_ban,
    confirm,
    cancel,
    mode_emote_only,
    mode_subscribers,
    mode_followers,
    mode_slow,
    mode_unique,
    clear_chat,
    replying_to,
}

/// A language's file as it is written.
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    language: About,
    #[serde(default)]
    ui: HashMap<String, String>,
    /// What the engine's codes say; a code missing here is shown in the
    /// engine's own English.
    #[serde(default)]
    engine: HashMap<String, String>,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct About {
    name: String,
    #[serde(default)]
    machine: bool,
    /// What a machine translation is called, in the language itself.
    #[serde(default)]
    note: Option<String>,
    #[serde(default)]
    rtl: bool,
}

impl Strings {
    /// What the engine's code says in this language, if it says anything.
    pub fn engine(&self, code: &str) -> Option<&'static str> {
        self.engine.get(code).copied()
    }
}

/// What to say about how Streamlink ended, if anything: nothing for a
/// stream watched and closed, the reason for one that never played.
pub fn streamlink_ended(
    strings: &'static Strings,
    channel: &str,
    outcome: &crate::streamlink::Outcome,
) -> Option<String> {
    use crate::streamlink::Outcome;
    let error = match outcome {
        Outcome::Done => return None,
        Outcome::Failed(error) => error.clone(),
        Outcome::ClosedAtOnce { seconds, said } => fill(
            strings.streamlink_closed_at_once,
            &[("seconds", &seconds.to_string()), ("said", said)],
        ),
    };
    Some(fill(
        strings.streamlink_failed,
        &[("channel", channel), ("error", &error)],
    ))
}

/// How a stream stands, in a few words: "viewers: 1 234 · live 2:05".
/// The time is hours and minutes since it started, as of `now`.
pub fn stream_stats(
    strings: &'static Strings,
    stats: &liveside_proto::model::StreamStats,
    now: liveside_proto::Timestamp,
) -> String {
    let minutes = (now.0 - stats.started.0).max(0) / 60_000;
    let uptime = format!("{}:{:02}", minutes / 60, minutes % 60);
    fill(
        strings.stream_stats,
        &[("viewers", &grouped(stats.viewers)), ("uptime", &uptime)],
    )
}

/// How a stream's audience has gone, in a line: now, the most and when,
/// and the average. `clock` writes a time the way the frontend does.
pub fn viewer_summary(
    strings: &'static Strings,
    history: &liveside_proto::model::ViewerHistory,
    clock: impl Fn(liveside_proto::Timestamp) -> String,
) -> String {
    let (Some(last), Some(peak), Some(average)) =
        (history.samples.last(), history.peak(), history.average())
    else {
        return strings.graph_none.to_owned();
    };
    fill(
        strings.graph_summary,
        &[
            ("now", &grouped(last.viewers)),
            ("peak", &grouped(peak.viewers)),
            ("time", &clock(peak.time)),
            ("average", &grouped(average)),
        ],
    )
}

/// A change of title or category, as the graph lists it.
pub fn stream_mark(mark: &liveside_proto::model::StreamMark) -> String {
    match (mark.title.is_empty(), mark.game.is_empty()) {
        (false, false) => format!("{} — {}", mark.title, mark.game),
        (false, true) => mark.title.clone(),
        _ => mark.game.clone(),
    }
}

/// A count with its thousands apart, the way both languages here read
/// it: 1 234, 12 345.
pub fn grouped(n: u32) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            // A no-break space, so a count never breaks across lines.
            out.push('\u{a0}');
        }
        out.push(c);
    }
    out
}

/// How a link's check reads, for the colour it is shown in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// Not answered yet.
    Waiting,
    /// No engine found anything.
    Clean,
    /// An engine or two call it malicious or suspicious, which is as
    /// often a false alarm as not.
    Doubtful,
    /// Enough engines call it malicious or suspicious to take seriously.
    Dangerous,
    /// No report, or no answer at all.
    Unknown,
    /// A fact rather than a judgement: where a link leads.
    Info,
}

/// What a check of a link says, in a few words to put after the link.
pub fn link_report(
    strings: &'static Strings,
    source: liveside_proto::model::LinkSource,
    state: &liveside_proto::model::LinkState,
) -> (String, Verdict) {
    use liveside_proto::model::{LinkSource, LinkState};
    match state {
        LinkState::Checking if source == LinkSource::Redirects => {
            (strings.follow_checking.to_owned(), Verdict::Waiting)
        }
        LinkState::Checking => (strings.vt_checking.to_owned(), Verdict::Waiting),
        LinkState::Leads {
            final_url: Some(end),
            redirects,
            ..
        } => (
            fill(
                strings.follow_leads,
                &[
                    ("url", end),
                    ("count", &redirects.len().saturating_sub(1).to_string()),
                ],
            ),
            Verdict::Info,
        ),
        LinkState::Leads {
            final_url: None,
            status,
            ..
        } => (
            fill(strings.follow_stays, &[("status", &status.to_string())]),
            Verdict::Info,
        ),
        LinkState::Scanning => (strings.vt_scanning.to_owned(), Verdict::Waiting),
        LinkState::Unknown => (strings.vt_unknown.to_owned(), Verdict::Unknown),
        LinkState::Failed { code, detail } => {
            (from_engine(strings, code, &[], detail), Verdict::Unknown)
        }
        LinkState::Found {
            malicious,
            suspicious,
            engines,
            final_url,
            ..
        } => {
            let engines = engines.to_string();
            let counts = [
                ("malicious", malicious.to_string()),
                ("suspicious", suspicious.to_string()),
                ("engines", engines.clone()),
            ];
            let counts: Vec<(&str, &str)> = counts.iter().map(|(k, v)| (*k, v.as_str())).collect();
            use crate::links::Findings;
            let (mut said, verdict) = match crate::links::findings(*malicious, *suspicious) {
                Findings::None => (
                    fill(strings.vt_clean, &[("engines", &engines)]),
                    Verdict::Clean,
                ),
                Findings::Few => (fill(strings.vt_doubtful, &counts), Verdict::Doubtful),
                Findings::Dangerous => (fill(strings.vt_dangerous, &counts), Verdict::Dangerous),
            };
            // Where it really goes is the other half of the answer, and
            // the address is never shortened.
            if let Some(end) = final_url {
                said.push_str(" · ");
                said.push_str(&fill(strings.vt_ends, &[("url", end)]));
            }
            (said, verdict)
        }
    }
}

/// The heading of a list of people: how many, and for a list too long to
/// be sent whole, how many there are in all.
pub fn people_heading(
    strings: &Strings,
    list: liveside_proto::event::PeopleList,
    shown: usize,
    total: Option<u64>,
    points: Option<u64>,
) -> String {
    use liveside_proto::event::PeopleList;
    let count = shown.to_string();
    let total_text = total.unwrap_or(shown as u64).to_string();
    let heading = match list {
        PeopleList::Moderators => fill(strings.list_mods, &[("count", &count)]),
        PeopleList::Vips => fill(strings.list_vips, &[("count", &count)]),
        PeopleList::Banned => fill(strings.list_banned, &[("count", &count)]),
        // Counted, but nobody named: Twitch tells only the channel and
        // its moderators who follows.
        PeopleList::Followers if shown == 0 && total.is_some_and(|t| t > 0) => {
            return fill(strings.list_followers_unnamed, &[("total", &total_text)]);
        }
        PeopleList::Followers => fill(strings.list_followers, &[("total", &total_text)]),
        PeopleList::Subscribers => fill(
            strings.list_subscribers,
            &[
                ("total", &total_text),
                ("points", &points.unwrap_or(0).to_string()),
            ],
        ),
    };
    match total {
        Some(total) if (shown as u64) < total && shown > 0 => format!(
            "{heading} ({})",
            fill(strings.list_shown, &[("count", &count)])
        ),
        _ => heading,
    }
}

/// What a list says about one person beside the name, if anything.
pub fn person_detail(
    strings: &Strings,
    list: liveside_proto::event::PeopleList,
    person: &liveside_proto::event::Person,
) -> Option<String> {
    use liveside_proto::event::PeopleList;
    if person.detail.is_empty() {
        return None;
    }
    // Twitch writes times down to the second; the day is what matters.
    let day = person.detail.split('T').next().unwrap_or(&person.detail);
    Some(match list {
        PeopleList::Banned => fill(strings.card_timed_out, &[("date", day)]),
        PeopleList::Followers => fill(strings.list_since, &[("date", day)]),
        PeopleList::Subscribers if !person.gifted_by.is_empty() => fill(
            strings.list_tier_gift,
            &[("tier", &person.detail), ("user", &person.gifted_by)],
        ),
        PeopleList::Subscribers => fill(strings.list_tier, &[("tier", &person.detail)]),
        PeopleList::Moderators | PeopleList::Vips => person.detail.clone(),
    })
}

/// Why somebody is worth watching, in the reader's own language. Both
/// frontends say it the same way, so it lives here rather than twice.
pub fn suspicion(
    strings: &'static Strings,
    suspicion: &liveside_proto::model::Suspicion,
) -> String {
    use liveside_proto::model::SuspicionStatus;
    let mut said: Vec<String> = Vec::new();
    match suspicion.status {
        SuspicionStatus::Monitored => said.push(strings.suspicion_monitored.to_owned()),
        SuspicionStatus::Restricted => said.push(strings.suspicion_restricted.to_owned()),
        SuspicionStatus::None => {}
    }
    for reason in &suspicion.reasons {
        match reason.as_str() {
            "ban_evader" => said.push(strings.suspicion_evader.to_owned()),
            "banned_in_shared_channel" => said.push(fill(
                strings.suspicion_shared,
                &[("count", &suspicion.shared_bans.to_string())],
            )),
            "manually_added" => said.push(strings.suspicion_flagged.to_owned()),
            // A reason this version does not know is still a reason.
            other => said.push(other.to_owned()),
        }
    }
    said.join(strings.list_separator)
}

/// Says what the engine sent, in this language when it knows the code and
/// in the engine's own English when it does not. Text from Twitch has no
/// code and comes through untouched.
pub fn from_engine(
    strings: &Strings,
    code: &str,
    args: &[(String, String)],
    fallback: &str,
) -> String {
    let Some(pattern) = strings.engine(code) else {
        return fallback.to_owned();
    };
    let values: Vec<(&str, &str)> = args
        .iter()
        .map(|(name, value)| (name.as_str(), value.as_str()))
        .collect();
    fill(pattern, &values)
}

/// What is wrong with a rule, in the reader's language.
pub fn rule_problem(strings: &Strings, error: &liveside_core::rules::RuleError) -> String {
    let args: Vec<(String, String)> = error
        .args
        .iter()
        .map(|(name, value)| ((*name).to_owned(), value.clone()))
        .collect();
    from_engine(strings, error.code, &args, &error.message)
}

/// What is wrong with a name or a category in the address book, in the
/// reader's language.
pub fn book_problem(strings: &Strings, error: &liveside_core::addressbook::BookError) -> String {
    let args: Vec<(String, String)> = error
        .args
        .iter()
        .map(|(name, value)| ((*name).to_owned(), value.clone()))
        .collect();
    from_engine(strings, error.code, &args, &error.message)
}

/// What is wrong with a custom command, in the reader's language.
pub fn custom_problem(
    strings: &Strings,
    error: &liveside_core::commands::CommandProblem,
) -> String {
    let args: Vec<(String, String)> = error
        .args
        .iter()
        .map(|(name, value)| ((*name).to_owned(), value.clone()))
        .collect();
    from_engine(strings, error.code, &args, &error.message)
}

/// Which rule in which list is wrong, and how, in the reader's language.
pub fn rule_error(strings: &Strings, error: &liveside_core::rules::ListError) -> String {
    let list = match error.list {
        "highlight" => strings.rules_list_highlight,
        "ignore" => strings.rules_list_ignore,
        _ => strings.rules_list_filter,
    };
    fill(
        strings.rule_error,
        &[
            ("list", list),
            ("number", &error.number.to_string()),
            ("rule", &error.rule),
            ("error", &rule_problem(strings, &error.error)),
        ],
    )
}

#[cfg(test)]
mod tests {

    #[test]
    fn stream_stats_read_as_a_count_and_a_time() {
        let stats = liveside_proto::model::StreamStats {
            viewers: 12_345,
            started: liveside_proto::Timestamp(0),
        };
        assert_eq!(
            super::stream_stats(
                Language::English.strings(),
                &stats,
                liveside_proto::Timestamp((2 * 60 + 5) * 60_000 + 59_000)
            ),
            "viewers: 12\u{a0}345 · live 2:05"
        );
        assert_eq!(super::grouped(999), "999");
        assert_eq!(super::grouped(1_000), "1\u{a0}000");
    }

    use super::*;

    #[test]
    fn locales_map_to_languages() {
        assert_eq!(Language::from_locale("cs_CZ.UTF-8"), Language::Czech);
        assert_eq!(Language::from_locale("cs"), Language::Czech);
        assert_eq!(Language::from_locale("en_GB.UTF-8"), Language::English);
        assert_eq!(Language::from_locale("es_MX.UTF-8"), Language::Spanish);
        assert_eq!(
            Language::from_locale("zh_CN.UTF-8"),
            Language::ChineseSimplified
        );
        assert_eq!(Language::from_locale("zh_SG"), Language::ChineseSimplified);
        assert_eq!(
            Language::from_locale("zh_TW.UTF-8"),
            Language::ChineseTraditional
        );
        assert_eq!(Language::from_locale("zh_HK"), Language::ChineseTraditional);
        // Anything unknown falls back rather than failing to start.
        assert_eq!(Language::from_locale("sv_SE"), Language::English);
        assert_eq!(Language::from_locale(""), Language::English);
    }

    #[test]
    fn settings_names_are_understood() {
        assert_eq!(Language::parse("cs"), Some(Language::Czech));
        assert_eq!(Language::parse("EN"), Some(Language::English));
        assert_eq!(Language::parse("es"), Some(Language::Spanish));
        assert_eq!(
            Language::parse("zh-hant"),
            Some(Language::ChineseTraditional)
        );
        // Names written before the codes.
        assert_eq!(Language::parse("czech"), Some(Language::Czech));
        assert_eq!(Language::parse("čeština"), None, "only the codes");
        assert_eq!(Language::parse("klingon"), None);
    }

    #[test]
    fn engine_codes_are_said_in_both_languages() {
        let args = [("channel".to_owned(), "orbitcast".to_owned())];
        assert_eq!(
            from_engine(
                Language::English.strings(),
                "channel.unknown",
                &args,
                "No channel #orbitcast"
            ),
            "No channel #orbitcast"
        );
        assert_eq!(
            from_engine(
                Language::Czech.strings(),
                "channel.unknown",
                &args,
                "No channel #orbitcast"
            ),
            "Kanál #orbitcast neexistuje"
        );
        // Twitch's own words have no code and are shown as they came.
        assert_eq!(
            from_engine(
                Language::Czech.strings(),
                "",
                &[],
                "The user may not be banned"
            ),
            "The user may not be banned"
        );
        // A code this version does not know is not a reason to say nothing.
        assert_eq!(
            from_engine(
                Language::Czech.strings(),
                "from.the.future",
                &[],
                "something happened"
            ),
            "something happened"
        );
    }

    #[test]
    fn the_engine_codes_are_said_in_every_language() {
        // A code the engine sends that a language does not know shows the
        // engine's English in the middle of a translated interface.
        for code in [
            "channel.unknown",
            "channel.not_joined",
            "channel.invalid_name",
            "channel.broken",
            "bot.no_name",
            "bot.no_command",
            "bot.twice",
            "bot.unknown",
            "bot.running",
            "bot.not_running",
            "bot.started",
            "bot.stopping",
            "bot.stopped",
            "bot.ended",
            "bot.killed",
            "bot.cannot_start",
            "bot.elsewhere",
            "bot.not_reading",
            "bot.refused",
            "bot.not_understood",
            "bot.not_its_channel",
            "bot.not_joined",
            "bot.too_fast",
            "bot.rate",
            "bot.stop_after",
            "bot.none",
            "bot.is_running",
            "bot.is_stopped",
            "bot.is_finished",
            "bot.is_crashed",
            "bot.is_failed",
            "bot.is_elsewhere",
            "bot.log",
            "bot.output",
            "usage.bot",
            "user.unknown",
            "user.invalid_name",
            "account.required",
            "account.required_to_send",
            "login.in_progress",
            "twitch.not_connected",
            "twitch.missing_scope",
            "twitch.subscribe_failed",
            "twitch.budget_full",
            "logout.not_revoked",
            "account.old_permissions",
            "message.empty",
            "command.unknown",
            "usage.join",
            "rule.category",
            "addressbook.name",
            "addressbook.category",
            "join.category_empty",
            "join.category_option",
            "join.favourites",
            "usage.me",
            "me.needs_login",
            "me.refused",
            "me.closed",
            "hotkey.key",
            "hotkey.typing",
            "hotkey.reserved",
            "hotkey.empty",
            "hotkey.twice",
            "hotkey.missing",
            "custom.name",
            "custom.quote",
            "custom.bracket",
            "custom.function",
            "custom.identifier",
            "custom.syntax",
            "custom.missing",
            "usage.warn",
            "usage.announce",
            "usage.slow",
            "usage.delete",
            "usage.followers",
            "usage.user_missing",
            "timeout.range",
            "pin.range",
            "usage.pin",
            "usage.title",
            "usage.game",
            "usage.commercial",
            "usage.delay",
            "delay.range",
            "usage.blockterm",
            "blockterm.range",
            "blockterm.unknown",
            "blockterm.none",
            "blockterm.list",
            "commercial.range",
            "marker.too_long",
            "marker.set",
            "title.too_long",
            "channel.not_yours",
            "game.unknown",
            "channel.title_set",
            "channel.game_set",
            "followers.too_long",
            "emotes.unreadable",
            "settings.unreadable",
            "token.unreadable",
            "userlist.closed",
            "history.unreadable",
            "history.write_failed",
            "eventsub.revoked",
            "event.follow",
            "event.stream_online",
            "event.stream_offline",
            "event.channel_update",
            "event.shield_on",
            "event.shield_off",
            "event.shoutout_sent",
            "event.shoutout_received",
            "event.ad_break",
            "mod.suspicion_monitored",
            "mod.suspicion_restricted",
            "mod.suspicion_cleared",
            "clip.created",
            "token.renew_failed",
            "token.other_client",
            "token.restore_failed",
            "token.unchecked",
            "shared.begin",
            "shared.update",
            "money.charity_start",
            "ads.next",
            "demo.joined",
            "money.subscribe",
            "money.resubscribe",
            "money.gift",
            "money.cheer",
            "money.goal_begin",
            "reward.unknown",
            "usage.poll",
            "usage.prediction",
            "usage.newreward",
            "event.redemption",
            "event.redemption_text",
            "mod.ban",
            "mod.unban",
            "mod.timeout",
            "mod.untimeout",
            "mod.delete",
            "mod.warn",
            "mod.mod",
            "mod.unmod",
            "mod.vip",
            "mod.unvip",
            "mod.raid",
            "mod.emoteonly",
            "mod.emoteonlyoff",
            "mod.subscribers",
            "mod.subscribersoff",
            "mod.uniquechat",
            "mod.uniquechatoff",
            "mod.followers",
            "mod.followersoff",
            "mod.slow",
            "mod.slowoff",
            "mod.clear",
            "mod.other",
            "event.own_held",
            "event.own_allowed",
            "event.own_denied",
            "event.own_dropped",
            "backend.stopped",
            "internal.unknown_request",
        ] {
            for language in Language::ALL {
                assert!(
                    language.strings().engine(code).is_some(),
                    "{code} in {language:?}"
                );
            }
        }
    }

    #[test]
    fn long_lists_say_how_many_there_are_and_people_what_they_are() {
        use liveside_proto::event::{PeopleList, Person};
        let cs = Language::Czech.strings();
        let en = Language::English.strings();
        assert_eq!(
            people_heading(en, PeopleList::Followers, 500, Some(1234), None),
            "Followers: 1234 (the newest 500 shown)"
        );
        assert_eq!(
            people_heading(cs, PeopleList::Followers, 0, Some(1234), None),
            "Followeři: 1234 (kdo to je, vidí jen kanál a jeho moderátoři)"
        );
        assert_eq!(
            people_heading(cs, PeopleList::Subscribers, 3, Some(3), Some(7)),
            "Předplatitelé: 3, bodů: 7"
        );
        assert_eq!(
            people_heading(en, PeopleList::Vips, 2, None, None),
            "VIPs (2)"
        );
        let person = |detail: &str, gifted_by: &str| Person {
            name: "eve".into(),
            detail: detail.into(),
            gifted_by: gifted_by.into(),
        };
        assert_eq!(
            person_detail(
                cs,
                PeopleList::Followers,
                &person("2026-10-15T18:02:11Z", "")
            ),
            Some("od 2026-10-15".to_owned())
        );
        assert_eq!(
            person_detail(cs, PeopleList::Subscribers, &person("1", "carol")),
            Some("tier 1, dárek od carol".to_owned())
        );
        assert_eq!(
            person_detail(cs, PeopleList::Moderators, &person("", "")),
            None
        );
    }

    #[test]
    fn placeholders_are_filled() {
        assert_eq!(fill("{count} active", &[("count", "12")]), "12 active");
        // A value that is not mentioned changes nothing.
        assert_eq!(fill("nothing here", &[("count", "12")]), "nothing here");
    }

    /// What is wrong with a language's file, compared with English.
    fn problems(language: Language) -> Vec<String> {
        let read = |language: Language| -> File {
            toml::from_str(language.file()).expect("the file parses")
        };
        let english = read(Language::English);
        let file = read(language);
        let names = |text: &str| {
            let mut found: Vec<String> = text
                .match_indices('{')
                .filter_map(|(i, _)| text[i..].find('}').map(|j| text[i + 1..i + j].to_owned()))
                .collect();
            found.sort();
            found
        };
        let mut said = Vec::new();
        for (table, theirs, ours) in [
            ("ui", &file.ui, &english.ui),
            ("engine", &file.engine, &english.engine),
        ] {
            for (key, text) in ours {
                match theirs.get(key) {
                    None => said.push(format!("[{table}] {key} is missing")),
                    // A translation that drops a placeholder silently loses
                    // a channel name or a count.
                    Some(their) if names(their) != names(text) => {
                        said.push(format!("[{table}] {key} has other placeholders"))
                    }
                    Some(_) => {}
                }
            }
            for key in theirs.keys().filter(|key| !ours.contains_key(*key)) {
                said.push(format!("[{table}] {key} is not English's"));
            }
        }
        said
    }

    #[test]
    fn english_has_every_line_the_program_shows() {
        let file: File = toml::from_str(Language::English.file()).unwrap();
        let mut keys: Vec<&str> = file.ui.keys().map(String::as_str).collect();
        let mut wanted = KEYS.to_vec();
        keys.sort();
        wanted.sort();
        assert_eq!(keys, wanted);
    }

    #[test]
    fn every_language_has_every_line_with_the_same_placeholders() {
        for language in Language::ALL {
            assert_eq!(problems(language), Vec::<String>::new(), "{language:?}");
            assert_eq!(Language::parse(language.code()), Some(language));
        }
    }

    #[test]
    fn a_machine_translation_says_so_where_it_is_chosen() {
        for language in Language::ALL {
            let file: File = toml::from_str(language.file()).unwrap();
            let strings = language.strings();
            assert_eq!(
                file.language.machine,
                file.language.note.is_some(),
                "{language:?}: a note exactly when a machine translated it"
            );
            if strings.machine {
                assert!(strings.listed.starts_with(strings.name));
                assert_ne!(strings.listed, strings.name);
            } else {
                assert_eq!(strings.listed, strings.name);
            }
            // The variant order is the table's.
            assert_eq!(Language::ALL[language as usize], language);
        }
    }

    #[test]
    fn arabic_is_laid_out_from_the_right() {
        for language in Language::ALL {
            assert_eq!(
                language.strings().rtl,
                language == Language::Arabic,
                "{language:?}"
            );
        }
    }

    #[test]
    fn a_missing_line_is_said_in_english() {
        let file: File = toml::from_str("[language]\nname = \"Test\"\nmachine = true\n").unwrap();
        let strings = Strings::from_file(file, Some(Language::English.strings()));
        assert_eq!(strings.connected, "connected");
        assert_eq!(strings.name, "Test");
        assert!(strings.machine);
        // The engine's own English stands in for its codes.
        assert_eq!(strings.engine("channel.unknown"), None);
    }

    #[test]
    fn reasons_are_listed_the_way_the_language_lists() {
        use liveside_proto::model::{Suspicion, SuspicionStatus};
        let suspicion = Suspicion {
            status: SuspicionStatus::Monitored,
            reasons: vec!["ban_evader".to_owned()],
            evasion: String::new(),
            shared_bans: 0,
        };
        assert_eq!(
            super::suspicion(Language::English.strings(), &suspicion),
            "under watch, likely a ban evader"
        );
        let japanese = Language::Japanese.strings();
        assert_eq!(
            super::suspicion(japanese, &suspicion),
            format!(
                "{}、{}",
                japanese.suspicion_monitored, japanese.suspicion_evader
            )
        );
    }
}
