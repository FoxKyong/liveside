//! The settings file: `$XDG_CONFIG_HOME/liveside/config.toml`.
//!
//! One file for the whole program, read by the daemon and by a frontend
//! that runs its own engine. Command-line options override it, so a flag
//! stays a way to try something without editing anything.
//!
//! A missing file means defaults, and an unreadable one is reported rather
//! than silently ignored: settings that quietly do not apply are worse than
//! an error.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::engine::EngineConfig;
use crate::history::HistoryConfig;
use crate::twitch::{TwitchConfig, UserListSource};

/// `$XDG_CONFIG_HOME/liveside/config.toml`, or `~/.config/liveside/config.toml`.
pub fn default_path() -> PathBuf {
    let dir = match std::env::var_os("XDG_CONFIG_HOME") {
        Some(config) if !config.is_empty() => PathBuf::from(config),
        _ => {
            let home = std::env::var_os("HOME").unwrap_or_default();
            PathBuf::from(home).join(".config")
        }
    };
    dir.join("liveside").join("config.toml")
}

/// Everything the file can say. Every field has a default, so a file with
/// one line in it is valid.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// Channels joined at start, without the `#`.
    pub channels: Vec<String>,
    pub chat: ChatConfig,
    pub highlight: HighlightConfig,
    pub ignore: IgnoreConfig,
    pub filter: FilterConfig,
    /// People and `#channels` in categories, which rules and `/join`
    /// refer to.
    pub addressbook: crate::addressbook::AddressBook,
    /// Marking messages somebody repeats.
    pub repeated: liveside_proto::model::RepeatedConfig,
    /// Custom commands.
    pub commands: CommandsConfig,
    /// Keys that run a line.
    pub hotkeys: HotkeysConfig,
    /// The window of chat for a stream to show. Read by the window only.
    pub stream_chat: StreamChatConfig,
    pub ui: UiConfig,
    pub security: SecurityConfig,
    /// The Twitch application this client logs in as.
    pub twitch: TwitchAppConfig,
    pub streamlink: StreamlinkConfig,
    pub notify: NotifyConfig,
    /// The user's own bots, `[[bots]]`: last, since a list of tables
    /// has to come after everything else in the file.
    pub bots: Vec<liveside_proto::model::BotConfig>,
}

/// Notifications and sounds. Read by the frontends, which raise them on
/// the machine the reader sits at; see `docs/architecture.md`, section
/// *Notifications and sounds*.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct NotifyConfig {
    /// When something is worth a notification at all.
    pub when: NotifyWhen,
    /// A desktop notification, through the session's D-Bus.
    pub desktop: bool,
    /// Seconds before a desktop notification closes by itself; 0 leaves
    /// it to the desktop, and some (mako) then never close it.
    pub close_after: u32,
    /// A sound in the window, the bell in a terminal.
    pub sound: bool,
    /// A message that mentions the account or a highlight word.
    pub mention: bool,
    /// A whisper from somebody else.
    pub whisper: bool,
    /// A joined channel that starts streaming.
    pub live: bool,
    /// A followed channel that starts streaming, one not joined. Needs a
    /// login that may read the follows; it plays the going-live sound.
    pub followed: bool,
    /// A message held for moderators, or an unban request.
    pub moderation: bool,
    /// A sound file for each, played instead of the one from the system's
    /// sound theme. Empty means the system's.
    pub mention_sound: String,
    pub whisper_sound: String,
    pub live_sound: String,
    pub moderation_sound: String,
}

impl Default for NotifyConfig {
    fn default() -> Self {
        Self {
            when: NotifyWhen::default(),
            desktop: true,
            close_after: 10,
            sound: true,
            mention: true,
            whisper: true,
            live: true,
            followed: true,
            moderation: true,
            mention_sound: String::new(),
            whisper_sound: String::new(),
            live_sound: String::new(),
            moderation_sound: String::new(),
        }
    }
}

impl NotifyConfig {
    /// One line for `--check-config`: what notifies, how, and when.
    fn summary(&self) -> String {
        let kinds: Vec<&str> = [
            (self.mention, "mentions"),
            (self.whisper, "whispers"),
            (self.live, "going live"),
            (self.followed, "followed going live"),
            (self.moderation, "moderation"),
        ]
        .into_iter()
        .filter_map(|(on, name)| on.then_some(name))
        .collect();
        let desktop = match self.close_after {
            0 => "desktop".to_owned(),
            seconds => format!("desktop for {seconds} s"),
        };
        let how: Vec<&str> = [(self.desktop, desktop.as_str()), (self.sound, "sound")]
            .into_iter()
            .filter_map(|(on, name)| on.then_some(name))
            .collect();
        if kinds.is_empty() || how.is_empty() {
            return "off".to_owned();
        }
        let when = match self.when {
            NotifyWhen::Unseen => "when not on screen",
            NotifyWhen::Unfocused => "when the window is not focused",
            NotifyWhen::Always => "always",
        };
        format!("{} ({}), {when}", kinds.join(", "), how.join(" + "))
    }
}

