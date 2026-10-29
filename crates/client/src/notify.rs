//! Notifications and sounds: what is worth one, decided once for both
//! frontends.
//!
//! The frontend raises them, not the engine: a daemon may run where there
//! is nobody to hear a sound, and two frontends attached to one engine may
//! sit at two desks. Each frontend feeds its events through a [`Notifier`],
//! says what is on its screen, and gets back the [`Alert`]s to raise; how
//! (a desktop notification, a sound, the terminal's bell) is up to it.
//! See `docs/architecture.md`, section *Notifications and sounds*.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use liveside_core::config::{NotifyConfig, NotifyWhen};
use liveside_proto::model::LineContent;
use liveside_proto::{Event, Login, Timestamp, UserId};

use crate::lang::{Strings, fill};

/// What happened, as far as notifying goes. Each has its own switch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    Mention,
    Whisper,
    Live,
    /// A followed channel, not joined, going live.
    Followed,
    Moderation,
}

impl Kind {
    /// What the settings call it, for saying which one has no sound.
    pub fn label(self, text: &Strings) -> &'static str {
        match self {
            Kind::Mention => text.row_notify_mention,
            Kind::Whisper => text.row_notify_whisper,
            Kind::Live => text.row_notify_live,
            Kind::Followed => text.row_notify_followed,
            Kind::Moderation => text.row_notify_moderation,
        }
    }

    /// The name in the freedesktop sound theme.
    fn theme_sound(self) -> &'static str {
        match self {
            Kind::Mention => "message-new-instant",
            Kind::Whisper => "message",
            Kind::Live | Kind::Followed => "service-login",
            Kind::Moderation => "dialog-warning",
        }
    }

    /// The name of the system's sound for it on a Mac, which has no
    /// freedesktop theme but sounds of its own in [`MAC_SOUNDS`].
    fn mac_sound(self) -> &'static str {
        match self {
            Kind::Mention => "Glass",
            Kind::Whisper => "Ping",
            Kind::Live | Kind::Followed => "Hero",
            Kind::Moderation => "Basso",
        }
    }
}

/// Where it happened: what has to be on screen for the reader to have
/// seen it already, and where clicking the notification goes.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Place {
    Channel(UserId),
    Whisper(Login),
    /// A followed channel that is not joined, by the login a join takes;
    /// `None` for several at once, which the list of followed channels
    /// shows. Never on screen, so never seen already.
    Followed(Option<Login>),
}

/// Something to tell the reader about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Alert {
    pub kind: Kind,
    pub place: Place,
    /// One line: who, and where.
    pub title: String,
    /// What they said, as plain text; may be empty.
    pub body: String,
    /// No sound for it: its highlight rule says `config:silent`.
    pub silent: bool,
    /// No desktop notification: its rule says `config:!notify`.
    pub quiet: bool,
}

/// What the reader has in front of them right now.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Seen {
    /// Whether the window (or the terminal) has the keyboard focus.
    pub focused: bool,
    /// The channel or whisper thread on screen, if any.
    pub place: Option<Place>,
    /// Others on screen beside it, such as channels docked in the window.
    pub beside: Vec<Place>,
}

/// A sound of one kind plays at most this often, however many messages
/// arrive: a raid that keeps naming the reader is one ping, not forty.
const SOUND_GAP: Duration = Duration::from_secs(3);

/// The longest body a notification gets; a wall of text belongs in the
/// chat, not on the desktop.
const BODY_CHARS: usize = 200;

/// Decides, from the events a frontend gets anyway, what is worth telling
/// the reader about.
#[derive(Debug)]
pub struct Notifier {
    config: NotifyConfig,
    /// When this frontend started: anything older that arrives now (the
    /// unban requests a join lists) is not news.
    started: Timestamp,
    /// Whether each channel streams, as last heard. A channel not in here
    /// has not been heard of yet, and its first report is not a change.
    live: HashMap<UserId, bool>,
    /// Display names of the joined channels, for the titles.
    names: HashMap<UserId, String>,
    last_sound: HashMap<Kind, Instant>,
}

impl Notifier {
    pub fn new(config: NotifyConfig) -> Self {
        Self {
            config,
            started: Timestamp::now(),
            live: HashMap::new(),
            names: HashMap::new(),
            last_sound: HashMap::new(),
        }
    }

    /// Takes the settings as just saved.
    pub fn set_config(&mut self, config: NotifyConfig) {
        self.config = config;
    }

    pub fn config(&self) -> &NotifyConfig {
        &self.config
    }

    /// A channel known from a snapshot rather than from events: its name,
    /// and whether it streams, so that is not taken for a change later.
    pub fn know_channel(&mut self, channel: &liveside_proto::model::Channel, live: bool) {
        self.names
            .insert(channel.id.clone(), channel.display_name.clone());
        self.live.insert(channel.id.clone(), live);
    }

