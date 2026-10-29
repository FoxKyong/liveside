//! The settings screen: the file made visible and editable in place.
//!
//! Deliberately a list of rows with a type each (a switch, a number, a list
//! of words), rather than a hand-written screen per setting. A GUI can show
//! the same rows as checkboxes and fields, so the two frontends cannot
//! offer different settings.
//!
//! Settings that the engine reads when it starts are marked as needing a
//! restart: the running engine is not reconfigured, and saying so is better
//! than a change that appears to do nothing.

use liveside_core::config::{Config, LinkMode, NotifyWhen, Resolver, Theme, UserList};

use crate::lang::Language;

/// What a row edits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Value {
    Switch(bool),
    /// A number with its allowed range and the step Left/Right take.
    Number {
        value: usize,
        min: usize,
        max: usize,
        step: usize,
    },
    /// Words or channel names, edited as one comma-separated line.
    Words(Vec<String>),
    /// One of a few named possibilities, with what each one means.
    Choice {
        options: Vec<(&'static str, &'static str)>,
        index: usize,
    },
    /// One line of text, kept as typed.
    Text(String),
    /// Something never shown once stored, such as an API key: whether one
    /// is stored, and a new one typed to replace it. An empty field keeps
    /// what is there.
    Secret {
        stored: bool,
        typed: Option<String>,
    },
    /// Rules in the rule language, one per line: too long, and too full
    /// of commas, to share one line the way words do.
    Rules(Vec<String>),
}

/// Which frontend the rows are for. A setting that only a window can obey
/// is not offered in a terminal, where it would sit there doing nothing.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Frontend {
    #[default]
    Terminal,
    Window,
}

/// The part of the screen a row belongs to. Rows come in order, so a
/// frontend writes a heading wherever the section changes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Section {
    #[default]
    General,
    /// What protects the reader from what other people post.
    Security,
    /// Watching the stream itself, with Streamlink.
    Streamlink,
    /// Desktop notifications and sounds.
    Notifications,
    /// What lights a message up, hides it, or takes part of it out.
    Rules,
    /// Custom commands.
    Commands,
    /// The window of chat for a stream to show.
    StreamChat,
}

impl Section {
    pub fn title(self, text: &crate::lang::Strings) -> &'static str {
        match self {
            Section::General => text.section_general,
            Section::Security => text.section_security,
            Section::Streamlink => text.section_streamlink,
            Section::Notifications => text.section_notify,
            Section::Rules => text.section_rules,
            Section::Commands => text.section_commands,
            Section::StreamChat => text.section_stream_chat,
        }
    }
}

/// One setting on screen.
#[derive(Debug, Clone)]
pub struct Row {
    /// Which setting this is, for `collect`; never shown.
    pub key: &'static str,
    pub section: Section,
    pub label: &'static str,
    pub help: &'static str,
    pub value: Value,
    /// True when the engine only reads this at startup.
    pub restart: bool,
}

impl Row {
    /// What an empty field means, written in it: nothing, mostly, but an
    /// empty sound file plays the system's.
    pub fn when_empty(&self, text: &crate::lang::Strings) -> &'static str {
        match self.key {
            "notify_mention_sound"
            | "notify_whisper_sound"
            | "notify_live_sound"
            | "notify_moderation_sound" => text.notify_sound_system,
            _ => text.settings_none,
        }
    }
}

/// The command that hands the engine the highlight words and the rules of
/// a configuration.
pub fn rules_command(config: &Config) -> liveside_proto::Command {
    liveside_proto::Command::SetRules {
        highlight_words: config.highlight.words.clone(),
        highlight: config.highlight.rules.clone(),
        ignore: config.ignore.rules.clone(),
        filter: config.filter.rules.clone(),
        addressbook: config.addressbook.to_map(),
        repeated: config.repeated.clone(),
    }
}

/// Checks one line of a list as it is written: a rule, or a name and its
/// categories in the address book. What is wrong, in the reader's
/// language.
pub fn check_line(key: &str, line: &str, text: &crate::lang::Strings) -> Result<(), String> {
    match key {
        "addressbook" => liveside_core::addressbook::AddressBook::line(line)
            .map(|_| ())
            .map_err(|e| crate::lang::book_problem(text, &e)),
        "custom_commands" => liveside_core::commands::CustomCommand::parse(line)
            .map(|_| ())
            .map_err(|e| crate::lang::custom_problem(text, &e)),
        "hotkeys" => liveside_core::hotkeys::Hotkey::parse(line)
            .map(|_| ())
            .map_err(|e| crate::lang::custom_problem(text, &e)),
        _ => liveside_core::rules::Rule::parse(line)
            .map(|_| ())
            .map_err(|e| crate::lang::rule_problem(text, &e)),
    }
}

/// A list of rules being edited in a terminal.
#[derive(Debug, Clone, Default)]
pub struct RuleList {
    pub lines: Vec<String>,
    pub selected: usize,
    /// What is wrong with the rule being typed, in the reader's language.
    pub error: Option<String>,
    /// The line being typed was added just now, and goes again if left
    /// empty.
    added: bool,
}

/// The settings screen: rows, where the cursor is, and what to save into.
#[derive(Debug, Clone, Default)]
pub struct Settings {
    pub rows: Vec<Row>,
    pub selected: usize,
    pub open: bool,
    /// Set while a Words row is being typed into.
    pub editing: Option<String>,
    /// What the last save said, shown under the list.
    pub message: Option<String>,
    /// A list of rules opened for editing, in a terminal: a rule a line,
    /// each typed into on its own.
    pub rules: Option<RuleList>,
    path: std::path::PathBuf,
    /// Where a VirusTotal key typed here is written.
    key_path: std::path::PathBuf,
    config: Config,
    language: Language,
    frontend: Frontend,
}

impl Settings {
    pub fn new(
        config: Config,
        path: std::path::PathBuf,
        language: Language,
        frontend: Frontend,
    ) -> Self {
        let mut settings = Self {
            key_path: liveside_core::virustotal::default_key_path(),
            path,
            config,
            language,
            frontend,
            ..Self::default()
        };
        settings.rebuild();
        settings
    }

    /// The settings as they stand, for the caller to apply.
    pub fn config(&self) -> &Config {
        &self.config
    }

    /// Rebuilds the rows in another language.
    pub fn set_language(&mut self, language: Language) {
        self.language = language;
        self.rebuild();
    }

