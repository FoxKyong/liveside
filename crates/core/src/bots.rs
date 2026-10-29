//! The user's own bots: programs the engine starts and talks to in JSON
//! lines, so that they need no token or Twitch connection of their own.
//! The protocol is in `docs/bots.md`, the decisions in
//! `docs/architecture.md`, *The user's own bots*.
//!
//! Each run of a bot is a process with a task that writes its input, two
//! that read its output and its errors, and one that waits for it to end.
//! They report to the engine through one channel, tagged with the run, so
//! what a run says after it was replaced is told apart from the new one.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

use liveside_proto::model::{BotConfig, BotLogLine, BotState, BotStatus, ChatMessage};
use liveside_proto::{CommandError, ErrorKind, Event, MessageId, Timestamp};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::{mpsc, oneshot};

/// The window a bot's `rate` counts lines in, as Twitch counts them.
const RATE_WINDOW: Duration = Duration::from_secs(30);

/// Lines kept of each bot's log.
const LOG_LINES: usize = 200;

/// Lines waiting for a bot that does not read them. Past this they are
/// dropped: a bot that stopped reading should not make the engine hold
/// the whole chat for it.
const INPUT_BACKLOG: usize = 1000;

/// The version of the protocol, said in `hello`.
const PROTOCOL: u32 = 1;

/// `$XDG_RUNTIME_DIR/liveside/bots`, or a directory in the temporary one:
/// where running bots hold their locks.
pub fn default_lock_dir() -> PathBuf {
    match std::env::var_os("XDG_RUNTIME_DIR") {
        Some(dir) if !dir.is_empty() => PathBuf::from(dir).join("liveside").join("bots"),
        _ => std::env::temp_dir().join("liveside-bots"),
    }
}

/// Whether a list of bots can be used: every one named, once, and with a
/// command.
pub fn check(bots: &[BotConfig]) -> Result<(), CommandError> {
    for (i, bot) in bots.iter().enumerate() {
        let name = bot.name.trim();
        if name.is_empty() {
            return Err(CommandError::new(
                ErrorKind::InvalidArgument,
                "bot.no_name",
                &[],
                "A bot has no name",
            ));
        }
        if bot
            .command
            .first()
            .is_none_or(|program| program.trim().is_empty())
        {
            return Err(CommandError::new(
                ErrorKind::InvalidArgument,
                "bot.no_command",
                &[("bot", name)],
                format!("Bot {name} has no command"),
            ));
        }
        if !(1..=BotConfig::MAX_RATE).contains(&bot.rate) {
            let most = BotConfig::MAX_RATE.to_string();
            return Err(CommandError::new(
                ErrorKind::InvalidArgument,
                "bot.rate",
                &[("bot", name), ("most", &most)],
                format!("Bot {name} may say from 1 to {most} lines in 30 seconds"),
            ));
        }
        if !(1..=BotConfig::MAX_STOP_AFTER).contains(&bot.stop_after) {
            let most = BotConfig::MAX_STOP_AFTER.to_string();
            return Err(CommandError::new(
                ErrorKind::InvalidArgument,
                "bot.stop_after",
                &[("bot", name), ("most", &most)],
                format!("Bot {name} must be given from 1 to {most} seconds to stop"),
            ));
        }
        if bots[..i]
            .iter()
            .any(|other| other.name.trim().eq_ignore_ascii_case(name))
        {
            return Err(CommandError::new(
                ErrorKind::InvalidArgument,
                "bot.twice",
                &[("bot", name)],
                format!("Two bots are called {name}"),
            ));
        }
    }
    Ok(())
}

/// What the tasks of a run report.
#[derive(Debug)]
pub enum Output {
    /// A line from its standard output.
    Line {
        name: String,
        run: u64,
        line: String,
    },
    /// A line from its standard error.
    Error {
        name: String,
        run: u64,
        line: String,
    },
    /// It ended; `asked` when it was stopped.
    Ended {
        name: String,
        run: u64,
        code: Option<i32>,
        asked: bool,
    },
    /// Twitch would not take what it said.
    Refused { name: String, error: CommandError },
}

/// A line a bot asked to be said, for the engine to send.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Said {
    pub name: String,
    /// The channel's login.
    pub channel: String,
    pub text: String,
    pub reply_to: Option<MessageId>,
}

/// What a bot is told, in the order `docs/bots.md` shows: the type first
/// reads best to whoever looks at the lines.
#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Told<'a> {
    Hello {
        protocol: u32,
        name: &'a str,
    },
    Message {
        channel: &'a str,
        id: &'a str,
        user: Person<'a>,
        badges: Vec<&'a str>,
        text: String,
        action: bool,
    },
}

#[derive(Serialize)]
struct Person<'a> {
    id: &'a str,
    login: &'a str,
    name: &'a str,
}

