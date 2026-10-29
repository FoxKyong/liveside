//! The rule language for highlighting, ignoring and filtering chat: one
//! rule per line, in the form Chatty's lists use, so a list kept there
//! carries over. See `docs/architecture.md`, section *Rules*.
//!
//! A rule is a few prefixed conditions and then the text to look for:
//!
//! ```text
//! [meta-prefix:value ...] [+[!]text-prefix:value ...] [!][text-prefix:]search text
//! ```
//!
//! `chan:harborlight w:rust` lights up "rust" as a word in #harborlight;
//! `user:nightbot` ignores one bot; `!status:smv startw:!bet` catches
//! "!bet" from anyone without a badge. Everything is matched against the
//! text as it reads, emotes by their names.
//!
//! Written from Chatty's documentation of the language, not from its code.
//! What Chatty has and this does not (a user's past messages, `if:`,
//! message tags) is refused when a rule is read, rather than quietly
//! matching something else. Address book categories (`cat:`, `chanCat:`)
//! are looked up by the engine, which fills them into the [`Subject`].

use std::fmt;
use std::ops::Range;

use liveside_proto::Rgb;
use liveside_proto::model::{Badge, Line, LineContent, MessageKind};
use regex::Regex;

/// One rule, read and ready to match.
#[derive(Debug, Clone)]
pub struct Rule {
    /// The line as written, for saying which rule did something.
    pub source: String,
    applies_to: AppliesTo,
    conditions: Vec<Condition>,
    /// The text that has to be found, if any: its matches are what a
    /// filter removes.
    main: Option<TextMatch>,
    /// Further text conditions (`+` prefixes, and a negated main text),
    /// which have to hold but mark nothing.
    extra: Vec<TextMatch>,
    /// Text whose match swallows a main match whole cancels it.
    blacklist: Vec<TextMatch>,
    pub style: Style,
    /// What a filter puts in place of what it removes.
    pub replacement: String,
}

/// What a matching highlight rule does besides lighting the line up.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Style {
    pub color: Option<Rgb>,
    pub background: Option<Rgb>,
    /// No sound for it (`config:silent`).
    pub silent: bool,
    /// No desktop notification for it (`config:!notify`).
    pub quiet: bool,
    /// Shown even when an ignore rule would hide it (`config:!ignore`).
    pub over_ignore: bool,
}

/// Which lines a rule looks at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AppliesTo {
    /// Messages people write: the default.
    Chat,
    /// Subscriptions, raids, announcements, and what Twitch or this
    /// client say in the channel (`config:info`).
    Info,
    /// Both (`config:any`).
    Any,
}

#[derive(Debug, Clone)]
enum Condition {
    User(String),
    NotUser(String),
    UserPattern(Regex),
    Channel(Vec<String>),
    NotChannel(Vec<String>),
    /// The sender is in at least one of these address book categories.
    Category(Vec<String>),
    /// The sender is not in all of them: Chatty's `!cat:a,b` fails only
    /// for someone in both, and `!cat:a !cat:b` for someone in either.
    NotCategory(Vec<String>),
    /// The same for the channel, by its `#name` in the book; `both` also
    /// looks under the name without `#` (`chanCat2:`).
    ChannelCategory {
        categories: Vec<String>,
        both: bool,
    },
    NotChannelCategory {
        categories: Vec<String>,
        both: bool,
    },
    /// At least one of these status codes.
    Status(String),
    /// None of these.
    NotStatus(String),
    FirstMessage,
    ChannelPoints,
    Historic,
    Link {
        message_only: bool,
    },
    /// Any of these badges, by set and optionally version.
    Badge(Vec<(String, Option<String>)>),
    Restricted,
    /// From Shared Chat, optionally from one of these channels.
    Shared(Vec<String>),
    Live(bool),
    /// Said at least this many times lately (`config:repeatedmsg`).
    Repeated(u32),
}

/// A piece of text to look for, and how.
#[derive(Debug, Clone)]
struct TextMatch {
    how: Find,
    /// Only in what the person wrote, not in the line around it (`msg`
    /// prefixes): for a subscription, the message attached to it.
    message_only: bool,
    /// Holds when the text is *not* there.
    negated: bool,
}

#[derive(Debug, Clone)]
enum Find {
    /// A plain string, found by a pattern made of it (for case folding),
    /// then checked at its edges.
    Text {
        pattern: Regex,
        word: bool,
        start: bool,
    },
    Pattern(Regex),
}

/// What is wrong with a rule: a code and arguments for a frontend to say
/// it in its language, and the English for one that does not know the
/// code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuleError {
    pub code: &'static str,
    pub args: Vec<(&'static str, String)>,
    pub message: String,
}

impl fmt::Display for RuleError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for RuleError {}

fn problem(code: &'static str, args: &[(&'static str, &str)], message: String) -> RuleError {
    RuleError {
        code,
        args: args.iter().map(|(k, v)| (*k, (*v).to_owned())).collect(),
        message,
    }
}

/// The three lists, read.
#[derive(Debug, Clone, Default)]
pub struct Rules {
    pub highlight: Vec<Rule>,
    pub ignore: Vec<Rule>,
    pub filter: Vec<Rule>,
}

/// Which list a rule came from, and where in it, for saying what is wrong
/// with it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListError {
    pub list: &'static str,
    /// From one, as a reader counts.
    pub number: usize,
    pub rule: String,
    pub error: RuleError,
}

impl fmt::Display for ListError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} rule {} ({}): {}",
            self.list, self.number, self.rule, self.error
        )
    }
}