    /// Builds the rows from the configuration.
    fn rebuild(&mut self) {
        let text = self.language.strings();
        let config = &self.config;
        self.rows = vec![
            Row {
                key: "channels",
                section: Section::General,
                label: text.row_channels,
                help: text.row_channels_help,
                value: Value::Words(config.channels.clone()),
                restart: true,
            },
            Row {
                key: "reopen_channels",
                section: Section::General,
                label: text.row_reopen,
                help: text.row_reopen_help,
                value: Value::Switch(config.chat.reopen_channels),
                restart: true,
            },
            Row {
                key: "language",
                section: Section::General,
                label: text.row_language,
                help: text.row_language_help,
                // Each language by its own name, which is what somebody
                // looking for theirs recognizes.
                value: Value::Choice {
                    options: std::iter::once(("auto", text.language_auto))
                        .chain(
                            Language::ALL
                                .into_iter()
                                .map(|language| (language.code(), language.strings().listed)),
                        )
                        .collect(),
                    index: Language::ALL
                        .into_iter()
                        .position(|language| {
                            language.code().eq_ignore_ascii_case(&config.ui.language)
                        })
                        .map_or(0, |i| i + 1),
                },
                // The interface is redrawn from the catalogue every frame.
                restart: false,
            },
            Row {
                key: "theme",
                section: Section::General,
                label: text.row_theme,
                help: text.row_theme_help,
                value: Value::Choice {
                    options: vec![
                        ("system", text.theme_system),
                        ("light", text.theme_light),
                        ("dark", text.theme_dark),
                    ],
                    index: match config.ui.theme {
                        Theme::System => 0,
                        Theme::Light => 1,
                        Theme::Dark => 2,
                    },
                },
                // The window repaints itself when this is saved.
                restart: false,
            },
            Row {
                key: "animated_emotes",
                section: Section::General,
                label: text.row_animated,
                help: text.row_animated_help,
                value: Value::Switch(config.ui.animated_emotes),
                // The pictures on screen start or stop as it is saved.
                restart: false,
            },
            Row {
                key: "user_list",
                section: Section::General,
                label: text.row_user_list,
                help: text.row_user_list_help,
                value: Value::Choice {
                    options: vec![
                        ("active", text.row_user_list_active),
                        ("irc", text.row_user_list_irc),
                    ],
                    index: usize::from(config.chat.user_list == UserList::Irc),
                },
                restart: true,
            },
            Row {
                key: "third_party_emotes",
                section: Section::General,
                label: text.row_emotes,
                help: text.row_emotes_help,
                value: Value::Switch(config.chat.third_party_emotes),
                restart: true,
            },
            Row {
                key: "history",
                section: Section::General,
                label: text.row_log,
                help: text.row_log_help,
                value: Value::Switch(config.chat.history),
                restart: true,
            },
            Row {
                key: "history_lines",
                section: Section::General,
                label: text.row_history,
                help: text.row_history_help,
                value: Value::Number {
                    value: config.chat.history_lines,
                    min: 0,
                    max: 500,
                    step: 10,
                },
                restart: true,
            },
            Row {
                key: "scrollback",
                section: Section::General,
                label: text.row_scrollback,
                help: text.row_scrollback_help,
                value: Value::Number {
                    value: config.chat.scrollback,
                    min: 100,
                    max: 100_000,
                    step: 100,
                },
                restart: true,
            },
            // Read by the engine, which takes them from a frontend as they
            // are saved: no restart, here or in a daemon.
            Row {
                key: "highlight",
                section: Section::Rules,
                label: text.row_highlight,
                help: text.row_highlight_help,
                value: Value::Words(config.highlight.words.clone()),
                restart: false,
            },
            Row {
                key: "highlight_rules",
                section: Section::Rules,
                label: text.row_highlight_rules,
                help: text.row_highlight_rules_help,
                value: Value::Rules(config.highlight.rules.clone()),
                restart: false,
            },
            Row {
                key: "ignore_rules",
                section: Section::Rules,
                label: text.row_ignore_rules,
                help: text.row_ignore_rules_help,
                value: Value::Rules(config.ignore.rules.clone()),
                restart: false,
            },
            Row {
                key: "filter_rules",
                section: Section::Rules,
                label: text.row_filter_rules,
                help: text.row_filter_rules_help,
                value: Value::Rules(config.filter.rules.clone()),
                restart: false,
            },
            // Repeated messages: also the engine's, also without restart.
            Row {
                key: "repeated_times",
                section: Section::Rules,
                label: text.row_repeated_times,
                help: text.row_repeated_times_help,
                value: Value::Number {
                    value: config.repeated.times as usize,
                    min: 0,
                    max: 20,
                    step: 1,
                },
                restart: false,
            },
            Row {
                key: "repeated_similarity",
                section: Section::Rules,
                label: text.row_repeated_similarity,
                help: text.row_repeated_similarity_help,
                value: Value::Number {
                    value: config.repeated.similarity as usize,
                    min: 50,
                    max: 100,
                    step: 5,
                },
                restart: false,
            },
            Row {
                key: "repeated_minutes",
                section: Section::Rules,
                label: text.row_repeated_minutes,
                help: text.row_repeated_minutes_help,
                value: Value::Number {
                    value: config.repeated.minutes as usize,
                    min: 1,
                    max: 1440,
                    step: 5,
                },
                restart: false,
            },
            Row {
                key: "repeated_only",
                section: Section::Rules,
                label: text.row_repeated_only,
                help: text.row_repeated_only_help,
                value: Value::Text(config.repeated.only.clone()),
                restart: false,
            },
            // A name and its categories a line, edited like the rules
            // that refer to them.
            Row {
                key: "addressbook",
                section: Section::Rules,
                label: text.row_addressbook,
                help: text.row_addressbook_help,
                value: Value::Rules(config.addressbook.lines()),
                restart: false,
            },
            // Expanded by the frontend as a line is typed: no engine, no
            // restart.
            Row {
                key: "custom_commands",
                section: Section::Commands,
                label: text.row_commands,
                help: text.row_commands_help,
                value: Value::Rules(config.commands.custom.clone()),
                restart: false,
            },
            Row {
                key: "hotkeys",
                section: Section::Commands,
                label: text.row_hotkeys,
                help: text.row_hotkeys_help,
                value: Value::Rules(config.hotkeys.bindings.clone()),
                restart: false,
            },
            // Read when a Stream Chat window opens.
            Row {
                key: "stream_chat_seconds",
                section: Section::StreamChat,
                label: text.row_stream_chat_seconds,
                help: text.row_stream_chat_seconds_help,
                value: Value::Number {
                    value: config.stream_chat.seconds as usize,
                    min: 0,
                    max: 600,
                    step: 10,
                },
                restart: false,
            },
            Row {
                key: "stream_chat_background",
                section: Section::StreamChat,
                label: text.row_stream_chat_background,
                help: text.row_stream_chat_background_help,
                value: Value::Text(config.stream_chat.background.clone()),
                restart: false,
            },
            Row {
                key: "stream_chat_font",
                section: Section::StreamChat,
                label: text.row_stream_chat_font,
                help: text.row_stream_chat_font_help,
                value: Value::Number {
                    value: config.stream_chat.font_size as usize,
                    min: 8,
                    max: 48,
                    step: 1,
                },
                restart: false,
            },
            Row {
                key: "links",
                section: Section::Security,
                label: text.row_links,
                help: text.row_links_help,
                // A terminal cannot ask first, so there the same three say
                // what they do there.
                value: Value::Choice {
                    options: match self.frontend {
                        Frontend::Window => vec![
                            ("ask", text.links_ask),
                            ("open", text.links_open),
                            ("off", text.links_off),
                        ],
                        Frontend::Terminal => vec![
                            ("ask", text.links_terminal_ask),
                            ("open", text.links_terminal_open),
                            ("off", text.links_off),
                        ],
                    },
                    index: match config.security.links {
                        LinkMode::Ask => 0,
                        LinkMode::Open => 1,
                        LinkMode::Off => 2,
                    },
                },
                // The window reads it at every click.
                restart: false,
            },
            Row {
                key: "resolver",
                section: Section::Security,
                label: text.row_resolver,
                help: text.row_resolver_help,
                value: Value::Choice {
                    options: vec![("off", text.resolver_off), ("local", text.resolver_local)],
                    index: usize::from(config.security.resolver == Resolver::Local),
                },
                restart: true,
            },
            Row {
                key: "virustotal",
                section: Section::Security,
                label: text.row_virustotal,
                help: text.row_virustotal_help,
                value: Value::Switch(config.security.virustotal),
                restart: true,
            },
            Row {
                key: "virustotal_key",
                section: Section::Security,
                label: text.row_virustotal_key,
                help: text.row_virustotal_key_help,
                value: Value::Secret {
                    stored: liveside_core::virustotal::read_key(&self.key_path).is_some(),
                    typed: None,
                },
                // Read for every link checked.
                restart: false,
            },
            Row {
                key: "streamlink_command",
                section: Section::Streamlink,
                label: text.row_streamlink_command,
                help: text.row_streamlink_command_help,
                value: Value::Text(config.streamlink.command.clone()),
                restart: false,
            },
            Row {
                key: "streamlink_quality",
                section: Section::Streamlink,
                label: text.row_streamlink_quality,
                help: text.row_streamlink_quality_help,
                value: Value::Text(config.streamlink.quality.clone()),
                restart: false,
            },
            Row {
                key: "streamlink_arguments",
                section: Section::Streamlink,
                label: text.row_streamlink_arguments,
                help: text.row_streamlink_arguments_help,
                value: Value::Text(config.streamlink.arguments.clone()),
                restart: false,
            },
            Row {
                key: "streamlink_player",
                section: Section::Streamlink,
                label: text.row_streamlink_player,
                help: text.row_streamlink_player_help,
                value: Value::Text(config.streamlink.player.clone()),
                restart: false,
            },
            // Read by the frontend as each event arrives.
            Row {
                key: "notify_when",
                section: Section::Notifications,
                label: text.row_notify_when,
                help: text.row_notify_when_help,
                value: Value::Choice {
                    options: vec![
                        ("unseen", text.notify_when_unseen),
                        ("unfocused", text.notify_when_unfocused),
                        ("always", text.notify_when_always),
                    ],
                    index: match config.notify.when {
                        NotifyWhen::Unseen => 0,
                        NotifyWhen::Unfocused => 1,
                        NotifyWhen::Always => 2,
                    },
                },
                restart: false,
            },
            Row {
                key: "notify_desktop",
                section: Section::Notifications,
                label: text.row_notify_desktop,
                help: text.row_notify_desktop_help,
                value: Value::Switch(config.notify.desktop),
                restart: false,
            },
            Row {
                key: "notify_close_after",
                section: Section::Notifications,
                label: text.row_notify_close_after,
                help: text.row_notify_close_after_help,
                value: Value::Number {
                    value: config.notify.close_after as usize,
                    min: 0,
                    max: 120,
                    step: 5,
                },
                restart: false,
            },
            // The same switch, which a terminal obeys with its bell.
            Row {
                key: "notify_sound",
                section: Section::Notifications,
                label: match self.frontend {
                    Frontend::Window => text.row_notify_sound,
                    Frontend::Terminal => text.row_notify_bell,
                },
                help: match self.frontend {
                    Frontend::Window => text.row_notify_sound_help,
                    Frontend::Terminal => text.row_notify_bell_help,
                },
                value: Value::Switch(config.notify.sound),
                restart: false,
            },
            Row {
                key: "notify_mention",
                section: Section::Notifications,
                label: text.row_notify_mention,
                help: text.row_notify_mention_help,
                value: Value::Switch(config.notify.mention),
                restart: false,
            },
            Row {
                key: "notify_whisper",
                section: Section::Notifications,
                label: text.row_notify_whisper,
                help: text.row_notify_whisper_help,
                value: Value::Switch(config.notify.whisper),
                restart: false,
            },
            Row {
                key: "notify_live",
                section: Section::Notifications,
                label: text.row_notify_live,
                help: text.row_notify_live_help,
                value: Value::Switch(config.notify.live),
                restart: false,
            },
            Row {
                key: "notify_followed",
                section: Section::Notifications,
                label: text.row_notify_followed,
                help: text.row_notify_followed_help,
                value: Value::Switch(config.notify.followed),
                restart: false,
            },
            Row {
                key: "notify_moderation",
                section: Section::Notifications,
                label: text.row_notify_moderation,
                help: text.row_notify_moderation_help,
                value: Value::Switch(config.notify.moderation),
                restart: false,
            },
            Row {
                key: "notify_mention_sound",
                section: Section::Notifications,
                label: text.row_notify_mention_sound,
                help: text.row_notify_file_help,
                value: Value::Text(config.notify.mention_sound.clone()),
                restart: false,
            },
            Row {
                key: "notify_whisper_sound",
                section: Section::Notifications,
                label: text.row_notify_whisper_sound,
                help: text.row_notify_file_help,
                value: Value::Text(config.notify.whisper_sound.clone()),
                restart: false,
            },
            Row {
                key: "notify_live_sound",
                section: Section::Notifications,
                label: text.row_notify_live_sound,
                help: text.row_notify_file_help,
                value: Value::Text(config.notify.live_sound.clone()),
                restart: false,
            },
            Row {
                key: "notify_moderation_sound",
                section: Section::Notifications,
                label: text.row_notify_moderation_sound,
                help: text.row_notify_file_help,
                value: Value::Text(config.notify.moderation_sound.clone()),
                restart: false,
            },
        ];
        // Rows that need pictures have nothing to say in a terminal. The
        // value in the file is left as it was, so the other frontend keeps
        // whatever was set there.
        // The terminal rings its bell rather than play a file.
        if self.frontend == Frontend::Terminal {
            self.rows.retain(|row| {
                !matches!(
                    row.key,
                    "animated_emotes"
                        | "stream_chat_seconds"
                        | "stream_chat_background"
                        | "stream_chat_font"
                        | "notify_mention_sound"
                        | "notify_whisper_sound"
                        | "notify_live_sound"
                        | "notify_moderation_sound"
                )
            });
        }
    }