/// When to notify about something.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NotifyWhen {
    /// When the reader cannot see it: the window is not focused, or
    /// another channel is on screen.
    #[default]
    Unseen,
    /// Only while the window is not focused.
    Unfocused,
    /// Whatever is on screen.
    Always,
}

/// Watching a stream with Streamlink, started from the client. Read by the
/// frontends, which start it on the machine the reader sits at.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct StreamlinkConfig {
    /// The program to run.
    pub command: String,
    /// Which stream to ask for: `best`, `720p60`, `audio_only`, …
    pub quality: String,
    /// The player to hand the stream to; empty leaves the choice to
    /// Streamlink and its own configuration.
    pub player: String,
    /// Anything else to give Streamlink, as on a command line, quotes
    /// included: `--twitch-low-latency --player-args "--vo=wlshm"`.
    pub arguments: String,
}

impl Default for StreamlinkConfig {
    fn default() -> Self {
        Self {
            command: "streamlink".to_owned(),
            quality: "best".to_owned(),
            player: String::new(),
            arguments: String::new(),
        }
    }
}

/// The Twitch application this client logs in as. Twitch wants every
/// application to have a Client ID of its own, so a build that is not
/// Liveside's — a fork, or one changed and passed on — registers one at
/// dev.twitch.tv and names it here rather than using Liveside's.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct TwitchAppConfig {
    /// Empty: the one built in.
    pub client_id: String,
}

/// What protects the reader from what other people post. See
/// `docs/architecture.md`, section *Links*.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SecurityConfig {
    /// What clicking a link in chat does.
    pub links: LinkMode,
    /// Whether links can be checked with VirusTotal, with the reader's own
    /// API key from `virustotal.key` beside this file.
    pub virustotal: bool,
    /// How to find out where a link leads without opening it.
    pub resolver: Resolver,
}

/// Who follows a link's redirects to say where it leads.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Resolver {
    /// Nobody: a link is not followed.
    #[default]
    Off,
    /// This machine, which tells every host on the way the reader's
    /// address — but never renders a page, and needs no service.
    Local,
}

/// What clicking a link does. Read by the frontends; the engine finds the
/// links either way.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LinkMode {
    /// Show the whole address, the host picked out, and open it only when
    /// that is confirmed. Chat is where phishing arrives, so a link is not
    /// one careless click from the browser.
    #[default]
    Ask,
    /// Open it in the browser at once.
    Open,
    /// Not a link at all: text to read and copy.
    Off,
}

/// Settings that belong to a frontend, not to the engine. The engine
/// carries them so that one file holds everything.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct UiConfig {
    /// `auto` to follow the system, or a language code such as `en`, `cs`.
    pub language: String,
    /// Light or dark. The window draws itself that way; the terminal
    /// client takes its colours from the terminal, and this tells it
    /// which background they are on, so names stay readable. Left to the
    /// system, the window follows the desktop and the terminal client
    /// asks the terminal.
    pub theme: Theme,
    /// Whether emotes that move are played. Only the graphical client
    /// draws pictures at all; a moving chat is not to everyone's taste,
    /// and standing still costs a laptop less.
    pub animated_emotes: bool,
}

/// Which way round the graphical client's colours go.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Theme {
    /// Whatever the desktop asks for, which is what most programs do.
    #[default]
    System,
    Light,
    Dark,
}