impl Rules {
    /// Reads the lists as the settings keep them. Every rule is read and
    /// every problem reported, not just the first.
    pub fn read(
        highlight: &[String],
        ignore: &[String],
        filter: &[String],
    ) -> Result<Self, Vec<ListError>> {
        let mut errors = Vec::new();
        let mut read = |list: &'static str, rules: &[String]| -> Vec<Rule> {
            let mut out = Vec::new();
            for (i, source) in rules.iter().enumerate() {
                match Rule::parse(source) {
                    Ok(rule) => out.push(rule),
                    Err(error) => errors.push(ListError {
                        list,
                        number: i + 1,
                        rule: source.clone(),
                        error,
                    }),
                }
            }
            out
        };
        let rules = Rules {
            highlight: read("highlight", highlight),
            ignore: read("ignore", ignore),
            filter: read("filter", filter),
        };
        match errors.is_empty() {
            true => Ok(rules),
            false => Err(errors),
        }
    }

    /// Reads the lists, leaving out what cannot be read.
    pub fn read_valid(highlight: &[String], ignore: &[String], filter: &[String]) -> Self {
        let read = |rules: &[String]| -> Vec<Rule> {
            rules.iter().filter_map(|r| Rule::parse(r).ok()).collect()
        };
        Rules {
            highlight: read(highlight),
            ignore: read(ignore),
            filter: read(filter),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.highlight.is_empty() && self.ignore.is_empty() && self.filter.is_empty()
    }
}

/// A line as the rules see it: its text, who wrote it, where.
#[derive(Debug, Clone, Default)]
pub struct Subject {
    /// The channel's login.
    pub channel: String,
    /// A message somebody wrote, or an info line.
    pub info: bool,
    /// The whole text: for an info line, what it says and the message
    /// attached to it.
    pub text: String,
    /// What the person wrote, where there is such a thing: the whole of a
    /// chat message, the message attached to a subscription.
    pub message: Option<String>,
    /// Who wrote it, by login, when somebody did.
    pub sender: Option<String>,
    pub badges: Vec<(String, String)>,
    pub first_message: bool,
    pub historical: bool,
    pub channel_points: bool,
    pub link: bool,
    pub message_link: bool,
    pub restricted: bool,
    /// The channel a Shared Chat message came from.
    pub shared_from: Option<String>,
    pub live: bool,
    /// The sender's address book categories.
    pub categories: Vec<String>,
    /// The channel's, under `#channel`.
    pub channel_categories: Vec<String>,
    /// The channel's under `#channel` and under `channel` together.
    pub channel_categories_both: Vec<String>,
    /// How many times the sender has now said this, when the engine
    /// counts it as repeated; 0 otherwise.
    pub repeated: u32,
}

impl Subject {
    /// Describes a line in a channel. `first_message` and `live` are the
    /// engine's to know.
    pub fn of(line: &Line, channel: &str, first_message: bool, live: bool) -> Self {
        let mut subject =
            Subject {
                channel: channel.to_owned(),
                historical: line.flags.historical,
                restricted: line.flags.suspicion.as_ref().is_some_and(|s| {
                    s.status == liveside_proto::model::SuspicionStatus::Restricted
                }),
                first_message,
                live,
                ..Subject::default()
            };
        let has_link = |message: &liveside_proto::model::ChatMessage| {
            message
                .fragments
                .iter()
                .any(|f| matches!(f, liveside_proto::model::Fragment::Link { .. }))
        };
        match &line.content {
            LineContent::Message(message) => {
                let text = message.plain_text();
                subject.text = text.clone();
                subject.message = Some(text);
                subject.sender = Some(message.sender.login.to_string());
                subject.badges = badges(&message.badges);
                subject.channel_points = message.kind == MessageKind::ChannelPointsHighlighted;
                subject.link = has_link(message);
                subject.message_link = subject.link;
                subject.shared_from = message
                    .shared_source
                    .as_ref()
                    .map(|s| s.channel.login.to_string());
            }
            LineContent::UserNotice {
                system_text,
                message,
            } => {
                subject.info = true;
                subject.text = system_text.clone();
                if let Some(message) = message {
                    let said = message.plain_text();
                    if !said.is_empty() {
                        subject.text = format!("{system_text} {said}");
                    }
                    subject.message = Some(said);
                    subject.sender = Some(message.sender.login.to_string());
                    subject.badges = badges(&message.badges);
                    subject.message_link = has_link(message);
                    subject.link = subject.message_link;
                }
            }
            LineContent::Notice { text, .. } => {
                subject.info = true;
                subject.text = text.clone();
            }
        }
        subject
    }

    /// Fills in what the address book says about the sender and the
    /// channel.
    pub fn with_book(mut self, book: &crate::addressbook::AddressBook) -> Self {
        if book.is_empty() {
            return self;
        }
        if let Some(sender) = &self.sender {
            self.categories = book.categories_of(sender).to_vec();
        }
        let channel = format!("#{}", self.channel);
        self.channel_categories = book.categories_of(&channel).to_vec();
        self.channel_categories_both = self.channel_categories.clone();
        self.channel_categories_both
            .extend(book.categories_of(&self.channel).iter().cloned());
        self
    }
}

fn badges(badges: &[Badge]) -> Vec<(String, String)> {
    badges
        .iter()
        .map(|b| (b.set_id.clone(), b.id.clone()))
        .collect()
}

/// Meta prefixes this reads, and Chatty's that it refuses.
const SUPPORTED: &[&str] = &[
    "user",
    "!user",
    "reuser",
    "chan",
    "!chan",
    "cat",
    "!cat",
    "chanCat",
    "!chanCat",
    "chanCat2",
    "!chanCat2",
    "status",
    "!status",
    "config",
    "color",
    "bgcolor",
    "n",
    "blacklist",
    "replacement",
];