    /// Looks at an event and says whether it is worth telling the reader
    /// about, by its kind and the settings. Whether the reader can see it
    /// already is [`Notifier::wanted`]'s business.
    pub fn consider(&mut self, event: &Event, text: &Strings) -> Option<Alert> {
        let alert = match event {
            Event::ChannelJoined { channel, .. } => {
                self.names
                    .insert(channel.id.clone(), channel.display_name.clone());
                None
            }
            Event::ChannelParted { channel } => {
                self.live.remove(channel);
                None
            }
            Event::Stream { channel, live } => {
                let before = self.live.insert(channel.clone(), *live);
                // Off to on, and only that: a join or a reconnection says
                // how things stand, which is not news.
                (before == Some(false) && *live).then(|| Alert {
                    kind: Kind::Live,
                    silent: false,
                    quiet: false,
                    place: Place::Channel(channel.clone()),
                    title: fill(text.notify_live, &[("channel", &self.name(channel))]),
                    body: text.notify_live_body.to_owned(),
                })
            }
            Event::Line { channel, line } => {
                if !line.flags.highlighted || line.flags.own || line.flags.historical {
                    return None;
                }
                let LineContent::Message(message) = &line.content else {
                    // An info line a rule lit up: a subscription, an
                    // announcement, something said about the channel.
                    let body = match &line.content {
                        LineContent::UserNotice {
                            system_text,
                            message,
                        } => match message {
                            Some(message) => {
                                format!("{system_text} {}", message.plain_text())
                            }
                            None => system_text.clone(),
                        },
                        LineContent::Notice {
                            text: said,
                            code,
                            args,
                            ..
                        } => crate::lang::from_engine(text, code, args, said),
                        LineContent::Message(_) => unreachable!(),
                    };
                    return self.allowed(Alert {
                        kind: Kind::Mention,
                        silent: line.flags.highlight_silent,
                        quiet: line.flags.highlight_quiet,
                        place: Place::Channel(channel.clone()),
                        title: fill(text.notify_highlight, &[("channel", &self.name(channel))]),
                        body,
                    });
                };
                Some(Alert {
                    kind: Kind::Mention,
                    silent: line.flags.highlight_silent,
                    quiet: line.flags.highlight_quiet,
                    place: Place::Channel(channel.clone()),
                    title: fill(
                        text.notify_mention,
                        &[
                            ("user", &message.sender.display_name),
                            ("channel", &self.name(channel)),
                        ],
                    ),
                    body: message.plain_text(),
                })
            }
            Event::Whisper { user, line } => {
                let LineContent::Message(message) = &line.content else {
                    return None;
                };
                if line.flags.own || line.flags.historical {
                    return None;
                }
                Some(Alert {
                    kind: Kind::Whisper,
                    silent: false,
                    quiet: false,
                    place: Place::Whisper(user.login.clone()),
                    title: fill(text.notify_whisper, &[("user", &user.display_name)]),
                    body: message.plain_text(),
                })
            }
            Event::MessageHeld { channel, held } if held.time >= self.started => Some(Alert {
                kind: Kind::Moderation,
                silent: false,
                quiet: false,
                place: Place::Channel(channel.clone()),
                title: fill(text.notify_held, &[("channel", &self.name(channel))]),
                body: format!("{}: {}", held.sender.display_name, held.text),
            }),
            Event::UnbanRequested { channel, request } if request.time >= self.started => {
                Some(Alert {
                    kind: Kind::Moderation,
                    silent: false,
                    quiet: false,
                    place: Place::Channel(channel.clone()),
                    title: fill(text.notify_unban, &[("channel", &self.name(channel))]),
                    body: if request.text.is_empty() {
                        request.user.display_name.clone()
                    } else {
                        format!("{}: {}", request.user.display_name, request.text)
                    },
                })
            }
            Event::FollowedStreams { live, started } => {
                let started: Vec<_> = live
                    .iter()
                    .filter(|f| started.contains(&f.channel.id))
                    .collect();
                // Several at once (back from a night without network)
                // are one notification, not a stack of them.
                match started.as_slice() {
                    [] => None,
                    [one] => Some(Alert {
                        kind: Kind::Followed,
                        silent: false,
                        quiet: false,
                        place: Place::Followed(Some(one.channel.login.clone())),
                        title: fill(
                            text.notify_followed,
                            &[("channel", &one.channel.display_name)],
                        ),
                        body: one
                            .live
                            .as_ref()
                            .map(|stream| crate::followed::about(stream, text))
                            .unwrap_or_default(),
                    }),
                    [_, ..] => Some(Alert {
                        kind: Kind::Followed,
                        silent: false,
                        quiet: false,
                        place: Place::Followed(None),
                        title: fill(
                            text.notify_followed_many,
                            &[("count", &started.len().to_string())],
                        ),
                        body: started
                            .iter()
                            .map(|f| f.channel.display_name.as_str())
                            .collect::<Vec<_>>()
                            .join(", "),
                    }),
                }
            }
            _ => None,
        }?;
        self.allowed(alert)
    }

    /// The alert, if its kind is switched on and there is a way to say
    /// it, with its body made short.
    fn allowed(&self, alert: Alert) -> Option<Alert> {
        let on = match alert.kind {
            Kind::Mention => self.config.mention,
            Kind::Whisper => self.config.whisper,
            Kind::Live => self.config.live,
            Kind::Followed => self.config.followed,
            Kind::Moderation => self.config.moderation,
        };
        (on && (self.config.desktop || self.config.sound)).then(|| Alert {
            body: shortened(&alert.body),
            ..alert
        })
    }