    /// Copies the rows back into the configuration. Matched on the key,
    /// because the label is whatever the language calls it.
    fn collect(&mut self) {
        for row in &self.rows {
            match (row.key, &row.value) {
                ("channels", Value::Words(words)) => self.config.channels = words.clone(),
                ("highlight", Value::Words(words)) => self.config.highlight.words = words.clone(),
                ("highlight_rules", Value::Rules(rules)) => {
                    self.config.highlight.rules = rules.clone()
                }
                ("ignore_rules", Value::Rules(rules)) => self.config.ignore.rules = rules.clone(),
                ("filter_rules", Value::Rules(rules)) => self.config.filter.rules = rules.clone(),
                ("stream_chat_seconds", Value::Number { value, .. }) => {
                    self.config.stream_chat.seconds = *value as u32
                }
                ("stream_chat_background", Value::Text(text)) => {
                    self.config.stream_chat.background = text.trim().to_owned()
                }
                ("stream_chat_font", Value::Number { value, .. }) => {
                    self.config.stream_chat.font_size = *value as u32
                }
                ("custom_commands", Value::Rules(lines)) => {
                    self.config.commands.custom = lines.clone()
                }
                ("hotkeys", Value::Rules(lines)) => self.config.hotkeys.bindings = lines.clone(),
                // Checked before this; a book that cannot be read is
                // refused by `save`, which reads it again.
                ("addressbook", Value::Rules(lines)) => {
                    if let Ok(book) = liveside_core::addressbook::AddressBook::from_lines(lines) {
                        self.config.addressbook = book;
                    }
                }
                ("third_party_emotes", Value::Switch(on)) => {
                    self.config.chat.third_party_emotes = *on
                }
                ("history", Value::Switch(on)) => self.config.chat.history = *on,
                ("reopen_channels", Value::Switch(on)) => self.config.chat.reopen_channels = *on,
                ("animated_emotes", Value::Switch(on)) => self.config.ui.animated_emotes = *on,
                ("history_lines", Value::Number { value, .. }) => {
                    self.config.chat.history_lines = *value
                }
                ("scrollback", Value::Number { value, .. }) => self.config.chat.scrollback = *value,
                ("repeated_times", Value::Number { value, .. }) => {
                    self.config.repeated.times = *value as u32
                }
                ("repeated_similarity", Value::Number { value, .. }) => {
                    self.config.repeated.similarity = *value as u32
                }
                ("repeated_minutes", Value::Number { value, .. }) => {
                    self.config.repeated.minutes = *value as u32
                }
                ("repeated_only", Value::Text(text)) => {
                    self.config.repeated.only = text.trim().to_owned()
                }
                ("user_list", Value::Choice { options, index }) => {
                    self.config.chat.user_list = match options[*index].0 {
                        "irc" => UserList::Irc,
                        _ => UserList::Active,
                    }
                }
                ("theme", Value::Choice { options, index }) => {
                    self.config.ui.theme = match options[*index].0 {
                        "light" => Theme::Light,
                        "dark" => Theme::Dark,
                        _ => Theme::System,
                    }
                }
                ("links", Value::Choice { options, index }) => {
                    self.config.security.links = match options[*index].0 {
                        "open" => LinkMode::Open,
                        "off" => LinkMode::Off,
                        _ => LinkMode::Ask,
                    }
                }
                ("language", Value::Choice { options, index }) => {
                    self.config.ui.language = options[*index].0.to_owned()
                }
                ("virustotal", Value::Switch(on)) => self.config.security.virustotal = *on,
                ("streamlink_command", Value::Text(text)) => {
                    self.config.streamlink.command = text.trim().to_owned()
                }
                ("streamlink_quality", Value::Text(text)) => {
                    self.config.streamlink.quality = text.trim().to_owned()
                }
                ("streamlink_player", Value::Text(text)) => {
                    self.config.streamlink.player = text.trim().to_owned()
                }
                ("streamlink_arguments", Value::Text(text)) => {
                    self.config.streamlink.arguments = text.trim().to_owned()
                }
                ("notify_when", Value::Choice { options, index }) => {
                    self.config.notify.when = match options[*index].0 {
                        "unfocused" => NotifyWhen::Unfocused,
                        "always" => NotifyWhen::Always,
                        _ => NotifyWhen::Unseen,
                    }
                }
                ("notify_desktop", Value::Switch(on)) => self.config.notify.desktop = *on,
                ("notify_close_after", Value::Number { value, .. }) => {
                    self.config.notify.close_after = *value as u32
                }
                ("notify_sound", Value::Switch(on)) => self.config.notify.sound = *on,
                ("notify_mention", Value::Switch(on)) => self.config.notify.mention = *on,
                ("notify_whisper", Value::Switch(on)) => self.config.notify.whisper = *on,
                ("notify_live", Value::Switch(on)) => self.config.notify.live = *on,
                ("notify_followed", Value::Switch(on)) => self.config.notify.followed = *on,
                ("notify_moderation", Value::Switch(on)) => self.config.notify.moderation = *on,
                ("notify_mention_sound", Value::Text(text)) => {
                    self.config.notify.mention_sound = text.trim().to_owned()
                }
                ("notify_whisper_sound", Value::Text(text)) => {
                    self.config.notify.whisper_sound = text.trim().to_owned()
                }
                ("notify_live_sound", Value::Text(text)) => {
                    self.config.notify.live_sound = text.trim().to_owned()
                }
                ("notify_moderation_sound", Value::Text(text)) => {
                    self.config.notify.moderation_sound = text.trim().to_owned()
                }
                ("resolver", Value::Choice { options, index }) => {
                    self.config.security.resolver = match options[*index].0 {
                        "local" => Resolver::Local,
                        _ => Resolver::Off,
                    }
                }
                _ => {}
            }
        }
    }