const UNSUPPORTED: &[&str] = &[
    "mystatus",
    "!mystatus",
    "msgs",
    "!msgs",
    "if",
    "!if",
    "to",
    "ncat",
    "preset",
    "cc",
    "cc2",
    "ccf",
];

/// How text can be looked for, as the prefix names it.
const TEXT_PREFIXES: &[&str] = &[
    "text", "cs", "w", "wcs", "start", "startw", "reg", "re*", "regi", "regw", "regwi", "regm",
    "re",
];

impl Rule {
    /// Reads one rule.
    pub fn parse(source: &str) -> Result<Rule, RuleError> {
        let mut rule = Rule {
            source: source.to_owned(),
            applies_to: AppliesTo::Chat,
            conditions: Vec::new(),
            main: None,
            extra: Vec::new(),
            blacklist: Vec::new(),
            style: Style::default(),
            replacement: "..".to_owned(),
        };
        let mut rest = source.trim();
        loop {
            rest = rest.trim_start();
            if rest.is_empty() {
                return Ok(rule);
            }
            // A further text condition: `+w:word`, `+!start:"!bet all"`.
            if let Some(after) = rest.strip_prefix('+') {
                let (token, next) = split_token(after)?;
                let found = text_match(&unquote(token)?, true)?;
                let Some(found) = found else {
                    return Err(problem(
                        "rule.plus_without_prefix",
                        &[("text", token)],
                        format!("“+{token}” needs a way to look, such as +w: or +start:"),
                    ));
                };
                rule.extra.push(found);
                rest = next;
                continue;
            }
            match meta_prefix(rest) {
                Some((prefix, _)) if UNSUPPORTED.contains(&prefix) => {
                    return Err(problem(
                        "rule.unsupported",
                        &[("prefix", prefix)],
                        format!("“{prefix}:” is Chatty's, and not supported here"),
                    ));
                }
                Some((prefix, value_start)) => {
                    let (token, next) = split_token(&rest[value_start..])?;
                    rule.meta(prefix, token)?;
                    rest = next;
                }
                // Not a prefix: the rest of the line is the text to find.
                None => {
                    let main = text_match(rest.trim_end(), false)?;
                    match main {
                        Some(main) if main.negated => rule.extra.push(main),
                        main => rule.main = main,
                    }
                    return Ok(rule);
                }
            }
        }
    }

    fn meta(&mut self, prefix: &str, raw: &str) -> Result<(), RuleError> {
        let value = unquote(raw)?;
        if value.is_empty() && prefix != "n" && prefix != "replacement" {
            return Err(problem(
                "rule.empty",
                &[("prefix", prefix)],
                format!("“{prefix}:” needs a value"),
            ));
        }
        let names = || -> Result<Vec<String>, RuleError> {
            Ok(split_list(raw)?
                .into_iter()
                .map(|name| name.trim_start_matches('#').to_lowercase())
                .filter(|name| !name.is_empty())
                .collect())
        };
        match prefix {
            "user" => self.conditions.push(Condition::User(value.to_lowercase())),
            "!user" => self
                .conditions
                .push(Condition::NotUser(value.to_lowercase())),
            "reuser" => self
                .conditions
                .push(Condition::UserPattern(regex(&format!("^(?:{value})$"))?)),
            "chan" => self.conditions.push(Condition::Channel(names()?)),
            "!chan" => self.conditions.push(Condition::NotChannel(names()?)),
            "cat" | "!cat" | "chanCat" | "!chanCat" | "chanCat2" | "!chanCat2" => {
                let categories = split_list(raw)?
                    .into_iter()
                    .map(|c| c.trim().to_lowercase())
                    .filter(|c| !c.is_empty())
                    .collect::<Vec<_>>();
                if let Some(bad) = categories.iter().find(|c| c.contains(char::is_whitespace)) {
                    return Err(problem(
                        "rule.category",
                        &[("category", bad)],
                        format!("“{bad}” is not a category: one word, without commas"),
                    ));
                }
                let both = prefix.ends_with('2');
                self.conditions.push(match prefix {
                    "cat" => Condition::Category(categories),
                    "!cat" => Condition::NotCategory(categories),
                    "chanCat" | "chanCat2" => Condition::ChannelCategory { categories, both },
                    _ => Condition::NotChannelCategory { categories, both },
                });
            }
            "status" | "!status" => {
                if let Some(code) = value.chars().find(|c| !"bLmMsvaftg".contains(*c)) {
                    let code = code.to_string();
                    return Err(problem(
                        "rule.status",
                        &[("code", &code)],
                        format!(
                            "“{code}” is not a status this knows; there are \
                             b, L, m, M, s, v, a, f, t and g (there is no bot list for r)"
                        ),
                    ));
                }
                self.conditions.push(match prefix {
                    "status" => Condition::Status(value),
                    _ => Condition::NotStatus(value),
                });
            }
            "config" => {
                for option in split_list(raw)? {
                    self.config(&option)?;
                }
            }
            "color" => self.style.color = Some(color(&value)?),
            "bgcolor" => self.style.background = Some(color(&value)?),
            "n" => {}
            "blacklist" => {
                for item in split_list(raw)? {
                    let found = match text_match(&item, true)? {
                        Some(found) => found,
                        None => text_match(&format!("text:{item}"), true)?
                            .expect("text: is a text prefix"),
                    };
                    self.blacklist.push(found);
                }
            }
            "replacement" => {
                self.replacement = match value.as_str() {
                    "none" => String::new(),
                    _ => value,
                }
            }
            _ => unreachable!("only known prefixes come here"),
        }
        Ok(())
    }