impl Told<'_> {
    fn line(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }
}

/// What a bot can write, as far as the engine understands it.
#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Request {
    Say {
        channel: String,
        text: String,
    },
    Reply {
        channel: String,
        to: String,
        text: String,
    },
}

/// A running process.
struct Running {
    /// Lines for its input; gone once it is asked to stop, which ends its
    /// input.
    input: Option<mpsc::Sender<String>>,
    stop: Option<oneshot::Sender<()>>,
    /// Held while it runs; the lock goes with it.
    _lock: std::fs::File,
}

struct Bot {
    config: BotConfig,
    state: BotState,
    log: VecDeque<BotLogLine>,
    /// The latest run, whose reports count.
    run: u64,
    running: Option<Running>,
    /// When it said its latest lines.
    said: VecDeque<Instant>,
    /// Whether the log already says it is going too fast, or that it does
    /// not read, so a flood makes one line in the log and not a flood.
    throttled: bool,
    deaf: bool,
    /// Taken out of the list while it ran: it goes once it has ended.
    removed: bool,
}

impl Bot {
    fn new(config: BotConfig) -> Self {
        Self {
            config,
            state: BotState::Stopped,
            log: VecDeque::new(),
            run: 0,
            running: None,
            said: VecDeque::new(),
            throttled: false,
            deaf: false,
            removed: false,
        }
    }

    fn status(&self) -> BotStatus {
        BotStatus {
            config: self.config.clone(),
            state: self.state.clone(),
            log: self.log.iter().cloned().collect(),
        }
    }

    /// Keeps a line in its log, and the event that tells the frontends.
    fn note(&mut self, code: &str, args: &[(&str, &str)], text: String) -> Event {
        self.log_line(BotLogLine {
            time: Timestamp::now(),
            text,
            code: code.to_owned(),
            args: args
                .iter()
                .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
                .collect(),
        })
    }

    fn log_line(&mut self, line: BotLogLine) -> Event {
        self.log.push_back(line.clone());
        while self.log.len() > LOG_LINES {
            self.log.pop_front();
        }
        Event::BotLog {
            name: self.config.name.clone(),
            line,
        }
    }

    fn state_event(&self) -> Event {
        Event::BotState {
            name: self.config.name.clone(),
            state: self.state.clone(),
        }
    }

    /// Asks it to stop: its input ends, and it is killed after the grace.
    fn stop(&mut self) -> bool {
        match &mut self.running {
            Some(running) if running.input.is_some() => {
                running.input = None;
                if let Some(stop) = running.stop.take() {
                    let _ = stop.send(());
                }
                true
            }
            _ => false,
        }
    }
}

pub struct Bots {
    list: Vec<Bot>,
    locks: PathBuf,
    output: mpsc::UnboundedSender<Output>,
    next_run: u64,
}

impl Bots {
    pub fn new(
        configs: Vec<BotConfig>,
        locks: PathBuf,
        output: mpsc::UnboundedSender<Output>,
    ) -> Self {
        Self {
            list: configs.into_iter().map(Bot::new).collect(),
            locks,
            output,
            next_run: 1,
        }
    }

    /// Where a refusal from Twitch is to be reported.
    pub fn reports(&self) -> mpsc::UnboundedSender<Output> {
        self.output.clone()
    }

    pub fn statuses(&self) -> Vec<BotStatus> {
        self.list.iter().map(Bot::status).collect()
    }

    fn find(&mut self, name: &str) -> Result<&mut Bot, CommandError> {
        self.list
            .iter_mut()
            .find(|bot| bot.config.name.eq_ignore_ascii_case(name))
            .ok_or_else(|| {
                CommandError::new(
                    ErrorKind::InvalidArgument,
                    "bot.unknown",
                    &[("bot", name)],
                    format!("No bot is called {name}"),
                )
            })
    }

    /// Starts the ones that start with the engine.
    pub fn autostart(&mut self) -> Vec<Event> {
        let names: Vec<String> = self
            .list
            .iter()
            .filter(|bot| bot.config.autostart)
            .map(|bot| bot.config.name.clone())
            .collect();
        names
            .iter()
            .flat_map(|name| self.start(name).unwrap_or_default())
            .collect()
    }