impl Default for UiConfig {
    fn default() -> Self {
        Self {
            language: "auto".to_owned(),
            theme: Theme::default(),
            animated_emotes: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ChatConfig {
    /// Lines kept per channel in memory.
    pub scrollback: usize,
    /// Keep a local chat log, and restore from it when joining.
    pub history: bool,
    /// Lines restored from the log on join.
    pub history_lines: usize,
    /// Open again at start the channels that were open at the end, besides
    /// `channels`. Which they were is kept in the state directory
    /// (`crate::open_channels`).
    pub reopen_channels: bool,
    /// Ask 7TV, BetterTTV and FrankerFaceZ about emotes. See
    /// `docs/privacy.md`: it tells them which channels are read.
    pub third_party_emotes: bool,
    /// Where the user list comes from.
    pub user_list: UserList,
}

/// How to learn who is in a channel.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UserList {
    /// Whoever has talked. Costs nothing and works everywhere.
    #[default]
    Active,
    /// Also watch Twitch IRC, which reports people joining and leaving.
    /// Opens a second connection to Twitch; see `docs/privacy.md`.
    Irc,
}

impl Default for ChatConfig {
    fn default() -> Self {
        let engine = EngineConfig::default();
        Self {
            scrollback: engine.scrollback,
            history: true,
            history_lines: HistoryConfig::default().lines,
            reopen_channels: true,
            third_party_emotes: TwitchConfig::default().third_party_emotes,
            user_list: UserList::Active,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct HighlightConfig {
    /// Words that light a message up, besides the account's own name,
    /// which always does. Matched case-insensitively, as whole words.
    pub words: Vec<String>,
    /// Rules in the rule language (`crate::rules`), one per item, tried
    /// in order; the first that matches decides the colours.
    pub rules: Vec<String>,
}

/// Custom commands, in Chatty's form: `/name what to do`, one an item.
/// Read by the frontends, which expand them as a line is typed.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct CommandsConfig {
    pub custom: Vec<String>,
}

/// The Stream Chat window: a channel's messages alone, for a streaming
/// program to capture and show.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct StreamChatConfig {
    /// How long a message stays, in seconds; 0 keeps them.
    pub seconds: u32,
    /// `#rrggbb`: one colour for the whole window, so a streaming program
    /// can key it out.
    pub background: String,
    /// The size of the text, in points.
    pub font_size: u32,
}

impl Default for StreamChatConfig {
    fn default() -> Self {
        Self {
            seconds: 60,
            background: "#000000".to_owned(),
            font_size: 16,
        }
    }
}

/// Hotkeys: `KEY what to do`, one an item. Read by the frontends.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct HotkeysConfig {
    pub bindings: Vec<String>,
}

/// Messages not to show: hidden, and counted in the channel now and
/// then so it is clear something was.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct IgnoreConfig {
    pub rules: Vec<String>,
}

/// Parts of messages not to show: what the first matching rule finds is
/// replaced by `..` or the rule's `replacement:`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct FilterConfig {
    pub rules: Vec<String>,
}

/// What went wrong with the settings file, in a form worth printing: the
/// parser's own message (which names the line and points at the offending
/// text), plus what it means for the program.
#[derive(Debug, Clone)]
pub struct ConfigError {
    pub path: PathBuf,
    /// The parser's message, or the I/O error.
    pub detail: String,
    /// A misspelled key and the name it is probably meant to be.
    pub suggestion: Option<(String, String)>,
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(
            f,
            "{} is not valid, so none of its settings were applied.\n",
            self.path.display()
        )?;
        writeln!(f, "{}", self.detail.trim_end())?;
        if let Some((wrong, meant)) = &self.suggestion {
            writeln!(f, "\nDid you mean `{meant}` instead of `{wrong}`?")?;
        }
        write!(
            f,
            "\nFix the file, point --config somewhere else, or remove it to \
             start with the defaults."
        )
    }
}

impl std::error::Error for ConfigError {}