    fn config(&mut self, option: &str) -> Result<(), RuleError> {
        let (name, detail) = match option.split_once('|') {
            Some((name, detail)) => (name, Some(detail)),
            None => (option, None),
        };
        match (name, detail) {
            ("info", None) => self.applies_to = AppliesTo::Info,
            ("any", None) => self.applies_to = AppliesTo::Any,
            ("firstmsg", None) => self.conditions.push(Condition::FirstMessage),
            ("hl", None) => self.conditions.push(Condition::ChannelPoints),
            ("historic", None) => self.conditions.push(Condition::Historic),
            // Chatty's way to let history through features it keeps from
            // it; here every rule sees history anyway.
            ("historic2", None) => {}
            ("url", None) => self.conditions.push(Condition::Link {
                message_only: false,
            }),
            ("msgurl", None) => self.conditions.push(Condition::Link { message_only: true }),
            ("restricted", None) => self.conditions.push(Condition::Restricted),
            ("live", None) => self.conditions.push(Condition::Live(true)),
            ("repeatedmsg", None) => self.conditions.push(Condition::Repeated(1)),
            ("repeatedmsg", Some(times)) => match times.parse::<u32>() {
                Ok(times) => self.conditions.push(Condition::Repeated(times)),
                Err(_) => {
                    return Err(problem(
                        "rule.config",
                        &[("option", option)],
                        format!("“config:{option}” is not an option this knows"),
                    ));
                }
            },
            ("!live", None) => self.conditions.push(Condition::Live(false)),
            ("shared", channels) => self.conditions.push(Condition::Shared(
                channels
                    .map(|list| {
                        list.split('|')
                            .map(|c| c.trim_start_matches('#').to_lowercase())
                            .collect()
                    })
                    .unwrap_or_default(),
            )),
            ("b", Some(list)) => self.conditions.push(Condition::Badge(
                list.split('|')
                    .map(|badge| match badge.split_once('/') {
                        Some((set, version)) => (set.to_owned(), Some(version.to_owned())),
                        None => (badge.to_owned(), None),
                    })
                    .collect(),
            )),
            ("silent", None) => self.style.silent = true,
            ("!notify", None) => self.style.quiet = true,
            ("!ignore", None) => self.style.over_ignore = true,
            _ => {
                return Err(problem(
                    "rule.config",
                    &[("option", option)],
                    format!("“config:{option}” is not an option this knows"),
                ));
            }
        }
        Ok(())
    }

    /// Whether the rule matches, and if so where its main text was found,
    /// in the text it looked at (the message alone for a `msg` prefix).
    pub fn find(&self, subject: &Subject) -> Option<Vec<Range<usize>>> {
        let applies = match self.applies_to {
            AppliesTo::Chat => !subject.info,
            AppliesTo::Info => subject.info,
            AppliesTo::Any => true,
        };
        if !applies || !self.conditions.iter().all(|c| c.holds(subject)) {
            return None;
        }
        if !self.extra.iter().all(|t| t.holds(subject)) {
            return None;
        }
        let Some(main) = &self.main else {
            return Some(Vec::new());
        };
        let haystack = main.haystack(subject)?;
        let mut found = main.how.ranges(haystack);
        if !self.blacklist.is_empty() {
            let blocked: Vec<Range<usize>> = self
                .blacklist
                .iter()
                .filter_map(|b| b.haystack(subject).map(|h| b.how.ranges(h)))
                .flatten()
                .collect();
            found.retain(|r| !blocked.iter().any(|b| b.start <= r.start && r.end <= b.end));
        }
        (!found.is_empty()).then_some(found)
    }

    pub fn matches(&self, subject: &Subject) -> bool {
        self.find(subject).is_some()
    }

    /// Takes what the rule's main text finds out of `text`, putting the
    /// replacement in its place. `None` when there is nothing to take.
    pub fn filter_text(&self, text: &str) -> Option<String> {
        let main = self.main.as_ref()?;
        let found = main.how.ranges(text);
        if found.is_empty() {
            return None;
        }
        let mut out = String::with_capacity(text.len());
        let mut at = 0;
        for range in found {
            if range.start < at {
                continue;
            }
            out.push_str(&text[at..range.start]);
            out.push_str(&self.replacement);
            at = range.end;
        }
        out.push_str(&text[at..]);
        Some(out)
    }
}