    pub fn open(&mut self) {
        self.open = true;
        self.message = None;
        self.editing = None;
        self.rules = None;
        self.reload();
    }

    /// Puts the rows back to what is saved. Closing without saving drops
    /// what was changed; shown again, it would look saved when it is not.
    pub fn reload(&mut self) {
        let selected = self.selected;
        self.rebuild();
        self.selected = selected.min(self.rows.len().saturating_sub(1));
    }

    pub fn close(&mut self) {
        self.open = false;
        self.editing = None;
        self.rules = None;
    }

    /// Moves through the rules of the list that is open.
    pub fn rules_move(&mut self, delta: isize) {
        let Some(list) = &mut self.rules else { return };
        if list.lines.is_empty() {
            return;
        }
        let last = list.lines.len() as isize - 1;
        list.selected = (list.selected as isize + delta).clamp(0, last) as usize;
    }

    /// Starts typing into the selected rule.
    pub fn rules_edit(&mut self) {
        let Some(list) = &mut self.rules else { return };
        if let Some(line) = list.lines.get(list.selected) {
            self.editing = Some(line.clone());
            list.added = false;
        }
    }

    /// Adds a rule below the selected one and starts typing into it.
    pub fn rules_add(&mut self) {
        let Some(list) = &mut self.rules else { return };
        let at = if list.lines.is_empty() {
            0
        } else {
            list.selected + 1
        };
        list.lines.insert(at, String::new());
        list.selected = at;
        list.added = true;
        list.error = None;
        self.editing = Some(String::new());
    }

    pub fn rules_delete(&mut self) {
        let Some(list) = &mut self.rules else { return };
        if list.selected < list.lines.len() {
            list.lines.remove(list.selected);
            list.selected = list.selected.min(list.lines.len().saturating_sub(1));
        }
    }