    /// Whether to raise it, given what the reader can see.
    pub fn wanted(&self, alert: &Alert, seen: &Seen) -> bool {
        match self.config.when {
            NotifyWhen::Always => true,
            NotifyWhen::Unfocused => !seen.focused,
            NotifyWhen::Unseen => {
                !seen.focused
                    || (seen.place.as_ref() != Some(&alert.place)
                        && !seen.beside.contains(&alert.place))
            }
        }
    }

    /// Whether a sound of this kind is to play now: sounds are on, and
    /// the last one of the kind was a while ago. Counts it as played.
    pub fn sound_now(&mut self, kind: Kind) -> bool {
        if !self.config.sound {
            return false;
        }
        let now = Instant::now();
        if self
            .last_sound
            .get(&kind)
            .is_some_and(|last| now.duration_since(*last) < SOUND_GAP)
        {
            return false;
        }
        self.last_sound.insert(kind, now);
        true
    }

    /// The file to play for a kind: the one set in the settings, or the
    /// system's from its sound theme, or a Mac's own. `None` when there is
    /// none of them.
    pub fn sound_file(&self, kind: Kind) -> Option<PathBuf> {
        let set = match kind {
            Kind::Mention => &self.config.mention_sound,
            Kind::Whisper => &self.config.whisper_sound,
            // One sound for going live, whoever it is.
            Kind::Live | Kind::Followed => &self.config.live_sound,
            Kind::Moderation => &self.config.moderation_sound,
        };
        if !set.trim().is_empty() {
            return Some(PathBuf::from(set.trim()));
        }
        theme_sound(kind.theme_sound(), &data_dirs())
            .or_else(|| system_sound(kind.mac_sound(), Path::new(MAC_SOUNDS)))
    }

    fn name(&self, channel: &UserId) -> String {
        self.names
            .get(channel)
            .cloned()
            .unwrap_or_else(|| channel.to_string())
    }
}

/// At most [`BODY_CHARS`] characters, on one line.
fn shortened(body: &str) -> String {
    let flat: String = body
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let flat = flat.trim();
    match flat.char_indices().nth(BODY_CHARS) {
        Some((cut, _)) => format!("{}…", &flat[..cut]),
        None => flat.to_owned(),
    }
}

/// Where the freedesktop specifications look for shared data, most
/// personal first.
fn data_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    match std::env::var_os("XDG_DATA_HOME") {
        Some(home) if !home.is_empty() => dirs.push(PathBuf::from(home)),
        _ => {
            if let Some(home) = std::env::var_os("HOME") {
                dirs.push(PathBuf::from(home).join(".local/share"));
            }
        }
    }
    let system = std::env::var("XDG_DATA_DIRS")
        .ok()
        .filter(|dirs| !dirs.is_empty())
        .unwrap_or_else(|| "/usr/local/share:/usr/share".to_owned());
    dirs.extend(
        system
            .split(':')
            .filter(|dir| !dir.is_empty())
            .map(PathBuf::from),
    );
    dirs
}

/// A sound from the freedesktop sound theme: `sounds/freedesktop/stereo/
/// <name>.<ext>` under one of `dirs`. A name that is not there falls back
/// to its shorter forms, as the specification says: `message-new-instant`,
/// then `message-new`, then `message`.
fn theme_sound(name: &str, dirs: &[PathBuf]) -> Option<PathBuf> {
    let mut name = name;
    loop {
        for dir in dirs {
            let stereo = dir.join("sounds").join("freedesktop").join("stereo");
            for extension in ["oga", "ogg", "wav"] {
                let file = stereo.join(format!("{name}.{extension}"));
                if file.is_file() {
                    return Some(file);
                }
            }
        }
        name = &name[..name.rfind('-')?];
    }
}

/// Where a Mac keeps its alert sounds.
const MAC_SOUNDS: &str = "/System/Library/Sounds";

/// A sound of the system's own, `<name>.aiff` in `dir`, if it is there.
fn system_sound(name: &str, dir: &Path) -> Option<PathBuf> {
    let file = dir.join(format!("{name}.aiff"));
    file.is_file().then_some(file)
}

/// Programs that play a sound file, tried in this order: PipeWire's, then
/// PulseAudio's, one of which is on practically every desktop with sound,
/// then the one every Mac has.
const PLAYERS: &[&str] = &["pw-play", "paplay", "afplay"];

/// Why a sound did not play.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlayError {
    /// None of the players is installed.
    NoPlayer,
    /// The player ran and failed; what it said last.
    Failed(String),
}

/// Plays a sound file in a process of its own, as Streamlink is run: a
/// player that fails, or crashes, cannot take the chat with it. (GTK's own
/// media backend did exactly that, in a machine without working OpenGL.)
/// `failed` is called from another thread when it does not play.
pub fn play(file: &Path, failed: impl FnOnce(PlayError) + Send + 'static) {
    play_with(PLAYERS, file, failed);
}