impl Condition {
    fn holds(&self, s: &Subject) -> bool {
        let sender = s.sender.as_deref();
        match self {
            Condition::User(name) => sender == Some(name.as_str()),
            Condition::NotUser(name) => sender != Some(name.as_str()),
            Condition::UserPattern(pattern) => sender.is_some_and(|login| pattern.is_match(login)),
            Condition::Channel(names) => names.iter().any(|n| n == &s.channel),
            Condition::NotChannel(names) => !names.iter().any(|n| n == &s.channel),
            Condition::Category(wanted) => wanted.iter().any(|c| s.categories.contains(c)),
            Condition::NotCategory(wanted) => wanted.iter().any(|c| !s.categories.contains(c)),
            Condition::ChannelCategory { categories, both } => {
                let has = if *both {
                    &s.channel_categories_both
                } else {
                    &s.channel_categories
                };
                categories.iter().any(|c| has.contains(c))
            }
            Condition::NotChannelCategory { categories, both } => {
                let has = if *both {
                    &s.channel_categories_both
                } else {
                    &s.channel_categories
                };
                categories.iter().any(|c| !has.contains(c))
            }
            Condition::Status(codes) => codes.chars().any(|code| has_status(s, code)),
            Condition::NotStatus(codes) => !codes.chars().any(|code| has_status(s, code)),
            Condition::FirstMessage => s.first_message,
            Condition::ChannelPoints => s.channel_points,
            Condition::Historic => s.historical,
            Condition::Link { message_only } => match message_only {
                true => s.message_link,
                false => s.link,
            },
            Condition::Badge(wanted) => wanted.iter().any(|(set, version)| {
                s.badges
                    .iter()
                    .any(|(has, id)| has == set && version.as_ref().is_none_or(|v| v == id))
            }),
            Condition::Restricted => s.restricted,
            Condition::Shared(channels) => s
                .shared_from
                .as_ref()
                .is_some_and(|from| channels.is_empty() || channels.contains(from)),
            Condition::Live(live) => s.live == *live,
            Condition::Repeated(times) => s.repeated > 0 && s.repeated >= *times,
        }
    }
}

/// Whether the sender has a status, by Chatty's letters for them.
fn has_status(s: &Subject, code: char) -> bool {
    let has = |set: &str| s.badges.iter().any(|(b, _)| b == set);
    match code {
        'b' => has("broadcaster"),
        'L' => has("lead_moderator"),
        'm' => has("moderator") || has("lead_moderator"),
        'M' => {
            has("broadcaster")
                || has("lead_moderator")
                || has("moderator")
                || has("admin")
                || has("staff")
                || has("global_mod")
        }
        's' => has("subscriber") || has("founder"),
        'v' => has("vip"),
        'a' => has("admin"),
        'f' => has("staff"),
        't' => has("turbo"),
        'g' => has("global_mod"),
        _ => false,
    }
}

impl TextMatch {
    fn haystack<'s>(&self, s: &'s Subject) -> Option<&'s str> {
        match self.message_only {
            true => s.message.as_deref(),
            false => Some(&s.text),
        }
    }

    fn holds(&self, s: &Subject) -> bool {
        // Nothing to look in is no match either way, negated or not: a
        // subscription without a message has no message to lack a word.
        let Some(haystack) = self.haystack(s) else {
            return false;
        };
        self.how.ranges(haystack).is_empty() == self.negated
    }
}