    /// Starts a bot. A bot that cannot start says so in its state and its
    /// log rather than as an error: it belongs to the bot, and stays on
    /// show beside it.
    pub fn start(&mut self, name: &str) -> Result<Vec<Event>, CommandError> {
        let run = self.next_run;
        let locks = self.locks.clone();
        let output = self.output.clone();
        let bot = self.find(name)?;
        if bot.running.is_some() {
            return Err(CommandError::new(
                ErrorKind::InvalidArgument,
                "bot.running",
                &[("bot", &bot.config.name)],
                format!("Bot {} is running already", bot.config.name),
            ));
        }
        let note = match lock(&locks, &bot.config.name) {
            Ok(Some(file)) => match spawn(&bot.config, run, output) {
                Ok((input, stop)) => {
                    bot.run = run;
                    bot.running = Some(Running {
                        input: Some(input),
                        stop: Some(stop),
                        _lock: file,
                    });
                    bot.said.clear();
                    bot.throttled = false;
                    bot.deaf = false;
                    bot.state = BotState::Running;
                    bot.note("bot.started", &[], "Started".to_owned())
                }
                Err(e) => {
                    let error = e.to_string();
                    bot.state = BotState::Failed {
                        error: error.clone(),
                    };
                    bot.note(
                        "bot.cannot_start",
                        &[("error", &error)],
                        format!("Could not start: {error}"),
                    )
                }
            },
            Ok(None) => {
                bot.state = BotState::Elsewhere;
                bot.note(
                    "bot.elsewhere",
                    &[],
                    "Running in another Liveside on this machine".to_owned(),
                )
            }
            Err(e) => {
                let error = e.to_string();
                bot.state = BotState::Failed {
                    error: error.clone(),
                };
                bot.note(
                    "bot.cannot_start",
                    &[("error", &error)],
                    format!("Could not start: {error}"),
                )
            }
        };
        let events = vec![bot.state_event(), note];
        self.next_run += 1;
        Ok(events)
    }

    /// Asks a bot to stop. Its state changes when it has ended.
    pub fn stop(&mut self, name: &str) -> Result<Vec<Event>, CommandError> {
        let bot = self.find(name)?;
        // One waiting for another Liveside to let it go stops waiting.
        if bot.state == BotState::Elsewhere {
            bot.state = BotState::Stopped;
            let note = bot.note("bot.stopped", &[], "Stopped".to_owned());
            return Ok(vec![bot.state_event(), note]);
        }
        if !bot.stop() {
            return Err(CommandError::new(
                ErrorKind::InvalidArgument,
                "bot.not_running",
                &[("bot", &bot.config.name)],
                format!("Bot {} is not running", bot.config.name),
            ));
        }
        Ok(vec![bot.note("bot.stopping", &[], "Stopping".to_owned())])
    }

    /// Starts the bots another Liveside on this machine was running, once
    /// it has let them go: closed, or stopped them. Asked every few
    /// seconds; a bot still held elsewhere is left as it is, unsaid.
    pub fn take_over(&mut self) -> Vec<Event> {
        let waiting: Vec<String> = self
            .list
            .iter()
            .filter(|bot| bot.state == BotState::Elsewhere && bot.running.is_none())
            .map(|bot| bot.config.name.clone())
            .collect();
        let mut events = Vec::new();
        for name in waiting {
            if let Ok(Some(free)) = lock(&self.locks, &name) {
                drop(free);
                events.extend(self.start(&name).unwrap_or_default());
            }
        }
        events
    }

    /// Takes a new list. A bot no longer in it is stopped and goes once it
    /// has ended; one that changed keeps running as it was until it is
    /// started again. The whole list comes back to be published.
    pub fn set(&mut self, configs: Vec<BotConfig>) -> Result<Event, CommandError> {
        check(&configs)?;
        let mut old = std::mem::take(&mut self.list);
        for config in configs {
            match old
                .iter()
                .position(|bot| bot.config.name.eq_ignore_ascii_case(&config.name))
            {
                Some(at) => {
                    let mut bot = old.remove(at);
                    bot.config = config;
                    self.list.push(bot);
                }
                None => self.list.push(Bot::new(config)),
            }
        }
        // Gone from the list, but not from the machine until they end:
        // dropping them would leave their processes to the kill on drop.
        for mut bot in old {
            if bot.running.is_some() {
                bot.stop();
                bot.removed = true;
                self.list.push(bot);
            }
        }
        Ok(Event::Bots {
            bots: self.statuses(),
        })
    }

    /// Hands a chat message to the running bots of its channel. `said_by`
    /// is the bot whose line it is, coming back as the account's: that bot
    /// never hears it, and the others only when they hear bots. `hidden`
    /// is a message the ignore rules hide, for the bots that hear those.
    pub fn message(
        &mut self,
        channel: &str,
        message: &ChatMessage,
        said_by: Option<&str>,
        hidden: bool,
    ) -> Vec<Event> {
        let line = Told::Message {
            channel,
            id: message.id.as_str(),
            user: Person {
                id: message.sender.id.as_str(),
                login: message.sender.login.as_str(),
                name: &message.sender.display_name,
            },
            badges: message.badges.iter().map(|b| b.set_id.as_str()).collect(),
            text: message.plain_text(),
            action: message.kind == liveside_proto::model::MessageKind::Action,
        }
        .line();
        let mut events = Vec::new();
        for bot in &mut self.list {
            let from_a_bot = match said_by {
                Some(name) => !bot.config.hear_bots || bot.config.name.eq_ignore_ascii_case(name),
                None => false,
            };
            if !bot.config.serves(channel) || from_a_bot || (hidden && !bot.config.hear_ignored) {
                continue;
            }
            let Some(input) = bot.running.as_ref().and_then(|r| r.input.as_ref()) else {
                continue;
            };
            match input.try_send(line.clone()) {
                Ok(()) => bot.deaf = false,
                Err(mpsc::error::TrySendError::Full(_)) if !bot.deaf => {
                    bot.deaf = true;
                    events.push(bot.note(
                        "bot.not_reading",
                        &[],
                        "It does not read what it is sent; messages are dropped".to_owned(),
                    ));
                }
                Err(_) => {}
            }
        }
        events
    }