fn play_with(
    players: &'static [&'static str],
    file: &Path,
    failed: impl FnOnce(PlayError) + Send + 'static,
) {
    let file = file.to_owned();
    // Inside a Flatpak a file of the user's is outside the sandbox, but the
    // sound theme bundled with it is only inside.
    let bundled = liveside_core::host::in_flatpak() && file.starts_with("/app");
    // Starting a process and waiting for it to end is no work for the
    // thread that draws the window.
    std::thread::spawn(move || {
        // A player that is installed may still fail where the next one
        // plays: pw-play without PipeWire running, or inside a Flatpak,
        // which lets sound out only through PulseAudio's socket. What the
        // first one said is kept for when none plays.
        let mut failure = None;
        for player in players {
            let mut command = if bundled {
                std::process::Command::new(player)
            } else {
                liveside_core::host::command(*player, None)
            };
            let started = command
                .arg(&file)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::piped())
                .spawn();
            let child = match started {
                Ok(child) => child,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                Err(e) => {
                    failure.get_or_insert(e.to_string());
                    continue;
                }
            };
            match child.wait_with_output() {
                Ok(output) if output.status.success() => return,
                Ok(output) => {
                    let said = String::from_utf8_lossy(&output.stderr);
                    let said = said
                        .lines()
                        .rev()
                        .find(|line| !line.trim().is_empty())
                        .map(|line| line.trim().to_owned())
                        .unwrap_or_else(|| output.status.to_string());
                    failure.get_or_insert(said);
                }
                Err(e) => {
                    failure.get_or_insert(e.to_string());
                }
            }
        }
        failed(failure.map_or(PlayError::NoPlayer, PlayError::Failed));
    });
}