    /// Leaves the rule being typed as it was; a rule added and never
    /// written goes again.
    pub fn rules_cancel_line(&mut self) {
        self.editing = None;
        let Some(list) = &mut self.rules else { return };
        list.error = None;
        if list.added {
            list.added = false;
            self.rules_delete();
        }
    }

    /// Closes the list, putting it back into its row.
    pub fn rules_close(&mut self) {
        let Some(list) = self.rules.take() else {
            return;
        };
        if let Value::Rules(rules) = &mut self.rows[self.selected].value {
            *rules = list
                .lines
                .into_iter()
                .filter(|line| !line.trim().is_empty())
                .collect();
        }
    }

    pub fn move_by(&mut self, delta: isize) {
        if self.rows.is_empty() {
            return;
        }
        let last = self.rows.len() as isize - 1;
        let next = (self.selected as isize + delta).clamp(0, last);
        self.selected = next as usize;
    }

    /// Space and Enter flip a switch, step a choice, or start editing a
    /// list of words or a secret.
    pub fn activate(&mut self) {
        match &mut self.rows[self.selected].value {
            Value::Switch(on) => *on = !*on,
            Value::Words(words) => self.editing = Some(words.join(", ")),
            // Never the stored one: it is not shown, only replaced.
            Value::Secret { .. } => self.editing = Some(String::new()),
            Value::Text(text) => self.editing = Some(text.clone()),
            Value::Choice { options, index } => *index = (*index + 1) % options.len(),
            Value::Number { .. } => {}
            Value::Rules(rules) => {
                self.rules = Some(RuleList {
                    lines: rules.clone(),
                    ..RuleList::default()
                })
            }
        }
    }

    /// Left and Right change a number, and also flip a switch.
    pub fn adjust(&mut self, direction: isize) {
        match &mut self.rows[self.selected].value {
            Value::Switch(on) => *on = direction > 0,
            Value::Number {
                value,
                min,
                max,
                step,
            } => {
                let moved = *value as isize + direction * *step as isize;
                *value = moved.clamp(*min as isize, *max as isize) as usize;
            }
            Value::Choice { options, index } => {
                let moved = *index as isize + direction;
                *index = moved.clamp(0, options.len() as isize - 1) as usize;
            }
            Value::Words(_) | Value::Secret { .. } | Value::Text(_) | Value::Rules(_) => {}
        }
    }

    /// Finishes editing a list of words or a secret.
    pub fn finish_editing(&mut self) {
        let Some(text) = self.editing.take() else {
            return;
        };
        // A rule is checked as it is written: one that cannot be read
        // stays in the field, with what is wrong with it below.
        let key = self.rows[self.selected].key;
        if let Some(list) = &mut self.rules {
            let text = text.trim().to_owned();
            if text.is_empty() {
                list.added = true;
                self.rules_cancel_line();
                return;
            }
            match check_line(key, &text, self.language.strings()) {
                Ok(()) => {
                    if let Some(line) = list.lines.get_mut(list.selected) {
                        *line = text;
                    }
                    list.error = None;
                    list.added = false;
                }
                Err(e) => {
                    list.error = Some(e);
                    self.editing = Some(text);
                }
            }
            return;
        }
        if let Value::Secret { typed, .. } = &mut self.rows[self.selected].value {
            let text = text.trim();
            *typed = (!text.is_empty()).then(|| text.to_owned());
            return;
        }
        if let Value::Text(target) = &mut self.rows[self.selected].value {
            *target = text.trim().to_owned();
            return;
        }
        let words: Vec<String> = text
            .split(',')
            .map(|word| word.trim().trim_start_matches('#').to_owned())
            .filter(|word| !word.is_empty())
            .collect();
        if let Value::Words(target) = &mut self.rows[self.selected].value {
            *target = words;
        }
    }