    /// What a run reported: events to publish, and lines to say.
    pub fn report(&mut self, output: Output, now: Instant) -> (Vec<Event>, Vec<Said>) {
        let mut events = Vec::new();
        let mut said = Vec::new();
        match output {
            Output::Line { name, run, line } => {
                let Ok(bot) = self.find(&name) else {
                    return (events, said);
                };
                if bot.run != run {
                    return (events, said);
                }
                match understand(bot, &line, now) {
                    Ok(Some(line)) => said.push(line),
                    Ok(None) => {}
                    Err(event) => events.push(*event),
                }
            }
            Output::Error { name, run, line } => {
                if let Ok(bot) = self.find(&name)
                    && bot.run == run
                {
                    events.push(bot.log_line(BotLogLine {
                        time: Timestamp::now(),
                        text: line,
                        code: String::new(),
                        args: Vec::new(),
                    }));
                }
            }
            Output::Ended {
                name,
                run,
                code,
                asked,
            } => {
                let Some(at) = self
                    .list
                    .iter()
                    .position(|bot| bot.config.name.eq_ignore_ascii_case(&name))
                else {
                    return (events, said);
                };
                let bot = &mut self.list[at];
                if bot.run != run {
                    return (events, said);
                }
                bot.running = None;
                bot.state = if asked {
                    BotState::Stopped
                } else {
                    BotState::Ended { code }
                };
                events.push(bot.state_event());
                events.push(match (asked, code) {
                    (true, _) => bot.note("bot.stopped", &[], "Stopped".to_owned()),
                    (false, Some(code)) => bot.note(
                        "bot.ended",
                        &[("code", &code.to_string())],
                        format!("Ended with exit code {code}"),
                    ),
                    (false, None) => bot.note("bot.killed", &[], "Ended by a signal".to_owned()),
                });
                // One taken out of the list while it ran goes now.
                if bot.removed {
                    self.list.remove(at);
                    events.push(Event::Bots {
                        bots: self.statuses(),
                    });
                }
            }
            Output::Refused { name, error } => {
                if let Ok(bot) = self.find(&name) {
                    events.push(bot.note(
                        "bot.refused",
                        &[("error", &error.message)],
                        format!("Twitch refused it: {}", error.message),
                    ));
                }
            }
        }
        (events, said)
    }

    /// A line for a bot's log that the engine has to say, such as that the
    /// channel it named is not joined.
    pub fn note(
        &mut self,
        name: &str,
        code: &str,
        args: &[(&str, &str)],
        text: String,
    ) -> Option<Event> {
        self.find(name).ok().map(|bot| bot.note(code, args, text))
    }

    /// Stops every bot and waits, for as long as the grace, for them to
    /// end, so that closing the engine ends them too.
    pub async fn shutdown(&mut self) {
        let mut longest = None;
        for bot in &mut self.list {
            if bot.stop() {
                longest = longest.max(Some(bot.config.stop_after));
            }
        }
        if let Some(seconds) = longest {
            tokio::time::sleep(Duration::from_secs(seconds.into()) + Duration::from_millis(200))
                .await;
        }
    }
}