impl Config {
    /// Reads the file. A missing file gives defaults; a broken one is an
    /// error, because settings that do not apply are hard to notice.
    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(e) => {
                return Err(ConfigError {
                    path: path.to_path_buf(),
                    detail: format!("cannot read it: {e}"),
                    suggestion: None,
                });
            }
        };
        let mut config: Config = toml::from_str(&text).map_err(|e| {
            let detail = e.to_string();
            ConfigError {
                path: path.to_path_buf(),
                suggestion: suggest(&detail),
                detail,
            }
        })?;
        // A name in the book that cannot be one would never match either.
        config.addressbook = std::mem::take(&mut config.addressbook)
            .checked()
            .map_err(|e| ConfigError {
                path: path.to_path_buf(),
                detail: format!("addressbook: {e}"),
                suggestion: None,
            })?;
        if let Err(problems) = crate::commands::CustomCommands::read(&config.commands.custom) {
            return Err(ConfigError {
                path: path.to_path_buf(),
                detail: problems
                    .iter()
                    .map(|p| p.to_string())
                    .collect::<Vec<_>>()
                    .join("\n"),
                suggestion: None,
            });
        }
        if liveside_proto::Rgb::parse_hex(&config.stream_chat.background).is_none() {
            return Err(ConfigError {
                path: path.to_path_buf(),
                detail: format!(
                    "stream_chat.background: “{}” is not a colour; write #rrggbb",
                    config.stream_chat.background
                ),
                suggestion: None,
            });
        }
        if let Err(problems) = crate::hotkeys::Hotkeys::read(&config.hotkeys.bindings) {
            return Err(ConfigError {
                path: path.to_path_buf(),
                detail: problems
                    .iter()
                    .map(|p| p.to_string())
                    .collect::<Vec<_>>()
                    .join("\n"),
                suggestion: None,
            });
        }
        if let Err(e) = crate::repeated::Repeated::read(&config.repeated) {
            return Err(ConfigError {
                path: path.to_path_buf(),
                detail: format!("repeated.only ({}): {e}", config.repeated.only),
                suggestion: None,
            });
        }
        // Twitch's are thirty letters and digits. Anything else would only
        // show up as a login Twitch refuses with a message about clients.
        let client_id = config.twitch.client_id.trim();
        if !client_id.chars().all(|c| c.is_ascii_alphanumeric()) {
            return Err(ConfigError {
                path: path.to_path_buf(),
                detail: format!(
                    "twitch.client_id: “{client_id}” is not a Client ID; it is letters and digits"
                ),
                suggestion: None,
            });
        }
        if let Err(e) = crate::bots::check(&config.bots) {
            return Err(ConfigError {
                path: path.to_path_buf(),
                detail: format!("bots: {}", e.message),
                suggestion: None,
            });
        }
        // A rule that cannot be read is as wrong as a key that cannot: it
        // would otherwise just never match, and nobody would know why.
        if let Err(errors) = config.rules() {
            let detail = errors
                .iter()
                .map(|e| e.to_string())
                .collect::<Vec<_>>()
                .join("\n");
            return Err(ConfigError {
                path: path.to_path_buf(),
                detail,
                suggestion: None,
            });
        }
        Ok(config)
    }

    /// The highlight, ignore and filter rules, read.
    pub fn rules(&self) -> Result<crate::rules::Rules, Vec<crate::rules::ListError>> {
        crate::rules::Rules::read(
            &self.highlight.rules,
            &self.ignore.rules,
            &self.filter.rules,
        )
    }

    /// The settings as they will be used, for `--check-config`. Mentions
    /// where things are kept, since that is what people want to know.
    pub fn summary(&self) -> String {
        let channels = if self.channels.is_empty() {
            "(none; join with /join)".to_owned()
        } else {
            self.channels.join(", ")
        };
        let words = if self.highlight.words.is_empty() {
            "(only your own name)".to_owned()
        } else {
            self.highlight.words.join(", ")
        };
        let reopen = if self.chat.reopen_channels {
            format!(
                "on, kept in {}",
                crate::open_channels::default_path(false).display()
            )
        } else {
            "off".to_owned()
        };
        let log = if self.chat.history {
            format!(
                "on, in {} ({} lines restored on join)",
                crate::history::default_dir().display(),
                self.chat.history_lines
            )
        } else {
            "off".to_owned()
        };
        let count = |rules: &[String]| match rules.len() {
            0 => "none".to_owned(),
            n => n.to_string(),
        };
        format!(
            "Channels on start:  {channels}\n\
             Reopen last ones:   {reopen}\n\
             Highlight words:    {words}\n\
             Highlight rules:    {}\n\
             Ignore rules:       {} (hidden, counted every 30 s)\n\
             Filter rules:       {}\n\
             Address book:       {}\n\
             Repeated messages:  {}\n\
             Custom commands:    {}\n\
             Hotkeys:            {}\n\
             Interface language: {}\n\
             User list:          {}\n\
             Third-party emotes: {}\n\
             Local chat log:     {log}\n\
             Scrollback:         {} lines per channel\n\
             Links:              {}\n\
             Where links lead:   {}\n\
             VirusTotal:         {}\n\
             Notifications:      {}\n\
             Bots:               {}\n\
             Twitch Client ID:   {}",
            count(&self.highlight.rules),
            count(&self.ignore.rules),
            count(&self.filter.rules),
            match self.addressbook.len() {
                0 => "empty".to_owned(),
                n => format!(
                    "{n} names in {}",
                    match self.addressbook.all_categories() {
                        c if c.is_empty() => "no categories".to_owned(),
                        c => c.into_iter().collect::<Vec<_>>().join(", "),
                    }
                ),
            },
            match &self.repeated {
                r if r.times == 0 => "not marked".to_owned(),
                r => format!(
                    "marked once said {} times, {} % alike, within {} min{}",
                    r.times,
                    r.similarity,
                    r.minutes,
                    match r.only.trim() {
                        "" => String::new(),
                        only => format!(", only {only}"),
                    }
                ),
            },
            match crate::commands::CustomCommands::read(&self.commands.custom) {
                Ok(list) if list.is_empty() => "none".to_owned(),
                Ok(list) => list
                    .names()
                    .iter()
                    .map(|n| format!("/{n}"))
                    .collect::<Vec<_>>()
                    .join(" "),
                Err(_) => "(cannot be read)".to_owned(),
            },
            match crate::hotkeys::Hotkeys::read(&self.hotkeys.bindings) {
                Ok(keys) if keys.is_empty() => "none".to_owned(),
                Ok(keys) => keys.keys().join(" "),
                Err(_) => "(cannot be read)".to_owned(),
            },
            self.ui.language,
            match self.chat.user_list {
                UserList::Active => "who has talked (no extra connection)",
                UserList::Irc => "Twitch IRC membership (a second connection)",
            },
            if self.chat.third_party_emotes {
                "on (7TV, BetterTTV, FrankerFaceZ)"
            } else {
                "off"
            },
            self.chat.scrollback,
            match self.security.links {
                LinkMode::Ask => "open after confirming the address",
                LinkMode::Open => "open at once",
                LinkMode::Off => "plain text",
            },
            match self.security.resolver {
                Resolver::Off => "not followed",
                Resolver::Local => "followed from this machine (the sites see your IP)",
            },
            if self.security.virustotal {
                "on, with the key in virustotal.key"
            } else {
                "off"
            },
            self.notify.summary(),
            match self.bots.len() {
                0 => "none".to_owned(),
                _ => self
                    .bots
                    .iter()
                    .map(|bot| match bot.autostart {
                        true => format!("{} (starts with Liveside)", bot.name),
                        false => bot.name.clone(),
                    })
                    .collect::<Vec<_>>()
                    .join(", "),
            },
            match self.twitch.client_id.trim() {
                "" => "Liveside's own".to_owned(),
                own => format!("{own} (your own; a login made with another is asked for again)"),
            },
        )
    }

    /// Writes the file, creating the directory. Settings are not secret,
    /// but they say which channels are read, so the directory is the same
    /// private one as the token's.
    pub fn save(&self, path: &Path) -> Result<(), String> {
        let text = toml::to_string_pretty(self).map_err(|e| e.to_string())?;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)
                .map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
            }
        }
        // Written beside the target and renamed, so an interrupted save
        // cannot leave a half-written settings file behind.
        let temporary = path.with_extension("toml.new");
        std::fs::write(&temporary, text)
            .map_err(|e| format!("cannot write {}: {e}", temporary.display()))?;
        std::fs::rename(&temporary, path)
            .map_err(|e| format!("cannot replace {}: {e}", path.display()))
    }

    /// The engine settings this file asks for.
    pub fn engine(&self) -> EngineConfig {
        EngineConfig {
            scrollback: self.chat.scrollback.max(1),
            history: if self.chat.history {
                HistoryConfig {
                    lines: self.chat.history_lines,
                    ..HistoryConfig::default()
                }
            } else {
                HistoryConfig::disabled()
            },
            highlight_words: self.highlight.words.clone(),
            addressbook: self.addressbook.clone(),
            repeated: crate::repeated::Repeated::read(&self.repeated).unwrap_or_default(),
            // Checked when the file was read; one made in code that is
            // wrong loses only its broken rules.
            rules: self.rules().unwrap_or_else(|_| {
                crate::rules::Rules::read_valid(
                    &self.highlight.rules,
                    &self.ignore.rules,
                    &self.filter.rules,
                )
            }),
            virustotal: self
                .security
                .virustotal
                .then(crate::virustotal::default_key_path),
            follow_links: self.security.resolver == Resolver::Local,
            bots: self.bots.clone(),
            open_channels: self
                .chat
                .reopen_channels
                .then(|| crate::open_channels::default_path(false)),
            channels: self.channels.clone(),
            ..EngineConfig::default()
        }
    }

    /// The Twitch settings this file asks for.
    pub fn twitch(&self) -> TwitchConfig {
        let client_id = match self.twitch.client_id.trim() {
            "" => TwitchConfig::default().client_id,
            own => own.to_owned(),
        };
        TwitchConfig {
            client_id,
            third_party_emotes: self.chat.third_party_emotes,
            user_list: match self.chat.user_list {
                UserList::Active => UserListSource::Active,
                UserList::Irc => UserListSource::Irc,
            },
            ..TwitchConfig::default()
        }
    }
}