    /// Writes the file, and says whether it did. The caller shows
    /// `message` either way.
    pub fn save(&mut self) -> bool {
        let before = self.config.clone();
        // An address book that cannot be read is not written either.
        if let Some(Value::Rules(lines)) = self
            .rows
            .iter()
            .find(|row| row.key == "addressbook")
            .map(|row| &row.value)
            && let Err((number, e)) = liveside_core::addressbook::AddressBook::from_lines(lines)
        {
            let text = self.language.strings();
            self.message = Some(crate::lang::fill(
                text.addressbook_line_error,
                &[
                    ("number", &number.to_string()),
                    ("error", &crate::lang::book_problem(text, &e)),
                ],
            ));
            return false;
        }
        self.collect();
        // Nor a custom command.
        if let Err(problems) =
            liveside_core::commands::CustomCommands::read(&self.config.commands.custom)
        {
            let text = self.language.strings();
            self.message = Some(
                problems
                    .iter()
                    .map(|p| {
                        crate::lang::fill(
                            text.commands_line_error,
                            &[
                                ("number", &p.number.to_string()),
                                ("error", &crate::lang::custom_problem(text, &p.problem)),
                            ],
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n"),
            );
            self.config = before;
            return false;
        }
        // Nor a colour that is none: the file would not start.
        if liveside_proto::Rgb::parse_hex(&self.config.stream_chat.background).is_none() {
            let text = self.language.strings();
            self.message = Some(crate::lang::fill(
                text.stream_chat_bad_colour,
                &[("value", &self.config.stream_chat.background)],
            ));
            self.config = before;
            return false;
        }
        // Nor a hotkey.
        if let Err(problems) = liveside_core::hotkeys::Hotkeys::read(&self.config.hotkeys.bindings)
        {
            let text = self.language.strings();
            self.message = Some(
                problems
                    .iter()
                    .map(|p| {
                        crate::lang::fill(
                            text.hotkeys_line_error,
                            &[
                                ("number", &p.number.to_string()),
                                ("error", &crate::lang::custom_problem(text, &p.problem)),
                            ],
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n"),
            );
            self.config = before;
            return false;
        }
        // Nor a restriction for repeated messages that cannot be read.
        if let Err(e) = liveside_core::repeated::Repeated::read(&self.config.repeated) {
            let text = self.language.strings();
            self.message = Some(crate::lang::fill(
                text.repeated_only_error,
                &[
                    ("rule", &self.config.repeated.only),
                    ("error", &crate::lang::rule_problem(text, &e)),
                ],
            ));
            self.config = before;
            return false;
        }
        // Rules that cannot be read are not written: the file would then
        // not start the program.
        if let Err(errors) = self.config.rules() {
            let text = self.language.strings();
            self.message = Some(
                errors
                    .iter()
                    .map(|e| crate::lang::rule_error(text, e))
                    .collect::<Vec<_>>()
                    .join("\n"),
            );
            return false;
        }
        // The secret goes to a file of its own, before the settings, so a
        // failure is the one said.
        if let Some(row) = self
            .rows
            .iter_mut()
            .find(|row| matches!(row.value, Value::Secret { typed: Some(_), .. }))
            && let Value::Secret { stored, typed } = &mut row.value
            && let Some(key) = typed.take()
        {
            if let Err(e) = liveside_core::virustotal::write_key(&self.key_path, &key) {
                let text = self.language.strings();
                self.message = Some(crate::lang::fill(
                    text.settings_save_failed,
                    &[("error", &e.to_string())],
                ));
                *typed = Some(key);
                return false;
            }
            *stored = true;
        }
        // Language and theme are picked up as the window draws itself;
        // everything else is read when the client starts. Saying "restart"
        // when nothing needs one sends people to close a window for
        // nothing.
        let needs_restart = before.channels != self.config.channels
            || before.chat != self.config.chat
            || before.security.virustotal != self.config.security.virustotal
            || before.security.resolver != self.config.security.resolver;
        // In the language just chosen, not the one in force a moment ago.
        let text = Language::parse(&self.config.ui.language)
            .unwrap_or(self.language)
            .strings();
        let said = if needs_restart {
            text.settings_saved
        } else {
            text.settings_saved_live
        };
        let saved = self.config.save(&self.path);
        self.message = Some(match &saved {
            Ok(()) => crate::lang::fill(said, &[("path", &self.path.display().to_string())]),
            Err(e) => crate::lang::fill(text.settings_save_failed, &[("error", e)]),
        });
        saved.is_ok()
    }

    /// Ignores somebody everywhere: an ignore rule for them, saved at
    /// once. Whether there was not one already.
    pub fn ignore_user(&mut self, login: &str) -> Result<bool, String> {
        let rule = format!("user:{}", login.trim_start_matches('@').to_lowercase());
        if self
            .config
            .ignore
            .rules
            .iter()
            .any(|r| r.eq_ignore_ascii_case(&rule))
        {
            return Ok(false);
        }
        self.config.ignore.rules.push(rule);
        self.config.save(&self.path)?;
        self.rebuild();
        Ok(true)
    }

    /// Takes back what `ignore_user` did. Rules that name the person
    /// among other conditions stay: they say more than "ignore them".
    pub fn unignore_user(&mut self, login: &str) -> Result<bool, String> {
        let rule = format!("user:{}", login.trim_start_matches('@').to_lowercase());
        let before = self.config.ignore.rules.len();
        self.config
            .ignore
            .rules
            .retain(|r| !r.trim().eq_ignore_ascii_case(&rule));
        if self.config.ignore.rules.len() == before {
            return Ok(false);
        }
        self.config.save(&self.path)?;
        self.rebuild();
        Ok(true)
    }

    /// The custom commands as saved, read. A list that cannot be read is
    /// not saved, so this is all of them.
    pub fn custom_commands(&self) -> liveside_core::commands::CustomCommands {
        liveside_core::commands::CustomCommands::read(&self.config.commands.custom)
            .unwrap_or_default()
    }

    /// The hotkeys as saved, read.
    pub fn hotkeys(&self) -> liveside_core::hotkeys::Hotkeys {
        liveside_core::hotkeys::Hotkeys::read(&self.config.hotkeys.bindings).unwrap_or_default()
    }

    /// The address book as saved.
    pub fn addressbook(&self) -> &liveside_core::addressbook::AddressBook {
        &self.config.addressbook
    }

    /// Changes the address book and saves it at once, as `/ab` does.
    /// Nothing is written when nothing changed.
    pub fn change_addressbook<R>(
        &mut self,
        change: impl FnOnce(&mut liveside_core::addressbook::AddressBook) -> R,
    ) -> Result<R, String> {
        let before = self.config.addressbook.clone();
        let result = change(&mut self.config.addressbook);
        if self.config.addressbook != before {
            if let Err(e) = self.config.save(&self.path) {
                self.config.addressbook = before;
                return Err(e);
            }
            self.rebuild();
        }
        Ok(result)
    }

    /// Saves a new list of bots, and gives back the command that hands it
    /// to the engine; the rest of the file is written as it stands. A list
    /// that cannot be used is not saved.
    pub fn save_bots(
        &mut self,
        bots: Vec<liveside_proto::model::BotConfig>,
    ) -> Result<liveside_proto::Command, String> {
        liveside_core::bots::check(&bots).map_err(|e| {
            let args: Vec<(String, String)> = e.args.clone();
            crate::lang::from_engine(self.language.strings(), &e.code, &args, &e.message)
        })?;
        let before = std::mem::replace(&mut self.config.bots, bots);
        if let Err(e) = self.config.save(&self.path) {
            self.config.bots = before;
            return Err(e);
        }
        Ok(liveside_proto::Command::SetBots {
            bots: self.config.bots.clone(),
        })
    }

    /// The command that hands the engine the rules as saved.
    pub fn rules_command(&self) -> liveside_proto::Command {
        rules_command(&self.config)
    }

    /// The heading to write above row `index`, if a section starts there.
    /// None when every row is in one section: a heading over the whole
    /// list says nothing.
    pub fn heading(&self, index: usize) -> Option<Section> {
        let section = self.rows.get(index)?.section;
        let several = self.rows.iter().any(|row| row.section != section);
        let starts = index == 0 || self.rows[index - 1].section != section;
        (several && starts).then_some(section)
    }

    pub fn path(&self) -> &std::path::Path {
        &self.path
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings() -> Settings {
        // A file of its own per test: they run side by side in one
        // process, and sharing one path made them delete it under each
        // other.
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let nth = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "liveside-settings-{}-{nth}.toml",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        let mut settings = Settings::new(
            Config::default(),
            path.clone(),
            Language::English,
            Frontend::Window,
        );
        // Never the reader's real key.
        settings.key_path = path.with_extension("key");
        let _ = std::fs::remove_file(&settings.key_path);
        settings.rebuild();
        settings
    }

    fn value(settings: &Settings, label: &str) -> Value {
        settings
            .rows
            .iter()
            .find(|row| row.key == label)
            .map(|row| row.value.clone())
            .expect("row exists")
    }

    fn select(settings: &mut Settings, key: &str) {
        settings.selected = settings
            .rows
            .iter()
            .position(|row| row.key == key)
            .expect("row exists");
    }

    fn type_in(settings: &mut Settings, text: &str) {
        settings.editing = Some(text.to_owned());
        settings.finish_editing();
    }

    #[test]
    fn a_list_of_rules_is_edited_a_rule_at_a_time_and_checked_as_it_is_written() {
        let mut settings = settings();
        select(&mut settings, "ignore_rules");
        settings.activate();
        assert!(settings.rules.is_some(), "the list opens");

        settings.rules_add();
        type_in(&mut settings, "user:nightbot");
        settings.rules_add();
        // Wrong: stays in the field, with why.
        type_in(&mut settings, "msgs:bots");
        let list = settings.rules.as_ref().unwrap();
        assert_eq!(settings.editing.as_deref(), Some("msgs:bots"));
        assert!(
            list.error.as_deref().unwrap().contains("Chatty"),
            "{:?}",
            list.error
        );
        type_in(&mut settings, "!status:smv startw:!bet");
        assert!(settings.editing.is_none());

        // A rule added and left empty goes again.
        settings.rules_add();
        settings.rules_cancel_line();
        assert_eq!(settings.rules.as_ref().unwrap().lines.len(), 2);

        // Changed in place, and one deleted.
        settings.rules_move(-1);
        settings.rules_edit();
        type_in(&mut settings, "user:moobot");
        settings.rules_move(1);
        settings.rules_delete();
        settings.rules_close();
        assert_eq!(
            value(&settings, "ignore_rules"),
            Value::Rules(vec!["user:moobot".to_owned()])
        );

        assert!(settings.save());
        assert_eq!(settings.config().ignore.rules, ["user:moobot"]);
        let liveside_proto::Command::SetRules { ignore, .. } = settings.rules_command() else {
            panic!("the rules, for the engine");
        };
        assert_eq!(ignore, ["user:moobot"]);
        let _ = std::fs::remove_file(settings.path());
    }

    #[test]
    fn somebody_is_ignored_and_let_back_with_one_rule_each_way() {
        let mut settings = settings();
        assert_eq!(settings.ignore_user("@NightBot"), Ok(true));
        assert_eq!(settings.ignore_user("nightbot"), Ok(false), "once");
        assert_eq!(settings.config().ignore.rules, ["user:nightbot"]);
        assert_eq!(
            value(&settings, "ignore_rules"),
            Value::Rules(vec!["user:nightbot".to_owned()]),
            "the row shows it"
        );
        let saved = Config::load(settings.path()).unwrap();
        assert_eq!(saved.ignore.rules, ["user:nightbot"]);
        assert_eq!(settings.unignore_user("NightBot"), Ok(true));
        assert_eq!(settings.unignore_user("nightbot"), Ok(false));
        assert!(
            Config::load(settings.path())
                .unwrap()
                .ignore
                .rules
                .is_empty()
        );
        let _ = std::fs::remove_file(settings.path());
    }

    #[test]
    fn repeated_messages_are_set_here_and_a_bad_restriction_is_not_saved() {
        let mut settings = settings();
        select(&mut settings, "repeated_times");
        settings.adjust(1);
        select(&mut settings, "repeated_only");
        settings.activate();
        settings.editing = Some("msgs:x".to_owned());
        settings.finish_editing();
        assert!(!settings.save());
        assert!(
            settings.message.as_deref().unwrap().contains("msgs:x"),
            "{:?}",
            settings.message
        );
        settings.activate();
        settings.editing = Some("!status:M".to_owned());
        settings.finish_editing();
        assert!(settings.save(), "{:?}", settings.message);
        let saved = Config::load(settings.path()).unwrap();
        assert_eq!(saved.repeated.times, 3);
        assert_eq!(saved.repeated.only, "!status:M");
        let liveside_proto::Command::SetRules { repeated, .. } = settings.rules_command() else {
            panic!("rules");
        };
        assert_eq!(repeated.times, 3);
        let _ = std::fs::remove_file(settings.path());
    }

    #[test]
    fn what_was_changed_and_not_saved_is_gone_when_opened_again() {
        let mut settings = settings();
        select(&mut settings, "links");
        let before = settings.rows[settings.selected].value.clone();
        settings.activate();
        assert_ne!(settings.rows[settings.selected].value, before);
        settings.close();
        settings.open();
        assert_eq!(settings.rows[settings.selected].value, before);
        assert_eq!(settings.rows[settings.selected].key, "links", "still there");
    }

    #[test]
    fn the_address_book_is_a_list_of_names_checked_as_written() {
        let mut settings = settings();
        select(&mut settings, "addressbook");
        settings.activate();
        settings.rules_add();
        type_in(&mut settings, "alice two words");
        assert_eq!(settings.editing.as_deref(), Some("alice two words"));
        assert!(
            settings
                .rules
                .as_ref()
                .unwrap()
                .error
                .as_deref()
                .unwrap()
                .contains("not a category"),
            "{:?}",
            settings.rules.as_ref().unwrap().error
        );
        type_in(&mut settings, "Alice friends,VIP");
        settings.rules_close();
        assert!(settings.save(), "{:?}", settings.message);
        let saved = Config::load(settings.path()).unwrap();
        assert_eq!(saved.addressbook.categories_of("alice"), ["friends", "vip"]);
        let liveside_proto::Command::SetRules { addressbook, .. } = settings.rules_command() else {
            panic!("rules");
        };
        assert_eq!(addressbook["alice"], ["friends", "vip"]);
        let _ = std::fs::remove_file(settings.path());
    }

    #[test]
    fn rules_that_cannot_be_read_are_not_saved() {
        let mut settings = settings();
        select(&mut settings, "filter_rules");
        settings.rows[settings.selected].value = Value::Rules(vec!["reg:(".to_owned()]);
        assert!(!settings.save());
        assert!(!settings.path().exists(), "nothing was written");
        let said = settings.message.clone().unwrap();
        assert!(said.starts_with("Filter rule 1 (reg:(): "), "{said}");
    }

    #[test]
    fn bots_are_saved_with_the_rest_and_a_bad_list_is_not() {
        let mut settings = settings();
        let bot = |name: &str| liveside_proto::model::BotConfig {
            name: name.into(),
            command: vec!["python3".into(), "bot.py".into()],
            dir: String::new(),
            channels: Vec::new(),
            autostart: false,
            ..Default::default()
        };
        let command = settings.save_bots(vec![bot("weather")]).unwrap();
        assert!(matches!(command, liveside_proto::Command::SetBots { bots } if bots.len() == 1));
        assert_eq!(
            Config::load(settings.path()).unwrap().bots,
            [bot("weather")]
        );

        let error = settings
            .save_bots(vec![bot("weather"), bot("Weather")])
            .unwrap_err();
        assert_eq!(error, "Two bots are called Weather");
        assert_eq!(settings.config().bots, [bot("weather")]);
        assert_eq!(
            Config::load(settings.path()).unwrap().bots,
            [bot("weather")]
        );
        let _ = std::fs::remove_file(settings.path());
    }

    #[test]
    fn a_terminal_is_not_offered_what_only_a_window_can_do() {
        let path = settings().path().to_owned();
        let mut config = Config::default();
        config.ui.animated_emotes = false;
        let mut terminal =
            Settings::new(config, path.clone(), Language::English, Frontend::Terminal);
        assert!(terminal.rows.iter().all(|row| row.key != "animated_emotes"));

        // Saving from there must not quietly turn the setting back on for
        // the frontend that does obey it.
        terminal.save();
        let read = Config::load(&path).expect("saved settings");
        assert!(!read.ui.animated_emotes);

        let window = Settings::new(read, path, Language::English, Frontend::Window);
        assert_eq!(
            value(&window, "animated_emotes"),
            Value::Switch(false),
            "the window offers the row, set as the file has it"
        );
    }

    #[test]
    fn a_terminal_rings_its_bell_rather_than_play_files() {
        let window = settings();
        assert!(
            window
                .rows
                .iter()
                .any(|row| row.key == "notify_whisper_sound")
        );
        let mut terminal = window.clone();
        terminal.frontend = Frontend::Terminal;
        terminal.rebuild();
        assert!(
            terminal
                .rows
                .iter()
                .all(|row| row.key == "notify_sound" || !row.key.ends_with("_sound"))
        );
        let sound = terminal
            .rows
            .iter()
            .find(|row| row.key == "notify_sound")
            .expect("the sound row");
        assert_eq!(sound.label, Language::English.strings().row_notify_bell);

        // Saved from the terminal, the files set in the window stay.
        let mut config = Config::default();
        config.notify.live_sound = "/x/live.oga".to_owned();
        let mut terminal = Settings::new(
            config,
            window.path().to_owned(),
            Language::English,
            Frontend::Terminal,
        );
        terminal.save();
        let read = Config::load(window.path()).expect("saved settings");
        assert_eq!(read.notify.live_sound, "/x/live.oga");
        std::fs::remove_file(window.path()).unwrap();
    }

    #[test]
    fn each_section_is_one_block_with_one_heading() {
        // Nothing is saved; the path is only where it would go.
        let path = std::env::temp_dir().join("liveside-test-sections.toml");
        for frontend in [Frontend::Window, Frontend::Terminal] {
            let settings =
                Settings::new(Config::default(), path.clone(), Language::English, frontend);
            let headings: Vec<Section> = (0..settings.rows.len())
                .filter_map(|i| settings.heading(i))
                .collect();
            for (i, section) in headings.iter().enumerate() {
                assert!(
                    !headings[..i].contains(section),
                    "{section:?} is headed twice in {frontend:?}: {headings:?}"
                );
            }
        }
    }

    #[test]
    fn notification_settings_save_without_a_restart() {
        let mut settings = settings();
        settings.selected = settings
            .rows
            .iter()
            .position(|row| row.key == "notify_when")
            .expect("when row");
        settings.adjust(2);
        settings.selected = settings
            .rows
            .iter()
            .position(|row| row.key == "notify_live")
            .expect("live row");
        settings.activate();
        settings.save();
        let said = settings.message.clone().expect("a message");
        assert!(!said.contains("Restart"), "{said}");
        let read = Config::load(settings.path()).expect("saved settings");
        assert_eq!(read.notify.when, NotifyWhen::Always);
        assert!(!read.notify.live);
        std::fs::remove_file(settings.path()).unwrap();
    }

    #[test]
    fn sections_get_a_heading_only_when_there_are_several() {
        let window = settings();
        let links = window
            .rows
            .iter()
            .position(|row| row.key == "links")
            .expect("links row");
        assert_eq!(window.heading(0), Some(Section::General));
        assert_eq!(window.heading(1), None);
        assert_eq!(window.heading(links), Some(Section::Security));

        // The terminal says what the links row means there.
        let mut terminal = window.clone();
        terminal.frontend = Frontend::Terminal;
        terminal.rebuild();
        let Value::Choice { options, .. } = value(&terminal, "links") else {
            panic!("a choice");
        };
        assert_eq!(options[0].1, Language::English.strings().links_terminal_ask);
    }

    #[test]
    fn a_typed_key_goes_to_its_own_file_and_is_never_shown_again() {
        let mut settings = settings();
        settings.selected = settings
            .rows
            .iter()
            .position(|row| row.key == "virustotal_key")
            .expect("key row");
        assert_eq!(
            value(&settings, "virustotal_key"),
            Value::Secret {
                stored: false,
                typed: None
            }
        );
        settings.activate();
        assert_eq!(settings.editing.as_deref(), Some(""));
        settings.editing = Some(" abc ".to_owned());
        settings.finish_editing();
        settings.save();
        assert_eq!(
            liveside_core::virustotal::read_key(&settings.key_path).as_deref(),
            Some("abc")
        );
        assert_eq!(
            value(&settings, "virustotal_key"),
            Value::Secret {
                stored: true,
                typed: None
            }
        );
        // Not in the settings file.
        let saved = std::fs::read_to_string(settings.path()).unwrap();
        assert!(!saved.contains("abc"), "{saved}");

        // Editing again and leaving it empty keeps the key.
        settings.activate();
        settings.finish_editing();
        settings.save();
        assert!(liveside_core::virustotal::read_key(&settings.key_path).is_some());
        std::fs::remove_file(&settings.key_path).unwrap();
    }

    #[test]
    fn only_a_change_that_needs_a_restart_says_so() {
        let mut settings = settings();
        // The theme is read every time the window draws itself.
        settings.selected = settings
            .rows
            .iter()
            .position(|row| row.key == "theme")
            .expect("theme row");
        settings.adjust(1);
        settings.save();
        let said = settings.message.clone().expect("a message");
        assert!(!said.contains("Restart"), "{said}");

        // The scrollback is read once, when the client starts.
        settings.selected = settings
            .rows
            .iter()
            .position(|row| row.key == "scrollback")
            .expect("scrollback row");
        settings.adjust(1);
        settings.save();
        let said = settings.message.clone().expect("a message");
        assert!(said.contains("Restart"), "{said}");
    }

    #[test]
    fn switches_and_numbers_change() {
        let mut settings = settings();
        settings.selected = settings
            .rows
            .iter()
            .position(|row| row.key == "history")
            .unwrap();
        assert_eq!(value(&settings, "history"), Value::Switch(true));
        settings.activate();
        assert_eq!(value(&settings, "history"), Value::Switch(false));

        settings.selected = settings
            .rows
            .iter()
            .position(|row| row.key == "history_lines")
            .unwrap();
        settings.adjust(-1);
        assert_eq!(
            value(&settings, "history_lines"),
            Value::Number {
                value: 40,
                min: 0,
                max: 500,
                step: 10
            }
        );
        // Numbers stop at their limits instead of wrapping.
        for _ in 0..10 {
            settings.adjust(-1);
        }
        assert!(matches!(
            value(&settings, "history_lines"),
            Value::Number { value: 0, .. }
        ));
    }

    #[test]
    fn word_lists_are_typed_as_one_line() {
        let mut settings = settings();
        settings.selected = 0; // Channels
        settings.activate();
        assert_eq!(settings.editing.as_deref(), Some(""));
        settings.editing = Some("#orbitcast, tidewave , ,".into());
        settings.finish_editing();
        // The # is optional, empty entries are dropped.
        assert_eq!(
            value(&settings, "channels"),
            Value::Words(vec!["orbitcast".into(), "tidewave".into()])
        );
    }

    #[test]
    fn saving_writes_a_file_that_reads_back() {
        let mut settings = settings();
        settings.selected = 0;
        settings.editing = Some("orbitcast".into());
        settings.finish_editing();
        settings.save();
        assert!(
            settings
                .message
                .as_deref()
                .is_some_and(|m| m.contains("Saved")),
            "{:?}",
            settings.message
        );

        let written = Config::load(settings.path()).unwrap();
        assert_eq!(written.channels, ["orbitcast"]);
        std::fs::remove_file(settings.path()).unwrap();
    }
}