/// Reads a line a bot wrote: something to say, nothing, or why not.
fn understand(bot: &mut Bot, line: &str, now: Instant) -> Result<Option<Said>, Box<Event>> {
    let request: Request = match serde_json::from_str(line) {
        Ok(request) => request,
        Err(_) => {
            return Err(Box::new(bot.note(
                "bot.not_understood",
                &[("line", line)],
                format!("Not understood: {line}"),
            )));
        }
    };
    let (channel, text, reply_to) = match request {
        Request::Say { channel, text } => (channel, text, None),
        Request::Reply { channel, to, text } => (channel, text, Some(MessageId::from(to))),
    };
    let channel = channel.trim_start_matches('#').to_lowercase();
    if !bot.config.serves(&channel) {
        return Err(Box::new(bot.note(
            "bot.not_its_channel",
            &[("channel", &channel)],
            format!("#{channel} is not one of its channels"),
        )));
    }
    let text = text.trim().to_owned();
    if text.is_empty() {
        return Ok(None);
    }
    while bot
        .said
        .front()
        .is_some_and(|at| now.duration_since(*at) >= RATE_WINDOW)
    {
        bot.said.pop_front();
    }
    let rate = bot.config.rate;
    if bot.said.len() >= rate as usize {
        if bot.throttled {
            return Ok(None);
        }
        bot.throttled = true;
        let shown = rate.to_string();
        return Err(Box::new(bot.note(
            "bot.too_fast",
            &[("rate", &shown)],
            format!("More than {rate} lines in 30 seconds; the rest are dropped"),
        )));
    }
    bot.throttled = false;
    bot.said.push_back(now);
    Ok(Some(Said {
        name: bot.config.name.clone(),
        channel,
        text,
        reply_to,
    }))
}

/// Takes the bot's lock: the file, or `None` when another engine holds it.
fn lock(dir: &Path, name: &str) -> std::io::Result<Option<std::fs::File>> {
    std::fs::create_dir_all(dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
    }
    // Any name makes a file name: what is not plainly safe is spelled out.
    let file_name: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c.to_string()
            } else {
                format!("%{:x}", c as u32)
            }
        })
        .collect();
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(dir.join(format!("{file_name}.lock")))?;
    match file.try_lock() {
        Ok(()) => Ok(Some(file)),
        Err(std::fs::TryLockError::WouldBlock) => Ok(None),
        Err(std::fs::TryLockError::Error(e)) => Err(e),
    }
}

/// Starts a run: the process, and the tasks around it. Gives back where
/// its input goes and how to ask it to stop.
fn spawn(
    config: &BotConfig,
    run: u64,
    output: mpsc::UnboundedSender<Output>,
) -> std::io::Result<(mpsc::Sender<String>, oneshot::Sender<()>)> {
    let (program, args) = config
        .command
        .split_first()
        .ok_or_else(|| std::io::Error::other("no command"))?;
    let dir = match config.dir.trim() {
        "" => std::env::var_os("HOME").map(PathBuf::from),
        dir => Some(PathBuf::from(dir)),
    };
    let mut command = tokio::process::Command::from(crate::host::command(program, dir.as_deref()));
    command
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let mut child = command.spawn()?;
    let (Some(mut stdin), Some(stdout), Some(stderr)) =
        (child.stdin.take(), child.stdout.take(), child.stderr.take())
    else {
        return Err(std::io::Error::other("no pipes"));
    };
    let name = config.name.clone();
    let grace = Duration::from_secs(config.stop_after.into());

    let (input, mut lines) = mpsc::channel::<String>(INPUT_BACKLOG);
    let hello = Told::Hello {
        protocol: PROTOCOL,
        name: &name,
    }
    .line();
    tokio::spawn(async move {
        let mut next = Some(hello);
        while let Some(line) = next {
            if stdin.write_all(line.as_bytes()).await.is_err()
                || stdin.write_all(b"\n").await.is_err()
                || stdin.flush().await.is_err()
            {
                break;
            }
            next = lines.recv().await;
        }
        // Dropping it ends the bot's input: the sign to finish.
    });

    let reader = |stream: Box<dyn tokio::io::AsyncRead + Unpin + Send>, errors: bool| {
        let (output, name) = (output.clone(), name.clone());
        tokio::spawn(async move {
            let mut lines = BufReader::new(stream).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let name = name.clone();
                let report = if errors {
                    Output::Error { name, run, line }
                } else {
                    Output::Line { name, run, line }
                };
                if output.send(report).is_err() {
                    break;
                }
            }
        })
    };
    let out = reader(Box::new(stdout), false);
    let err = reader(Box::new(stderr), true);

    let (stop, stopped) = oneshot::channel::<()>();
    tokio::spawn(async move {
        let mut asked = false;
        let status = tokio::select! {
            status = child.wait() => status,
            _ = stopped => {
                asked = true;
                match tokio::time::timeout(grace, child.wait()).await {
                    Ok(status) => status,
                    Err(_) => {
                        let _ = child.kill().await;
                        child.wait().await
                    }
                }
            }
        };
        // What it wrote before it ended is reported before that it ended.
        let _ = tokio::time::timeout(Duration::from_secs(1), out).await;
        let _ = tokio::time::timeout(Duration::from_secs(1), err).await;
        let code = status.ok().and_then(|status| status.code());
        let _ = output.send(Output::Ended {
            name,
            run,
            code,
            asked,
        });
    });
    Ok((input, stop))
}

#[cfg(test)]
mod tests {
    use super::*;
    use liveside_proto::model::{Badge, Fragment, User};