/// Pulls `unknown field \`x\`, expected one of \`a\`, \`b\`` out of the
/// parser's message and picks the closest name: a typo in a key is the
/// most common mistake, and the parser does not guess.
fn suggest(message: &str) -> Option<(String, String)> {
    let rest = message.split("unknown field `").nth(1)?;
    let (wrong, rest) = rest.split_once('`')?;
    let known: Vec<&str> = rest
        .split('`')
        .skip(1)
        .step_by(2)
        .filter(|name| !name.is_empty())
        .collect();
    let best = known
        .into_iter()
        .map(|name| (distance(wrong, name), name))
        .min_by_key(|(distance, _)| *distance)?;
    // Only when it really is close; otherwise the list in the message is
    // more useful than a wrong guess.
    (best.0 <= wrong.len().div_ceil(2)).then(|| (wrong.to_owned(), best.1.to_owned()))
}

/// Levenshtein distance, enough for "did you mean".
fn distance(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, a) in a.chars().enumerate() {
        let mut previous = row[0];
        row[0] = i + 1;
        for (j, b) in b.iter().enumerate() {
            let cost = usize::from(a != *b);
            let next = (row[j] + 1).min(row[j + 1] + 1).min(previous + cost);
            previous = row[j + 1];
            row[j + 1] = next;
        }
    }
    row[b.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "liveside-config-{name}-{}.toml",
            std::process::id()
        ))
    }

    #[test]
    fn a_missing_file_means_defaults() {
        let path = temp_path("missing");
        let _ = std::fs::remove_file(&path);
        assert_eq!(Config::load(&path).unwrap(), Config::default());
    }

    #[test]
    fn reads_a_partial_file() {
        let path = temp_path("partial");
        std::fs::write(
            &path,
            "channels = [\"orbitcast\"]\n[chat]\nthird_party_emotes = false\n",
        )
        .unwrap();
        let config = Config::load(&path).unwrap();
        assert_eq!(config.channels, ["orbitcast"]);
        assert!(!config.chat.third_party_emotes);
        // Everything not mentioned keeps its default.
        assert_eq!(config.chat.scrollback, ChatConfig::default().scrollback);
        assert!(config.chat.history);
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn a_broken_file_says_where_and_what() {
        let path = temp_path("broken");
        std::fs::write(&path, "channels = [\"a\"\n\n[chat]\nhistory = true\n").unwrap();
        let report = Config::load(&path).unwrap_err().to_string();
        assert!(report.contains(&path.display().to_string()), "{report}");
        assert!(report.contains("line 3"), "{report}");
        assert!(
            report.contains("none of its settings were applied"),
            "{report}"
        );
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn a_misspelled_key_gets_a_suggestion() {
        let path = temp_path("typo");
        std::fs::write(&path, "[chat]\nhistroy = true\n").unwrap();
        let report = Config::load(&path).unwrap_err().to_string();
        assert!(report.contains("Did you mean `history`"), "{report}");
        // The parser's own pointer at the line survives.
        assert!(report.contains("histroy = true"), "{report}");

        // A wrong value says what was expected.
        std::fs::write(&path, "[chat]\nscrollback = \"many\"\n").unwrap();
        let report = Config::load(&path).unwrap_err().to_string();
        assert!(report.contains("expected usize"), "{report}");
        assert!(!report.contains("Did you mean"), "no guess to make");
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn a_client_id_of_ones_own_replaces_the_built_in_one() {
        let path = temp_path("client-id");
        assert_eq!(
            Config::default().twitch().client_id,
            liveside_twitch::CLIENT_ID
        );
        std::fs::write(&path, "[twitch]\nclient_id = \" abc123 \"\n").unwrap();
        assert_eq!(Config::load(&path).unwrap().twitch().client_id, "abc123");
        std::fs::write(&path, "[twitch]\nclient_id = \"abc 123\"\n").unwrap();
        let report = Config::load(&path).unwrap_err().to_string();
        assert!(report.contains("twitch.client_id"), "{report}");
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn a_name_that_resembles_nothing_gets_no_guess() {
        // The parser lists the valid keys; a wrong guess would be worse.
        assert_eq!(
            suggest("unknown field `qqqqqqqq`, expected one of `scrollback`, `history`"),
            None
        );
        assert_eq!(
            suggest("unknown field `scrolback`, expected one of `scrollback`, `history`"),
            Some(("scrolback".to_owned(), "scrollback".to_owned()))
        );
    }

    #[test]
    fn the_summary_says_what_will_be_used() {
        let config = Config {
            channels: vec!["orbitcast".into()],
            chat: ChatConfig {
                history: false,
                third_party_emotes: false,
                ..ChatConfig::default()
            },
            ..Config::default()
        };
        let summary = config.summary();
        assert!(summary.contains("orbitcast"), "{summary}");
        assert!(summary.contains("Local chat log:     off"), "{summary}");
        assert!(summary.contains("only your own name"), "{summary}");
    }

    #[test]
    fn a_rule_that_cannot_be_read_stops_the_file_with_where_and_why() {
        let path = temp_path("bad-rule");
        std::fs::write(
            &path,
            "[highlight]\nrules = [\"w:ok\", \"msgs:friends\"]\n[filter]\nrules = [\"reg:(\"]\n",
        )
        .unwrap();
        let error = Config::load(&path).unwrap_err().to_string();
        std::fs::remove_file(&path).unwrap();
        assert!(error.contains("highlight rule 2 (msgs:friends)"), "{error}");
        assert!(error.contains("filter rule 1"), "{error}");
        assert!(
            error.contains("none of its settings were applied"),
            "{error}"
        );
    }

    #[test]
    fn the_address_book_is_read_into_its_form_and_a_bad_name_stops_the_file() {
        let path = temp_path("addressbook");
        std::fs::write(
            &path,
            "[addressbook]\nAlice = [\"VIP\", \"friends\"]\n\"#Harborlight\" = [\"czech\"]\n",
        )
        .unwrap();
        let config = Config::load(&path).unwrap();
        assert_eq!(
            config.addressbook.categories_of("alice"),
            ["friends", "vip"]
        );
        assert_eq!(config.addressbook.categories_of("#harborlight"), ["czech"]);
        assert!(config.summary().contains("2 names in czech, friends, vip"));
        std::fs::write(&path, "[addressbook]\n\"two words\" = []\n").unwrap();
        let error = Config::load(&path).unwrap_err().to_string();
        std::fs::remove_file(&path).unwrap();
        assert!(error.contains("addressbook: “two words”"), "{error}");
    }

    #[test]
    fn a_repetition_rule_that_cannot_be_read_stops_the_file() {
        let path = temp_path("repeated");
        std::fs::write(&path, "[repeated]\ntimes = 3\nonly = \"msgs:x\"\n").unwrap();
        let error = Config::load(&path).unwrap_err().to_string();
        std::fs::write(&path, "[repeated]\ntimes = 0\n").unwrap();
        let off = Config::load(&path).unwrap();
        std::fs::remove_file(&path).unwrap();
        assert!(error.contains("repeated.only (msgs:x)"), "{error}");
        assert!(off.summary().contains("Repeated messages:  not marked"));
        assert!(
            Config::default()
                .summary()
                .contains("marked once said 2 times, 90 % alike, within 60 min")
        );
    }

    #[test]
    fn a_custom_command_that_cannot_be_read_stops_the_file() {
        let path = temp_path("commands");
        std::fs::write(&path, "[commands]\ncustom = [\"/ok fine\", \"/bad $(1\"]\n").unwrap();
        let error = Config::load(&path).unwrap_err().to_string();
        std::fs::write(&path, "[commands]\ncustom = [\"/b x\", \"/a y\"]\n").unwrap();
        let fine = Config::load(&path).unwrap();
        std::fs::remove_file(&path).unwrap();
        assert!(error.contains("custom command 2 (/bad $(1)"), "{error}");
        assert!(fine.summary().contains("Custom commands:    /a /b"));
    }

    #[test]
    fn a_hotkey_that_cannot_be_read_stops_the_file() {
        let path = temp_path("hotkeys");
        std::fs::write(&path, "[hotkeys]\nbindings = [\"F5 /info\", \"B x\"]\n").unwrap();
        let error = Config::load(&path).unwrap_err().to_string();
        std::fs::write(
            &path,
            "[hotkeys]\nbindings = [\"F5 /info\", \"Ctrl+B /ban $$1\"]\n",
        )
        .unwrap();
        let fine = Config::load(&path).unwrap();
        std::fs::remove_file(&path).unwrap();
        assert!(error.contains("hotkey 2 (B x)"), "{error}");
        assert!(fine.summary().contains("Hotkeys:            F5 Ctrl+B"));
    }

    #[test]
    fn a_stream_chat_background_that_is_no_colour_stops_the_file() {
        let path = temp_path("stream-chat");
        std::fs::write(&path, "[stream_chat]\nbackground = \"green\"\n").unwrap();
        let error = Config::load(&path).unwrap_err().to_string();
        std::fs::remove_file(&path).unwrap();
        assert!(error.contains("stream_chat.background: “green”"), "{error}");
    }

    #[test]
    fn saves_and_reads_back() {
        let path = temp_path("roundtrip");
        let config = Config {
            channels: vec!["orbitcast".into(), "tidewave".into()],
            chat: ChatConfig {
                history: false,
                scrollback: 500,
                ..ChatConfig::default()
            },
            highlight: HighlightConfig {
                words: vec!["rust".into()],
                rules: vec!["chan:harborlight w:\"two words\"".into()],
            },
            ignore: IgnoreConfig {
                rules: vec!["user:nightbot".into()],
            },
            filter: FilterConfig {
                rules: vec!["replacement:[link] reg:https?://\\S+".into()],
            },
            addressbook: crate::addressbook::AddressBook::from_lines(&[
                "alice friends,vip".into(),
                "#harborlight czech".into(),
            ])
            .unwrap(),
            commands: CommandsConfig {
                custom: vec!["/slap /me slaps $$1 with a trout".into()],
            },
            hotkeys: HotkeysConfig {
                bindings: vec!["F5 /info".into()],
            },
            stream_chat: StreamChatConfig {
                seconds: 0,
                background: "#00ff00".into(),
                font_size: 20,
            },
            repeated: liveside_proto::model::RepeatedConfig {
                times: 3,
                similarity: 80,
                minutes: 10,
                min_length: 5,
                only: "!status:M".into(),
            },
            ui: UiConfig::default(),
            security: SecurityConfig {
                links: LinkMode::Off,
                virustotal: true,
                resolver: Resolver::Local,
            },
            streamlink: StreamlinkConfig {
                command: "/usr/bin/streamlink".to_owned(),
                quality: "720p60".to_owned(),
                player: "mpv".to_owned(),
                arguments: "--twitch-low-latency".to_owned(),
            },
            notify: NotifyConfig {
                when: NotifyWhen::Always,
                sound: false,
                live: false,
                whisper_sound: "/tmp/ping.oga".to_owned(),
                ..NotifyConfig::default()
            },
            twitch: TwitchAppConfig {
                client_id: "abc123".into(),
            },
            bots: vec![
                liveside_proto::model::BotConfig {
                    name: "weather".into(),
                    command: vec!["python3".into(), "/home/me/weather.py".into()],
                    dir: "/home/me".into(),
                    channels: vec!["brightmaple".into()],
                    autostart: true,
                    ..Default::default()
                },
                liveside_proto::model::BotConfig {
                    name: "quiet".into(),
                    command: vec!["./quiet".into()],
                    dir: String::new(),
                    channels: Vec::new(),
                    autostart: false,
                    ..Default::default()
                },
            ],
        };
        config.save(&path).unwrap();
        assert_eq!(Config::load(&path).unwrap(), config);
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn bots_with_one_name_or_no_command_stop_the_file() {
        let path = temp_path("bots");
        std::fs::write(
            &path,
            "[[bots]]\nname = \"a\"\ncommand = [\"x\"]\n\n[[bots]]\nname = \"A\"\ncommand = [\"y\"]\n",
        )
        .unwrap();
        let error = Config::load(&path).unwrap_err().to_string();
        assert!(error.contains("Two bots are called A"), "{error}");
        std::fs::write(&path, "[[bots]]\nname = \"a\"\ncommand = []\n").unwrap();
        let error = Config::load(&path).unwrap_err().to_string();
        assert!(error.contains("Bot a has no command"), "{error}");
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn turns_into_engine_and_twitch_settings() {
        let config = Config {
            chat: ChatConfig {
                history: false,
                third_party_emotes: false,
                ..ChatConfig::default()
            },
            highlight: HighlightConfig {
                words: vec!["rust".into()],
                rules: vec!["w:a".into(), "w:b".into()],
            },
            ignore: IgnoreConfig {
                rules: vec!["user:x".into()],
            },
            ..Config::default()
        };
        let engine = config.engine();
        assert_eq!(engine.rules.highlight.len(), 2);
        assert_eq!(engine.rules.ignore.len(), 1);
        assert!(engine.history.dir.is_none(), "history off means no log");
        assert_eq!(engine.highlight_words, ["rust"]);
        assert!(!config.twitch().third_party_emotes);

        // On by default, with the log in the usual place.
        let engine = Config::default().engine();
        assert!(engine.history.dir.is_some());
    }
}