impl Find {
    fn ranges(&self, text: &str) -> Vec<Range<usize>> {
        match self {
            Find::Pattern(pattern) => pattern
                .find_iter(text)
                .filter(|m| !m.is_empty())
                .map(|m| m.range())
                .collect(),
            Find::Text {
                pattern,
                word,
                start,
            } => pattern
                .find_iter(text)
                .map(|m| m.range())
                .filter(|r| !*start || r.start == 0)
                .filter(|r| {
                    !*word
                        || (text[..r.start]
                            .chars()
                            .next_back()
                            .is_none_or(|c| !is_word(c))
                            && text[r.end..].chars().next().is_none_or(|c| !is_word(c)))
                })
                .collect(),
        }
    }
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// A known meta prefix at the start of `rest`, and where its value
/// begins.
fn meta_prefix(rest: &str) -> Option<(&'static str, usize)> {
    let colon = rest.find(':')?;
    let name = &rest[..colon];
    if name.contains(char::is_whitespace) {
        return None;
    }
    SUPPORTED
        .iter()
        .chain(UNSUPPORTED)
        .find(|known| **known == name)
        .map(|known| (*known, colon + 1))
}

/// Reads a text condition: `[!][msg]prefix:text`, or, when `required` is
/// false, plain text found anywhere regardless of case. `None` for an
/// empty main text, which leaves the rule to its other conditions.
fn text_match(written: &str, required: bool) -> Result<Option<TextMatch>, RuleError> {
    let (negated, written) = match written.strip_prefix('!') {
        // "!" alone, or "!bet" without a prefix, is text to find.
        Some(after) if prefix_of(after).is_some() => (true, after),
        _ => (false, written),
    };
    let (message_only, prefix, text) = match prefix_of(written) {
        Some((message_only, prefix, text)) => (message_only, prefix, text),
        None if required => return Ok(None),
        None if written.is_empty() => return Ok(None),
        None => (false, "text", written),
    };
    if text.is_empty() {
        return Err(problem(
            "rule.empty",
            &[("prefix", prefix)],
            format!("“{prefix}:” needs something to look for"),
        ));
    }
    let literal = |case_sensitive: bool, word: bool, start: bool| -> Result<Find, RuleError> {
        let flags = if case_sensitive { "" } else { "(?iu)" };
        Ok(Find::Text {
            pattern: regex(&format!("{flags}{}", regex::escape(text)))?,
            word,
            start,
        })
    };
    let how = match prefix {
        "text" => literal(false, false, false)?,
        "cs" => literal(true, false, false)?,
        "w" => literal(false, true, false)?,
        "wcs" => literal(true, true, false)?,
        "start" => literal(false, false, true)?,
        // Chatty's start-and-word: the start, and a word's end after it.
        "startw" => literal(false, true, true)?,
        "reg" | "re*" => Find::Pattern(regex(text)?),
        "regi" => Find::Pattern(regex(&format!("(?iu){text}"))?),
        "regw" => Find::Pattern(regex(&format!(r"\b(?:{text})\b"))?),
        "regwi" => Find::Pattern(regex(&format!(r"(?iu)\b(?:{text})\b"))?),
        "regm" | "re" => Find::Pattern(regex(&format!("^(?:{text})$"))?),
        _ => unreachable!("only text prefixes come here"),
    };
    Ok(Some(TextMatch {
        how,
        message_only,
        negated,
    }))
}

/// `[msg]prefix:text` split up, when it starts with a text prefix.
fn prefix_of(written: &str) -> Option<(bool, &'static str, &str)> {
    let colon = written.find(':')?;
    let name = &written[..colon];
    let (message_only, name) = match name.strip_prefix("msg") {
        Some(rest) if TEXT_PREFIXES.contains(&rest) => (true, rest),
        _ => (false, name),
    };
    let prefix = TEXT_PREFIXES.iter().find(|p| **p == name)?;
    Some((message_only, prefix, &written[colon + 1..]))
}

fn regex(pattern: &str) -> Result<Regex, RuleError> {
    Regex::new(pattern).map_err(|e| {
        // The parser's own message is several lines with a caret under
        // the place; the last line says what is wrong.
        let why = e
            .to_string()
            .lines()
            .rev()
            .find(|l| !l.trim().is_empty())
            .unwrap_or_default()
            .trim()
            .trim_start_matches("error: ")
            .to_owned();
        problem(
            "rule.regex",
            &[("error", &why)],
            format!("the regular expression does not read: {why}"),
        )
    })
}

/// A colour by `#rrggbb`, `#rgb` or one of the common names.
fn color(value: &str) -> Result<Rgb, RuleError> {
    let hex = value.strip_prefix('#').unwrap_or(value);
    if hex.len() == 3 && hex.chars().all(|c| c.is_ascii_hexdigit()) {
        let doubled: String = hex.chars().flat_map(|c| [c, c]).collect();
        if let Some(rgb) = Rgb::parse_hex(&doubled) {
            return Ok(rgb);
        }
    }
    if let Some(rgb) = Rgb::parse_hex(value) {
        return Ok(rgb);
    }
    let named = match value.to_lowercase().as_str() {
        "black" => "000000",
        "white" => "ffffff",
        "red" => "ff0000",
        "green" => "008000",
        "lime" => "00ff00",
        "blue" => "0000ff",
        "yellow" => "ffff00",
        "orange" => "ffa500",
        "purple" => "800080",
        "magenta" | "fuchsia" => "ff00ff",
        "cyan" | "aqua" => "00ffff",
        "pink" => "ffc0cb",
        "brown" => "a52a2a",
        "gray" | "grey" => "808080",
        "silver" => "c0c0c0",
        "maroon" => "800000",
        "navy" => "000080",
        "olive" => "808000",
        "teal" => "008080",
        _ => "",
    };
    Rgb::parse_hex(named).ok_or_else(|| {
        problem(
            "rule.color",
            &[("value", value)],
            format!("“{value}” is not a colour: write #rrggbb or a name such as orange"),
        )
    })
}

/// The next value, up to a space outside quotes, and what follows it.
fn split_token(rest: &str) -> Result<(&str, &str), RuleError> {
    let mut quoted = false;
    for (i, c) in rest.char_indices() {
        match c {
            '"' => quoted = !quoted,
            c if c.is_whitespace() && !quoted => return Ok((&rest[..i], &rest[i..])),
            _ => {}
        }
    }
    if quoted {
        return Err(problem(
            "rule.quote",
            &[],
            "a quote is opened and not closed".to_owned(),
        ));
    }
    Ok((rest, ""))
}

/// A value with its quotes taken off: quotes keep spaces and commas in,
/// and two quotes inside them stand for one.
fn unquote(raw: &str) -> Result<String, RuleError> {
    let mut out = String::with_capacity(raw.len());
    let mut quoted = false;
    let mut chars = raw.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' if quoted && chars.peek() == Some(&'"') => {
                chars.next();
                out.push('"');
            }
            '"' => quoted = !quoted,
            c => out.push(c),
        }
    }
    if quoted {
        return Err(problem(
            "rule.quote",
            &[],
            "a quote is opened and not closed".to_owned(),
        ));
    }
    Ok(out)
}