    fn temp_dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("liveside-bots-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    /// A bot that is `sh -c script`, heard in `brightmaple` only.
    fn bot(name: &str, script: &str) -> BotConfig {
        BotConfig {
            name: name.to_owned(),
            command: vec!["sh".into(), "-c".into(), script.into()],
            dir: std::env::temp_dir().display().to_string(),
            channels: vec!["brightmaple".into()],
            autostart: false,
            ..Default::default()
        }
    }

    fn message(id: &str, text: &str) -> ChatMessage {
        ChatMessage {
            id: id.into(),
            sender: User {
                id: "7".into(),
                login: "pineviewer".into(),
                display_name: "Pineviewer".into(),
            },
            color: None,
            badges: vec![Badge {
                set_id: "vip".into(),
                id: "1".into(),
                info: String::new(),
                image: String::new(),
                title: String::new(),
            }],
            fragments: vec![Fragment::Text { text: text.into() }],
            kind: Default::default(),
            bits: None,
            reply: None,
            shared_source: None,
        }
    }

    /// Feeds what the runs report back in, until `done` says enough or
    /// five seconds pass; gives back the events and what was said.
    async fn settle(
        bots: &mut Bots,
        output: &mut mpsc::UnboundedReceiver<Output>,
        done: impl Fn(&[Event], &[Said]) -> bool,
    ) -> (Vec<Event>, Vec<Said>) {
        let (mut events, mut said) = (Vec::new(), Vec::new());
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        while !done(&events, &said) {
            let Ok(Some(report)) = tokio::time::timeout_at(deadline, output.recv()).await else {
                break;
            };
            let (e, s) = bots.report(report, Instant::now());
            events.extend(e);
            said.extend(s);
        }
        (events, said)
    }

    fn codes(events: &[Event]) -> Vec<String> {
        events
            .iter()
            .filter_map(|event| match event {
                Event::BotLog { line, .. } if !line.code.is_empty() => Some(line.code.clone()),
                Event::BotLog { line, .. } => Some(format!("stderr: {}", line.text)),
                _ => None,
            })
            .collect()
    }

    fn ended(events: &[Event]) -> bool {
        events.iter().any(|event| {
            matches!(event, Event::BotState { state, .. }
                if matches!(state, BotState::Stopped | BotState::Ended { .. }))
        })
    }

    #[tokio::test]
    async fn a_bot_hears_its_channel_and_answers() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        // It says what it was greeted with on its errors, and answers the
        // message it gets with a reply to it.
        let script = r##"read hello; echo "$hello" >&2
            while read line; do
              id=$(echo "$line" | sed 's/.*"channel":"[^"]*","id":"\([^"]*\)".*/\1/')
              echo "{\"type\":\"reply\",\"channel\":\"#BrightMaple\",\"to\":\"$id\",\"text\":\" hi \"}"
            done"##;
        let mut bots = Bots::new(vec![bot("echo", script)], temp_dir("hear"), tx);
        let events = bots.start("echo").unwrap();
        assert!(matches!(
            &events[0],
            Event::BotState {
                state: BotState::Running,
                ..
            }
        ));

        // Not its channel: nothing reaches it.
        bots.message("harborlight", &message("m0", "!pocasi"), None, false);
        bots.message("brightmaple", &message("m1", "!pocasi Brno"), None, false);
        let (events, said) = settle(&mut bots, &mut rx, |_, said| !said.is_empty()).await;
        assert_eq!(
            codes(&events),
            [r#"stderr: {"type":"hello","protocol":1,"name":"echo"}"#]
        );
        assert_eq!(
            said,
            [Said {
                name: "echo".into(),
                channel: "brightmaple".into(),
                text: "hi".into(),
                reply_to: Some("m1".into()),
            }]
        );

        bots.stop("echo").unwrap();
        let (events, _) = settle(&mut bots, &mut rx, |events, _| ended(events)).await;
        assert!(codes(&events).contains(&"bot.stopped".to_owned()));
        assert_eq!(bots.statuses()[0].state, BotState::Stopped);
    }

    #[tokio::test]
    async fn the_message_says_who_and_what() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut bots = Bots::new(
            vec![bot("tell", "read hello; read line; echo \"$line\" >&2")],
            temp_dir("message"),
            tx,
        );
        bots.start("tell").unwrap();
        bots.message("brightmaple", &message("m1", "!pocasi Brno"), None, false);
        let (events, _) = settle(&mut bots, &mut rx, |events, _| ended(events)).await;
        let line = codes(&events)
            .into_iter()
            .find_map(|c| c.strip_prefix("stderr: ").map(str::to_owned))
            .unwrap();
        let value: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert_eq!(value["type"], "message");
        assert_eq!(value["channel"], "brightmaple");
        assert_eq!(value["id"], "m1");
        assert_eq!(value["user"]["login"], "pineviewer");
        assert_eq!(value["user"]["name"], "Pineviewer");
        assert_eq!(value["badges"], serde_json::json!(["vip"]));
        assert_eq!(value["text"], "!pocasi Brno");
        assert_eq!(value["action"], false);
    }