/// Text for a notification's body, which notification services read as a
/// little HTML: chat text with a `<` or `&` in it must stay text.
pub fn escape_body(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// The freedesktop notification service, where every desktop listens.
const SERVICE: &str = "org.freedesktop.Notifications";
const SERVICE_PATH: &str = "/org/freedesktop/Notifications";

/// The icon the packages install under the app's ID. Plasma finds a
/// Flatpak's icon by itself; GNOME shows a blank symbol without it.
const APP_ICON: &str = "net.jifox.liveside.Liveside";

/// The notifications raised here that are still on the desktop, as the
/// service tells about them: where each leads, and the token a click
/// brings to let the window come to the front.
#[derive(Debug, Default)]
struct Raised {
    places: HashMap<u32, Place>,
    tokens: HashMap<u32, String>,
}

impl Raised {
    fn shown(&mut self, id: u32, place: Place) {
        self.places.insert(id, place);
    }

    /// The service sends the token just before the click it goes with.
    /// Every program on the bus may hear another's, so only ours count.
    fn token(&mut self, id: u32, token: String) {
        if self.places.contains_key(&id) {
            self.tokens.insert(id, token);
        }
    }

    /// Where a click leads, with its token, if it was ours and on the
    /// notification itself ("default") rather than on a button.
    fn invoked(&mut self, id: u32, action: &str) -> Option<(Place, Option<String>)> {
        let token = self.tokens.remove(&id);
        if action != "default" {
            return None;
        }
        self.places.get(&id).cloned().map(|place| (place, token))
    }

    fn closed(&mut self, id: u32) {
        self.places.remove(&id);
        self.tokens.remove(&id);
    }
}

/// Opens the one connection every notification goes over, and starts
/// hearing on it what becomes of them. One connection, because GNOME lets
/// only the connection that raised a notification replace it.
fn connect(
    raised: Arc<Mutex<Raised>>,
    clicked: Clicked,
) -> zbus::Result<zbus::blocking::Connection> {
    let connection = zbus::blocking::Connection::session()?;
    let rule = zbus::MatchRule::builder()
        .msg_type(zbus::message::Type::Signal)
        .interface(SERVICE)?
        .path(SERVICE_PATH)?
        .build();
    // Asked for before the first notification, so no answer comes before
    // anybody listens.
    let signals = zbus::blocking::MessageIterator::for_match_rule(rule, &connection, None)?;
    std::thread::spawn(move || {
        for message in signals {
            let Ok(message) = message else {
                continue;
            };
            let header = message.header();
            let Some(member) = header.member() else {
                continue;
            };
            let body = message.body();
            let click = {
                let Ok(mut raised) = raised.lock() else {
                    return;
                };
                match member.as_str() {
                    "ActivationToken" => {
                        if let Ok((id, token)) = body.deserialize::<(u32, String)>() {
                            raised.token(id, token);
                        }
                        None
                    }
                    "ActionInvoked" => body
                        .deserialize::<(u32, String)>()
                        .ok()
                        .and_then(|(id, action)| raised.invoked(id, &action)),
                    "NotificationClosed" => {
                        if let Ok((id, _reason)) = body.deserialize::<(u32, u32)>() {
                            raised.closed(id);
                        }
                        None
                    }
                    _ => None,
                }
            };
            if let Some((place, token)) = click {
                clicked(place, token);
            }
        }
    });
    Ok(connection)
}

/// The milliseconds a notification stays, as the service takes them:
/// -1 leaves it to the desktop.
fn expire_timeout(close_after: u32) -> i32 {
    if close_after == 0 {
        return -1;
    }
    i32::try_from(close_after.saturating_mul(1000)).unwrap_or(i32::MAX)
}

/// Raises one, in place of `replaces` unless that is 0, and gives the
/// number the service gave it.
fn notify(
    connection: &zbus::blocking::Connection,
    alert: &Alert,
    replaces: u32,
    close_after: u32,
    open: &str,
) -> zbus::Result<u32> {
    // The sound is Liveside's to play or not, by its own settings and
    // rules; Cinnamon would otherwise play its own beside it, or where a
    // rule says `config:silent`.
    let hints: HashMap<&str, zbus::zvariant::Value> =
        HashMap::from([("suppress-sound", true.into())]);
    let reply = connection.call_method(
        Some(SERVICE),
        SERVICE_PATH,
        Some(SERVICE),
        "Notify",
        &(
            "Liveside",
            replaces,
            APP_ICON,
            alert.title.as_str(),
            escape_body(&alert.body),
            // "default" is what clicking the notification itself does,
            // rather than a button beside it.
            vec!["default", open],
            hints,
            expire_timeout(close_after),
        ),
    )?;
    reply.body().deserialize::<u32>()
}

/// How long a notification counts as on screen when the desktop decides
/// how long it stays: as long as Liveside's own default.
const ON_SCREEN_DEFAULT: u32 = 10;

/// What became of the notification last raised for a kind and place.
#[derive(Debug, PartialEq, Eq)]
enum Earlier {
    None,
    /// Still on screen: the new one replaces it.
    OnScreen(u32),
    /// Gone from the screen: the new one is raised anew, for a desktop
    /// that keeps the old one in its list (Plasma) updates it there
    /// without showing anything, so the reader would never see it.
    Gone(u32),
}

fn earlier(last: Option<(u32, Instant)>, now: Instant, close_after: u32) -> Earlier {
    let Some((id, raised)) = last else {
        return Earlier::None;
    };
    let stays = if close_after == 0 {
        ON_SCREEN_DEFAULT
    } else {
        close_after
    };
    if now.duration_since(raised) < Duration::from_secs(stays.into()) {
        Earlier::OnScreen(id)
    } else {
        Earlier::Gone(id)
    }
}

/// Takes one off the desktop, and out of its list.
fn close(connection: &zbus::blocking::Connection, id: u32) -> zbus::Result<()> {
    connection.call_method(
        Some(SERVICE),
        SERVICE_PATH,
        Some(SERVICE),
        "CloseNotification",
        &(id,),
    )?;
    Ok(())
}

/// Raises desktop notifications on a thread of its own: the D-Bus calls
/// wait for an answer, and neither a window nor a terminal may stall on
/// one.
pub struct Desktop {
    queue: std::sync::mpsc::Sender<(Alert, u32)>,
}

/// Where a clicked notification leads, said from another thread, with
/// the activation token the desktop gave the click, if it gave one.
pub type Clicked = Arc<dyn Fn(Place, Option<String>) + Send + Sync>;

impl Desktop {
    /// Starts the thread. `failed` is called, from that thread, the first
    /// time a notification cannot be shown (no session bus over SSH, no
    /// notification service), and never again after that. `clicked` is
    /// called, from yet another thread, with where a notification the
    /// reader clicked happened; `open` is what the click is called, for a
    /// notification service that writes it on a button.
    pub fn start(
        failed: impl FnOnce(String) + Send + 'static,
        clicked: Clicked,
        open: &str,
    ) -> Self {
        let (queue, alerts) = std::sync::mpsc::channel::<(Alert, u32)>();
        let open = open.to_owned();
        std::thread::spawn(move || {
            let mut failed = Some(failed);
            let mut say = |error: zbus::Error| {
                if let Some(failed) = failed.take() {
                    failed(error.to_string());
                }
            };
            let raised = Arc::new(Mutex::new(Raised::default()));
            let mut connection = None;
            // The notification each kind and place last raised, and when,
            // which the next one replaces rather than stacking up beside
            // it while it is on screen.
            let mut shown: HashMap<(Kind, Place), (u32, Instant)> = HashMap::new();
            for (alert, close_after) in alerts {
                if connection.is_none() {
                    match connect(raised.clone(), clicked.clone()) {
                        Ok(opened) => connection = Some(opened),
                        Err(e) => {
                            say(e);
                            continue;
                        }
                    }
                }
                let Some(connection) = &connection else {
                    continue;
                };
                let key = (alert.kind, alert.place.clone());
                let replaces = match earlier(shown.get(&key).copied(), Instant::now(), close_after)
                {
                    Earlier::None => 0,
                    Earlier::OnScreen(id) => id,
                    Earlier::Gone(id) => {
                        // Out of its list too, so that list does not fill
                        // up with one channel going live again and again.
                        let _ = close(connection, id);
                        0
                    }
                };
                let mut result = notify(connection, &alert, replaces, close_after, &open);
                if result.is_err() && replaces != 0 {
                    // Should the service refuse to replace it, it is
                    // raised as new rather than lost.
                    result = notify(connection, &alert, 0, close_after, &open);
                }
                match result {
                    Ok(id) => {
                        shown.insert(key, (id, Instant::now()));
                        if let Ok(mut raised) = raised.lock() {
                            raised.shown(id, alert.place);
                        }
                    }
                    Err(e) => say(e),
                }
            }
        });
        Self { queue }
    }

    /// Raises it, to close by itself after `close_after` seconds, or when
    /// the desktop decides if that is 0.
    pub fn show(&self, alert: Alert, close_after: u32) {
        let _ = self.queue.send((alert, close_after));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use liveside_proto::model::{
        Channel, ChatMessage, Fragment, Line, LineFlags, MessageKind, User,
    };

    fn text() -> &'static Strings {
        crate::lang::Language::English.strings()
    }

    fn user(login: &str) -> User {
        User {
            id: format!("id-{login}").into(),
            login: login.into(),
            display_name: login.to_uppercase(),
        }
    }

    fn message_line(from: &str, said: &str, flags: LineFlags) -> Line {
        Line {
            time: Timestamp::now(),
            content: LineContent::Message(ChatMessage {
                id: "m1".into(),
                sender: user(from),
                color: None,
                badges: Vec::new(),
                fragments: vec![Fragment::Text {
                    text: said.to_owned(),
                }],
                kind: MessageKind::Text,
                bits: None,
                reply: None,
                shared_source: None,
            }),
            flags,
        }
    }

    fn highlighted() -> LineFlags {
        LineFlags {
            highlighted: true,
            ..LineFlags::default()
        }
    }

    fn notifier() -> Notifier {
        let mut notifier = Notifier::new(NotifyConfig::default());
        notifier.consider(
            &Event::ChannelJoined {
                channel: Channel {
                    id: "c1".into(),
                    login: "harborlight".into(),
                    display_name: "Harborlight".into(),
                },
                modes: Default::default(),
            },
            text(),
        );
        notifier
    }

    fn line(flags: LineFlags) -> Event {
        Event::Line {
            channel: "c1".into(),
            line: message_line("someone", "hey <b>brightmaple</b> & co", flags),
        }
    }

    #[test]
    fn a_mention_is_worth_one_and_says_who_and_where() {
        let alert = notifier().consider(&line(highlighted()), text()).unwrap();
        assert_eq!(alert.kind, Kind::Mention);
        assert_eq!(alert.place, Place::Channel("c1".into()));
        assert_eq!(alert.title, "SOMEONE in #Harborlight");
        assert_eq!(alert.body, "hey <b>brightmaple</b> & co");
        assert_eq!(
            escape_body(&alert.body),
            "hey &lt;b&gt;brightmaple&lt;/b&gt; &amp; co"
        );
    }

    #[test]
    fn ordinary_own_and_restored_messages_are_not() {
        let mut notifier = notifier();
        assert_eq!(notifier.consider(&line(LineFlags::default()), text()), None);
        let own = LineFlags {
            own: true,
            ..highlighted()
        };
        assert_eq!(notifier.consider(&line(own), text()), None);
        let restored = LineFlags {
            historical: true,
            ..highlighted()
        };
        assert_eq!(notifier.consider(&line(restored), text()), None);
    }

    #[test]
    fn a_kind_switched_off_is_not_and_neither_is_anything_with_no_way_to_say_it() {
        let mut notifier = notifier();
        notifier.set_config(NotifyConfig {
            mention: false,
            ..NotifyConfig::default()
        });
        assert_eq!(notifier.consider(&line(highlighted()), text()), None);
        notifier.set_config(NotifyConfig {
            desktop: false,
            sound: false,
            ..NotifyConfig::default()
        });
        assert_eq!(notifier.consider(&line(highlighted()), text()), None);
    }

    #[test]
    fn whispers_from_others_only() {
        let mut notifier = notifier();
        let whisper = |flags| Event::Whisper {
            user: user("friend"),
            line: message_line("friend", "psst", flags),
        };
        let alert = notifier
            .consider(&whisper(LineFlags::default()), text())
            .unwrap();
        assert_eq!(alert.place, Place::Whisper("friend".into()));
        assert_eq!(alert.title, "Whisper from FRIEND");
        let own = LineFlags {
            own: true,
            ..LineFlags::default()
        };
        assert_eq!(notifier.consider(&whisper(own), text()), None);
    }

    #[test]
    fn going_live_is_a_change_not_a_state() {
        let mut notifier = notifier();
        let stream = |live| Event::Stream {
            channel: "c1".into(),
            live,
        };
        // Joining in the middle of a stream: how things are, not news.
        assert_eq!(notifier.consider(&stream(true), text()), None);
        // A reconnection says the same again.
        assert_eq!(notifier.consider(&stream(true), text()), None);
        assert_eq!(notifier.consider(&stream(false), text()), None);
        let alert = notifier.consider(&stream(true), text()).unwrap();
        assert_eq!(alert.kind, Kind::Live);
        assert_eq!(alert.title, "#Harborlight is live");

        // A channel known from a snapshot: its state is not a change.
        let mut fresh = Notifier::new(NotifyConfig::default());
        fresh.know_channel(
            &Channel {
                id: "c2".into(),
                login: "x".into(),
                display_name: "X".into(),
            },
            false,
        );
        let alert = fresh.consider(
            &Event::Stream {
                channel: "c2".into(),
                live: true,
            },
            text(),
        );
        assert_eq!(alert.map(|a| a.title), Some("#X is live".to_owned()));
    }

    #[test]
    fn a_followed_channel_starting_says_what_it_streams_and_several_are_one() {
        use liveside_proto::model::{FollowedChannel, LiveStream, StreamStats};
        let followed = |id: &str, name: &str| FollowedChannel {
            channel: Channel {
                id: id.into(),
                login: name.to_lowercase().into(),
                display_name: name.into(),
            },
            live: Some(LiveStream {
                title: "Morning coffee".into(),
                category: "Just Chatting".into(),
                stats: StreamStats {
                    viewers: 12,
                    started: Timestamp::now(),
                },
            }),
        };
        let event = |started: &[&str]| Event::FollowedStreams {
            live: vec![followed("f1", "Quiet"), followed("f2", "Loud")],
            started: started.iter().map(|id| (*id).into()).collect(),
        };
        let mut notifier = notifier();
        // The first look, and a look where nothing began.
        assert_eq!(notifier.consider(&event(&[]), text()), None);

        let alert = notifier.consider(&event(&["f2"]), text()).unwrap();
        assert_eq!(alert.kind, Kind::Followed);
        assert_eq!(alert.place, Place::Followed(Some("loud".into())));
        assert_eq!(alert.title, "Loud is live");
        assert_eq!(alert.body, "Just Chatting · Morning coffee");

        let alert = notifier.consider(&event(&["f1", "f2"]), text()).unwrap();
        assert_eq!(alert.title, "2 channels you follow are live");
        assert_eq!(alert.place, Place::Followed(None));
        assert_eq!(alert.body, "Quiet, Loud");

        // A switch of its own, apart from the joined channels'.
        notifier.set_config(NotifyConfig {
            followed: false,
            ..NotifyConfig::default()
        });
        assert_eq!(notifier.consider(&event(&["f1"]), text()), None);
        assert!(
            notifier
                .consider(
                    &Event::Stream {
                        channel: "c1".into(),
                        live: false,
                    },
                    text(),
                )
                .is_none()
        );
        let joined_live = notifier.consider(
            &Event::Stream {
                channel: "c1".into(),
                live: true,
            },
            text(),
        );
        assert!(joined_live.is_some());
    }

    #[test]
    fn a_rule_can_light_up_an_info_line_and_keep_a_highlight_quiet() {
        let mut notifier = notifier();
        let info = Event::Line {
            channel: "c1".into(),
            line: Line {
                time: Timestamp::now(),
                content: LineContent::UserNotice {
                    system_text: "alice subscribed".into(),
                    message: None,
                },
                flags: highlighted(),
            },
        };
        let alert = notifier.consider(&info, text()).unwrap();
        assert_eq!(alert.title, "Highlighted in #Harborlight");
        assert_eq!(alert.body, "alice subscribed");
        assert!(!alert.silent && !alert.quiet);

        let alert = notifier
            .consider(
                &line(LineFlags {
                    highlight_silent: true,
                    highlight_quiet: true,
                    ..highlighted()
                }),
                text(),
            )
            .unwrap();
        assert!(alert.silent && alert.quiet);
    }

    #[test]
    fn only_moderation_newer_than_the_frontend() {
        use liveside_proto::event::{HeldMessage, UnbanRequest};
        let mut notifier = notifier();
        let held = |time| Event::MessageHeld {
            channel: "c1".into(),
            held: HeldMessage {
                id: "h1".into(),
                sender: user("rude"),
                text: "bad words".into(),
                reason: "automod".into(),
                detail: String::new(),
                time,
            },
        };
        let alert = notifier.consider(&held(Timestamp::now()), text()).unwrap();
        assert_eq!(alert.title, "Held in #Harborlight");
        assert_eq!(alert.body, "RUDE: bad words");
        // What a join lists as already waiting is not news.
        let old = Timestamp(Timestamp::now().0 - 3_600_000);
        assert_eq!(notifier.consider(&held(old), text()), None);
        let unban = Event::UnbanRequested {
            channel: "c1".into(),
            request: UnbanRequest {
                id: "u1".into(),
                user: user("sorry"),
                text: String::new(),
                time: old,
            },
        };
        assert_eq!(notifier.consider(&unban, text()), None);
    }

    #[test]
    fn what_is_on_screen_is_not_notified_unless_asked() {
        let mut notifier = notifier();
        let alert = notifier.consider(&line(highlighted()), text()).unwrap();
        let here = Seen {
            focused: true,
            place: Some(Place::Channel("c1".into())),
            beside: Vec::new(),
        };
        let elsewhere = Seen {
            focused: true,
            place: Some(Place::Channel("c2".into())),
            beside: Vec::new(),
        };
        let away = Seen {
            focused: false,
            place: Some(Place::Channel("c1".into())),
            beside: Vec::new(),
        };
        let docked = Seen {
            beside: vec![Place::Channel("c1".into())],
            ..elsewhere.clone()
        };
        assert!(!notifier.wanted(&alert, &docked), "on screen beside");
        assert!(!notifier.wanted(&alert, &here));
        assert!(notifier.wanted(&alert, &elsewhere));
        assert!(notifier.wanted(&alert, &away));

        notifier.set_config(NotifyConfig {
            when: NotifyWhen::Unfocused,
            ..NotifyConfig::default()
        });
        assert!(!notifier.wanted(&alert, &elsewhere));
        assert!(notifier.wanted(&alert, &away));

        notifier.set_config(NotifyConfig {
            when: NotifyWhen::Always,
            ..NotifyConfig::default()
        });
        assert!(notifier.wanted(&alert, &here));
    }

    #[test]
    fn a_sound_plays_once_in_a_while() {
        let mut notifier = notifier();
        assert!(notifier.sound_now(Kind::Mention));
        assert!(!notifier.sound_now(Kind::Mention));
        // Another kind has its own pause.
        assert!(notifier.sound_now(Kind::Whisper));
        notifier.set_config(NotifyConfig {
            sound: false,
            ..NotifyConfig::default()
        });
        assert!(!notifier.sound_now(Kind::Live));
    }

    #[test]
    fn long_bodies_are_cut_and_kept_on_one_line() {
        assert_eq!(shortened(" a\nb "), "a b");
        let long = "x".repeat(BODY_CHARS + 10);
        let cut = shortened(&long);
        assert_eq!(cut.chars().count(), BODY_CHARS + 1);
        assert!(cut.ends_with('…'));
    }

    #[test]
    fn theme_sounds_are_found_by_name_or_a_shorter_one() {
        let dir = std::env::temp_dir().join(format!("liveside-sounds-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let stereo = dir.join("share/sounds/freedesktop/stereo");
        std::fs::create_dir_all(&stereo).unwrap();
        std::fs::write(stereo.join("message.oga"), b"").unwrap();
        std::fs::write(stereo.join("dialog-warning.wav"), b"").unwrap();
        let dirs = vec![dir.join("nothing-here"), dir.join("share")];

        assert_eq!(
            theme_sound("dialog-warning", &dirs),
            Some(stereo.join("dialog-warning.wav"))
        );
        // message-new-instant → message-new → message.
        assert_eq!(
            theme_sound("message-new-instant", &dirs),
            Some(stereo.join("message.oga"))
        );
        assert_eq!(theme_sound("service-login", &dirs), None);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_mac_sound_is_found_only_where_it_is() {
        let dir = std::env::temp_dir().join(format!("liveside-mac-sounds-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("Glass.aiff"), b"").unwrap();

        assert_eq!(
            system_sound(Kind::Mention.mac_sound(), &dir),
            Some(dir.join("Glass.aiff"))
        );
        assert_eq!(system_sound(Kind::Moderation.mac_sound(), &dir), None);
        assert_eq!(system_sound("Glass", &dir.join("nothing-here")), None);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    fn played(players: &'static [&'static str]) -> Option<PlayError> {
        let (tx, rx) = std::sync::mpsc::channel();
        play_with(players, Path::new("/nonexistent.oga"), move |error| {
            let _ = tx.send(error);
        });
        rx.recv_timeout(Duration::from_secs(5)).ok()
    }

    #[test]
    fn the_first_player_that_plays_is_enough_and_a_failure_is_said() {
        // `true` stands in for a player that played.
        assert_eq!(played(&["liveside-no-such-player", "true"]), None);
        assert_eq!(
            played(&["liveside-no-such-player"]),
            Some(PlayError::NoPlayer)
        );
        // `sh -c` would need arguments; `false` fails without a word.
        assert!(matches!(played(&["false"]), Some(PlayError::Failed(_))));
        // One that fails gives way to the next, as pw-play does to paplay
        // inside a Flatpak.
        assert_eq!(played(&["false", "true"]), None);
        assert!(matches!(
            played(&["false", "liveside-no-such-player"]),
            Some(PlayError::Failed(_))
        ));
    }

    #[test]
    fn a_file_from_the_settings_comes_before_the_theme() {
        let mut notifier = notifier();
        notifier.set_config(NotifyConfig {
            whisper_sound: " /home/me/ping.wav ".to_owned(),
            ..NotifyConfig::default()
        });
        assert_eq!(
            notifier.sound_file(Kind::Whisper),
            Some(PathBuf::from("/home/me/ping.wav"))
        );
    }

    #[test]
    fn a_click_on_our_notification_leads_to_its_place_with_its_token() {
        let mut raised = Raised::default();
        let place = Place::Channel("c1".into());
        raised.shown(7, place.clone());
        // Another program's notification, its token and its click.
        raised.token(8, "theirs".to_owned());
        assert_eq!(raised.invoked(8, "default"), None);
        raised.token(7, "ours".to_owned());
        assert_eq!(
            raised.invoked(7, "default"),
            Some((place.clone(), Some("ours".to_owned())))
        );
        // A click without a token still leads there; a button does not.
        assert_eq!(raised.invoked(7, "default"), Some((place.clone(), None)));
        raised.token(7, "button".to_owned());
        assert_eq!(raised.invoked(7, "other"), None);
        assert_eq!(raised.invoked(7, "default"), Some((place, None)));
        raised.closed(7);
        assert_eq!(raised.invoked(7, "default"), None);
    }

    #[test]
    fn a_notification_is_replaced_only_while_it_is_on_screen() {
        let then = Instant::now();
        let later = |seconds| then + Duration::from_secs(seconds);
        assert_eq!(earlier(None, then, 10), Earlier::None);
        assert_eq!(earlier(Some((3, then)), later(9), 10), Earlier::OnScreen(3));
        assert_eq!(earlier(Some((3, then)), later(10), 10), Earlier::Gone(3));
        assert_eq!(
            earlier(Some((3, then)), later(30), 60),
            Earlier::OnScreen(3)
        );
        // Left to the desktop, it counts as Liveside's own default.
        assert_eq!(earlier(Some((3, then)), later(9), 0), Earlier::OnScreen(3));
        assert_eq!(earlier(Some((3, then)), later(11), 0), Earlier::Gone(3));
    }

    #[test]
    fn a_notification_stays_as_long_as_set_or_as_the_desktop_decides() {
        assert_eq!(expire_timeout(0), -1);
        assert_eq!(expire_timeout(10), 10_000);
        assert_eq!(expire_timeout(u32::MAX), i32::MAX);
    }
}