/// A comma-separated value, each item unquoted; a comma in quotes is
/// part of its item.
fn split_list(raw: &str) -> Result<Vec<String>, RuleError> {
    let mut items = Vec::new();
    let mut quoted = false;
    let mut start = 0;
    for (i, c) in raw.char_indices() {
        match c {
            '"' => quoted = !quoted,
            ',' if !quoted => {
                items.push(unquote(&raw[start..i])?);
                start = i + 1;
            }
            _ => {}
        }
    }
    items.push(unquote(&raw[start..])?);
    Ok(items.into_iter().filter(|item| !item.is_empty()).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chat(channel: &str, sender: &str, text: &str) -> Subject {
        Subject {
            channel: channel.into(),
            text: text.into(),
            message: Some(text.into()),
            sender: Some(sender.into()),
            ..Subject::default()
        }
    }

    fn rule(source: &str) -> Rule {
        Rule::parse(source).unwrap_or_else(|e| panic!("{source}: {e}"))
    }

    fn hits(source: &str, s: &Subject) -> bool {
        rule(source).matches(s)
    }

    #[test]
    fn plain_text_is_found_anywhere_regardless_of_case() {
        let s = chat("c", "a", "I like Rustaceans");
        assert!(hits("rust", &s));
        assert!(hits("text:RUST", &s));
        assert!(!hits("cs:rust", &s));
        assert!(hits("cs:Rust", &s));
        assert!(!hits("python", &s));
    }

    #[test]
    fn words_and_starts_mind_their_edges() {
        let s = chat("c", "a", "Anna: Hannah says hi");
        assert!(hits("w:anna", &s));
        assert!(!hits("w:hann", &s));
        assert!(!hits("wcs:anna", &s));
        assert!(hits("start:ann", &s));
        assert!(!hits("startw:ann", &s));
        assert!(hits("startw:anna", &s));
        assert!(!hits("start:hannah", &s));
        // A word may start with something that is not a letter.
        assert!(hits("startw:!bet", &chat("c", "a", "!bet 500")));
        assert!(!hits("startw:!bet", &chat("c", "a", "!betting")));
        assert!(hits("w:ěščř", &chat("c", "a", "to je ĚŠČŘ!")));
    }

    #[test]
    fn patterns_in_chattys_variants() {
        let s = chat("c", "a", "playing GTA 5 today");
        assert!(hits(r"regi:\bgta ?[v5]\b", &s));
        assert!(!hits(r"reg:\bgta ?[v5]\b", &s));
        assert!(hits("regwi:gta ?[v5]", &s));
        assert!(!hits("regwi:gt", &s));
        assert!(hits("regw:(?i)GTA ?[V5]", &s));
        assert!(!hits("regm:GTA 5", &s));
        assert!(hits("re:playing.*", &s));
        assert!(hits("re*:GTA", &s));
    }

    #[test]
    fn who_and_where() {
        let s = chat("harborlight", "nightbot", "hello");
        assert!(hits("user:NightBot", &s));
        assert!(!hits("!user:nightbot", &s));
        assert!(hits("reuser:night.*", &s));
        assert!(!hits("reuser:night", &s));
        assert!(hits("chan:other,#harborlight hello", &s));
        assert!(!hits("!chan:harborlight", &s));
        assert!(!hits("chan:other", &s));
        // All conditions hold, or the rule does not match.
        assert!(!hits("user:nightbot chan:other", &s));
        // A rule of conditions alone matches whatever they let through.
        assert!(hits("user:nightbot", &s));
        assert!(hits("", &s));
    }

    #[test]
    fn statuses_by_chattys_letters() {
        let mut s = chat("c", "a", "hi");
        s.badges = vec![("subscriber".into(), "12".into())];
        assert!(hits("status:s", &s));
        assert!(!hits("status:mv status:s", &s));
        assert!(hits("status:vs", &s));
        assert!(!hits("!status:smv", &s));
        assert!(hits("!status:M", &s));
        s.badges = vec![("lead_moderator".into(), "1".into())];
        assert!(hits("status:m", &s));
        assert!(hits("status:M", &s));
        assert!(hits("status:L", &s));
        assert_eq!(
            Rule::parse("status:r").unwrap_err().code,
            "rule.status",
            "there is no bot list to know r by"
        );
    }

    #[test]
    fn negation_and_further_conditions() {
        let s = chat("c", "a", "!bet all on red");
        assert!(hits("start:!bet", &s));
        assert!(!hits("+!start:\"!bet all\" start:!bet", &s));
        assert!(hits(
            "+!start:\"!bet all\" start:!bet",
            &chat("c", "a", "!bet 50")
        ));
        // A negated main text matches where the text is not.
        assert!(hits("!w:spam", &s));
        assert!(!hits("!w:red", &s));
        // "!" with no prefix after it is just text.
        assert!(hits("!bet", &s));
        assert!(hits("+w:red +w:all", &s));
        assert!(!hits("+w:red +w:blue", &s));
    }

    #[test]
    fn a_blacklisted_match_does_not_count() {
        assert!(!hits(
            "blacklist:\"!bet all\" start:!bet",
            &chat("c", "a", "!bet all")
        ));
        assert!(hits(
            "blacklist:\"!bet all\" start:!bet",
            &chat("c", "a", "!bet 5")
        ));
        // Only a match it swallows whole: elsewhere it does not matter.
        assert!(hits(
            r"blacklist:!cheesecake reg:^!\w+",
            &chat("c", "a", "!cake !cheesecake")
        ));
        assert!(!hits(
            r"blacklist:!cheesecake reg:^!\w+",
            &chat("c", "a", "!cheesecake")
        ));
    }

    #[test]
    fn info_lines_only_when_asked_and_msg_looks_at_what_was_written() {
        let info = Subject {
            channel: "c".into(),
            info: true,
            text: "alice subscribed for 3 months great stream".into(),
            message: Some("great stream".into()),
            sender: Some("alice".into()),
            ..Subject::default()
        };
        assert!(!hits("subscribed", &info));
        assert!(hits("config:info subscribed", &info));
        assert!(hits("config:any subscribed", &info));
        assert!(!hits("config:info msgtext:subscribed", &info));
        assert!(hits("config:info msgw:great", &info));
        assert!(!hits("config:info", &chat("c", "a", "x")));
        let without = Subject {
            message: None,
            ..info.clone()
        };
        assert!(!hits("config:info msgtext:great", &without));
        assert!(!hits("config:info !msgtext:great", &without));
    }

    #[test]
    fn config_conditions() {
        let mut s = chat("c", "a", "look https://example.com");
        s.link = true;
        s.message_link = true;
        assert!(hits("config:url", &s));
        assert!(!hits("config:firstmsg", &s));
        s.first_message = true;
        assert!(hits("config:firstmsg", &s));
        assert!(!hits("config:historic", &s));
        assert!(!hits("config:live", &s));
        assert!(hits("config:!live", &s));
        s.badges = vec![("vip".into(), "1".into())];
        assert!(hits("config:b|vip", &s));
        assert!(hits("config:b|moderator/1|vip/1", &s));
        assert!(!hits("config:b|vip/2", &s));
        s.shared_from = Some("other".into());
        assert!(hits("config:shared", &s));
        assert!(hits("config:shared|other|third", &s));
        assert!(!hits("config:shared|third", &s));
    }

    #[test]
    fn what_a_rule_does_besides_matching() {
        let r = rule("color:orange bgcolor:#123 config:silent,!notify,!ignore n:\"a note\" w:x");
        assert_eq!(
            r.style,
            Style {
                color: Some(Rgb {
                    r: 0xff,
                    g: 0xa5,
                    b: 0
                }),
                background: Some(Rgb {
                    r: 0x11,
                    g: 0x22,
                    b: 0x33
                }),
                silent: true,
                quiet: true,
                over_ignore: true,
            }
        );
        assert!(r.matches(&chat("c", "a", "x y")));
    }

    #[test]
    fn a_filter_takes_out_what_it_finds() {
        let r = rule(r"regw:a\w*");
        assert_eq!(
            r.filter_text("an apple and a pear").as_deref(),
            Some(".. .. .. .. pear")
        );
        let r = rule("replacement:[link] reg:https?://\\S+");
        assert_eq!(
            r.filter_text("see https://x.y/z now").as_deref(),
            Some("see [link] now")
        );
        assert_eq!(
            rule("replacement:none w:um")
                .filter_text("so um yes")
                .as_deref(),
            Some("so  yes")
        );
        assert_eq!(rule("w:um").filter_text("summary"), None);
        assert_eq!(rule("user:bot").filter_text("anything"), None);
    }

    #[test]
    fn quotes_keep_spaces_and_commas() {
        let r = rule("chan:\"a,b\",c start:x");
        assert!(r.matches(&chat("a,b", "u", "xy")));
        assert!(r.matches(&chat("c", "u", "xy")));
        assert!(!r.matches(&chat("a", "u", "xy")));
        assert_eq!(
            rule("replacement:\"two \"\"words\"\"\" w:x").replacement,
            "two \"words\""
        );
    }

    #[test]
    fn repeated_messages_match_from_how_many_times() {
        let mut said = chat("c", "u", "buy followers");
        assert!(!rule("config:repeatedmsg").matches(&said));
        said.repeated = 3;
        assert!(rule("config:repeatedmsg").matches(&said));
        assert!(rule("config:repeatedmsg|3").matches(&said));
        assert!(!rule("config:repeatedmsg|4").matches(&said));
    }

    #[test]
    fn categories_come_from_the_address_book() {
        let book = crate::addressbook::AddressBook::from_lines(&[
            "alice friends,vip".to_owned(),
            "bob family".to_owned(),
            "#harborlight modding,important".to_owned(),
            "other modding".to_owned(),
        ])
        .unwrap();
        let said = |channel: &str, user: &str| chat(channel, user, "hi").with_book(&book);
        let r = rule("cat:friends,family");
        assert!(r.matches(&said("c", "alice")));
        assert!(r.matches(&said("c", "bob")));
        assert!(!r.matches(&said("c", "carol")));
        // Chatty's !cat: fails only for someone in all of the list.
        let r = rule("!cat:friends,vip");
        assert!(!r.matches(&said("c", "alice")));
        assert!(r.matches(&said("c", "bob")));
        assert!(!rule("!cat:friends !cat:family").matches(&said("c", "alice")));
        // Channels go by their #name; chanCat2: also by the plain one.
        assert!(rule("chanCat:modding").matches(&said("harborlight", "x")));
        assert!(!rule("chanCat:modding").matches(&said("other", "x")));
        assert!(rule("chanCat2:modding").matches(&said("other", "x")));
        assert!(!rule("!chanCat:modding,important").matches(&said("harborlight", "x")));
        assert!(rule("!chanCat:modding,family").matches(&said("harborlight", "x")));
        // Nobody in the book is in no category.
        assert!(!rule("cat:friends").matches(&chat("c", "alice", "hi")));
    }

    #[test]
    fn what_cannot_be_read_says_why() {
        let code = |source: &str| Rule::parse(source).unwrap_err().code;
        assert_eq!(code("msgs:x hi"), "rule.unsupported");
        assert_eq!(code("if:\"user:a\",\"user:b\""), "rule.unsupported");
        assert_eq!(code(r"reg:(?<=a)b"), "rule.regex");
        assert_eq!(code(r"reg:(a)\1"), "rule.regex");
        assert_eq!(code("color:notacolour x"), "rule.color");
        assert_eq!(code("config:repeatedmsg|often"), "rule.config");
        assert_eq!(code("config:afterban"), "rule.config");
        assert_eq!(code("user: x"), "rule.empty");
        assert_eq!(code("w:"), "rule.empty");
        assert_eq!(code("chan:\"open x"), "rule.quote");
        assert_eq!(code("+word"), "rule.plus_without_prefix");
        // A word with a colon that is not a prefix is text to find.
        assert!(hits("http://x", &chat("c", "a", "go to http://x now")));
        assert!(hits("note: hi", &chat("c", "a", "a note: hi")));
    }

    #[test]
    fn every_problem_in_the_lists_is_reported_with_its_place() {
        let errors = Rules::read(
            &["w:ok".into(), "msgs:x".into()],
            &["user:a".into()],
            &["reg:(".into()],
        )
        .unwrap_err();
        let places: Vec<(&str, usize)> = errors.iter().map(|e| (e.list, e.number)).collect();
        assert_eq!(places, [("highlight", 2), ("filter", 1)]);
        assert!(Rules::read(&[], &[], &[]).unwrap().is_empty());
    }
}