    #[tokio::test]
    async fn a_bot_that_ends_says_how_and_is_not_started_again() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut bots = Bots::new(
            vec![bot("broken", "echo nonsense; echo oops >&2; exit 3")],
            temp_dir("ends"),
            tx,
        );
        bots.start("broken").unwrap();
        let (events, said) = settle(&mut bots, &mut rx, |events, _| ended(events)).await;
        assert!(said.is_empty());
        assert_eq!(
            codes(&events),
            ["bot.not_understood", "stderr: oops", "bot.ended"]
        );
        assert_eq!(bots.statuses()[0].state, BotState::Ended { code: Some(3) });
        // Started again only when asked, and then it can be.
        assert!(bots.start("broken").is_ok());
    }

    #[tokio::test]
    async fn a_bot_cannot_flood_or_speak_elsewhere() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let script = r#"echo '{"type":"say","channel":"harborlight","text":"x"}'
            i=0; while [ $i -lt 25 ]; do
              echo '{"type":"say","channel":"brightmaple","text":"x"}'; i=$((i+1))
            done; sleep 5"#;
        let mut bots = Bots::new(vec![bot("loud", script)], temp_dir("flood"), tx);
        bots.start("loud").unwrap();
        let rate = BotConfig::DEFAULT_RATE as usize;
        let (events, said) = settle(&mut bots, &mut rx, |_, said| said.len() >= rate).await;
        let (more, more_said) = settle(&mut bots, &mut rx, |events, _| !events.is_empty()).await;
        assert_eq!(said.len() + more_said.len(), rate);
        let mut all = codes(&events);
        all.extend(codes(&more));
        // Once each, however much it tries.
        assert_eq!(all, ["bot.not_its_channel", "bot.too_fast"]);
        bots.stop("loud").unwrap();
        settle(&mut bots, &mut rx, |events, _| ended(events)).await;
    }

    #[tokio::test]
    async fn one_copy_runs_per_machine() {
        let dir = temp_dir("lock");
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut first = Bots::new(vec![bot("one", "cat >/dev/null")], dir.clone(), tx.clone());
        let mut second = Bots::new(vec![bot("one", "cat >/dev/null")], dir.clone(), tx);
        first.start("one").unwrap();
        second.start("one").unwrap();
        assert_eq!(first.statuses()[0].state, BotState::Running);
        assert_eq!(second.statuses()[0].state, BotState::Elsewhere);
        // Still held: waiting changes nothing and says nothing.
        assert!(second.take_over().is_empty());
        // Once it has ended, the other takes it over by itself.
        first.stop("one").unwrap();
        settle(&mut first, &mut rx, |events, _| ended(events)).await;
        assert!(!second.take_over().is_empty());
        assert_eq!(second.statuses()[0].state, BotState::Running);
        second.stop("one").unwrap();
        settle(&mut second, &mut rx, |events, _| ended(events)).await;
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn stopping_a_bot_held_elsewhere_stops_the_wait() {
        let dir = temp_dir("wait");
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut first = Bots::new(vec![bot("one", "cat >/dev/null")], dir.clone(), tx.clone());
        let mut second = Bots::new(vec![bot("one", "cat >/dev/null")], dir.clone(), tx);
        first.start("one").unwrap();
        second.start("one").unwrap();
        second.stop("one").unwrap();
        assert_eq!(second.statuses()[0].state, BotState::Stopped);
        first.stop("one").unwrap();
        settle(&mut first, &mut rx, |events, _| ended(events)).await;
        // Let go, but no longer wanted here.
        assert!(second.take_over().is_empty());
        assert_eq!(second.statuses()[0].state, BotState::Stopped);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn a_bot_that_does_not_stop_is_killed() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        // It ignores the end of its input.
        let mut bots = Bots::new(
            vec![bot(
                "stubborn",
                "trap '' TERM; while true; do sleep 1; done",
            )],
            temp_dir("kill"),
            tx,
        );
        bots.start("stubborn").unwrap();
        let asked = Instant::now();
        bots.stop("stubborn").unwrap();
        let (events, _) = settle(&mut bots, &mut rx, |events, _| ended(events)).await;
        assert!(ended(&events));
        assert!(asked.elapsed() >= Duration::from_secs(BotConfig::DEFAULT_STOP_AFTER.into()));
        assert_eq!(bots.statuses()[0].state, BotState::Stopped);
    }

    #[tokio::test]
    async fn a_bot_taken_out_of_the_list_goes_when_it_has_ended() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut bots = Bots::new(
            vec![bot("a", "cat >/dev/null"), bot("b", "cat >/dev/null")],
            temp_dir("set"),
            tx,
        );
        bots.start("a").unwrap();
        let mut b = bot("b", "true");
        b.autostart = true;
        let Event::Bots { bots: listed } = bots.set(vec![b]).unwrap() else {
            panic!("the list");
        };
        // Still there while it stops, then gone.
        assert_eq!(listed.len(), 2);
        assert_eq!(listed[0].config.command[2], "true");
        settle(&mut bots, &mut rx, |events, _| ended(events)).await;
        let names: Vec<String> = bots.statuses().into_iter().map(|s| s.config.name).collect();
        assert_eq!(names, ["b"]);
        // And a list that cannot be used changes nothing.
        assert!(bots.set(vec![bot("x", ""), bot("X", "")]).is_err());
        assert_eq!(bots.statuses().len(), 1);
    }

    #[tokio::test]
    async fn who_hears_bots_and_ignored_people_is_set_per_bot() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        // Each says on its errors the ids it was told.
        let script = r#"read hello; while read line; do
              echo "$line" | sed 's/.*"channel":"[^"]*","id":"\([^"]*\)".*/\1/' >&2
            done"#;
        let plain = bot("plain", script);
        let mut open_ears = bot("open", script);
        open_ears.hear_bots = true;
        open_ears.hear_ignored = true;
        let mut bots = Bots::new(vec![plain, open_ears], temp_dir("hearing"), tx);
        bots.start("plain").unwrap();
        bots.start("open").unwrap();
        bots.message("brightmaple", &message("everyone", "hi"), None, false);
        bots.message(
            "brightmaple",
            &message("from-plain", "x"),
            Some("plain"),
            false,
        );
        bots.message(
            "brightmaple",
            &message("from-open", "x"),
            Some("open"),
            false,
        );
        bots.message("brightmaple", &message("ignored", "x"), None, true);
        bots.stop("plain").unwrap();
        bots.stop("open").unwrap();
        let (events, _) = settle(&mut bots, &mut rx, |events, _| {
            events
                .iter()
                .filter(|e| {
                    matches!(
                        e,
                        Event::BotState {
                            state: BotState::Stopped,
                            ..
                        }
                    )
                })
                .count()
                == 2
        })
        .await;
        let heard = |who: &str| -> Vec<String> {
            events
                .iter()
                .filter_map(|e| match e {
                    Event::BotLog { name, line } if name == who && line.code.is_empty() => {
                        Some(line.text.clone())
                    }
                    _ => None,
                })
                .collect()
        };
        // Nobody hears itself; only the one that hears bots hears another.
        assert_eq!(heard("plain"), ["everyone"]);
        assert_eq!(heard("open"), ["everyone", "from-plain", "ignored"]);
    }

    #[tokio::test]
    async fn a_bot_may_be_allowed_more_lines() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let script = r#"i=0; while [ $i -lt 30 ]; do
              echo '{"type":"say","channel":"brightmaple","text":"x"}'; i=$((i+1))
            done; sleep 5"#;
        let mut config = bot("chatty", script);
        config.rate = 25;
        let mut bots = Bots::new(vec![config], temp_dir("rate"), tx);
        bots.start("chatty").unwrap();
        let (events, said) = settle(&mut bots, &mut rx, |events, _| {
            !codes(events).is_empty() && codes(events).contains(&"bot.too_fast".to_owned())
        })
        .await;
        assert_eq!(said.len(), 25);
        assert!(codes(&events).contains(&"bot.too_fast".to_owned()));
        bots.stop("chatty").unwrap();
        settle(&mut bots, &mut rx, |events, _| ended(events)).await;
    }

    #[test]
    fn limits_out_of_range_are_refused() {
        let mut config = bot("a", "true");
        config.rate = 0;
        assert_eq!(check(&[config.clone()]).unwrap_err().code, "bot.rate");
        config.rate = BotConfig::MAX_RATE + 1;
        assert_eq!(check(&[config.clone()]).unwrap_err().code, "bot.rate");
        config.rate = BotConfig::MAX_RATE;
        config.stop_after = 0;
        assert_eq!(check(&[config.clone()]).unwrap_err().code, "bot.stop_after");
        config.stop_after = BotConfig::MAX_STOP_AFTER;
        assert!(check(&[config]).is_ok());
    }

    #[tokio::test]
    async fn a_program_that_is_not_there_fails() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let mut config = bot("missing", "");
        config.command = vec!["/nonexistent/liveside-bot".into()];
        let mut bots = Bots::new(vec![config], temp_dir("missing"), tx);
        let events = bots.start("missing").unwrap();
        assert!(matches!(bots.statuses()[0].state, BotState::Failed { .. }));
        assert_eq!(codes(&events), ["bot.cannot_start"]);
    }
}
