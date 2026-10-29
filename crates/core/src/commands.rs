//! Custom commands: a name for something one would otherwise type, with
//! the words after it put in. In Chatty's form, so a list kept there
//! carries over:
//!
//! ```text
//! /slap /me slaps $$1 around a bit with a large trout
//! /hi#harborlight Ahoj $1, vítej u Harborlight!
//! /info /echo $(streamtitle) · $(streamviewers) watching for $(streamuptime)
//! ```
//!
//! `$1`…`$9` are the words after the command, `$2-` the second and all
//! after it, `$(10)` the same in long form, and a doubled `$$` makes one
//! required: without it the command is not run, and the reader is told
//! why. `$(chan)`, `$(chans)`, `$(my-nick)` and the `stream…` values come
//! from where the command is typed; `$if`, `$ifeq`, `$lower`, `$upper`,
//! `$trim`, `$join`, `$rand` and `$randnum` are functions. `\$` and
//! `$"text"` are literal.
//!
//! Written from Chatty's help, not from its code. What it has and this
//! does not (the other functions, Chatty's context-menu identifiers, the
//! `//` anonymous commands, which here send a line starting with `/`) is
//! refused when a command is read, not quietly turned into something else.
//! A custom command's result is not expanded again, as in Chatty, so one
//! cannot call another and none can loop.

use std::fmt;

/// What is wrong with a command, with a code and arguments for a frontend
/// to say it in its language.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandProblem {
    pub code: &'static str,
    pub args: Vec<(&'static str, String)>,
    pub message: String,
}

impl fmt::Display for CommandProblem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

fn problem(code: &'static str, args: &[(&'static str, &str)], message: String) -> CommandProblem {
    CommandProblem {
        code,
        args: args.iter().map(|(k, v)| (*k, (*v).to_owned())).collect(),
        message,
    }
}

/// The values a command may name, besides the numbered words.
const NAMED: &[&str] = &[
    "chan",
    "stream",
    "chans",
    "my-nick",
    "streamviewers",
    "streamuptime",
    "streamtitle",
    "streamgame",
    "streamstatus",
];

/// Functions, by name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Function {
    If,
    IfEq,
    Lower,
    Upper,
    Trim,
    Join,
    Rand,
    RandNum,
}

impl Function {
    fn named(name: &str) -> Option<Self> {
        Some(match name {
            "if" => Function::If,
            "ifeq" => Function::IfEq,
            "lower" => Function::Lower,
            "upper" => Function::Upper,
            "trim" => Function::Trim,
            "join" => Function::Join,
            "rand" => Function::Rand,
            "randnum" => Function::RandNum,
            _ => return None,
        })
    }

    /// How many arguments it takes, at least and at most.
    fn arity(self) -> (usize, usize) {
        match self {
            Function::If => (2, 3),
            Function::IfEq => (3, 4),
            Function::Lower | Function::Upper | Function::Trim => (1, 1),
            Function::Join => (2, 2),
            Function::Rand => (1, usize::MAX),
            Function::RandNum => (1, 2),
        }
    }

    /// Whether its first argument is an identifier, as Chatty reads it:
    /// a bare `1-` or `chan` rather than text.
    fn takes_identifier(self) -> bool {
        !matches!(self, Function::Rand | Function::RandNum)
    }
}

/// What a value is taken from.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Id {
    /// A word after the command, from one; with the rest of them.
    Word {
        index: usize,
        rest: bool,
    },
    Named(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Part {
    Text(String),
    Value {
        id: Id,
        required: bool,
    },
    Call {
        function: Function,
        args: Vec<Arg>,
        required: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Arg {
    Id(Id),
    Parts(Vec<Part>),
}

/// One command, read.
#[derive(Debug, Clone)]
pub struct CustomCommand {
    /// Without the slash, lowercase.
    pub name: String,
    /// Only in this channel, when set.
    pub channel: Option<String>,
    /// The line as written.
    pub source: String,
    body: Vec<Part>,
}

/// Where a command is run: what its named values are.
#[derive(Debug, Clone, Default)]
pub struct Context {
    /// The channel's login, in a channel.
    pub channel: Option<String>,
    /// Every joined channel's login.
    pub channels: Vec<String>,
    /// The account's login.
    pub me: Option<String>,
    pub viewers: Option<u32>,
    /// How long the stream has run, as `1:15`.
    pub uptime: Option<String>,
    pub title: Option<String>,
    pub game: Option<String>,
}

/// The list, read.
#[derive(Debug, Clone, Default)]
pub struct CustomCommands {
    list: Vec<CustomCommand>,
}

/// Which line of the list is wrong, and how.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineProblem {
    /// From one.
    pub number: usize,
    pub line: String,
    pub problem: CommandProblem,
}

impl fmt::Display for LineProblem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "custom command {} ({}): {}",
            self.number, self.line, self.problem
        )
    }
}

impl CustomCommands {
    /// Reads the list; every problem is reported. Lines starting with `#`
    /// and empty ones are left out.
    pub fn read(lines: &[String]) -> Result<Self, Vec<LineProblem>> {
        let mut list = Vec::new();
        let mut problems = Vec::new();
        for (i, line) in lines.iter().enumerate() {
            match CustomCommand::parse(line) {
                Ok(Some(command)) => list.push(command),
                Ok(None) => {}
                Err(problem) => problems.push(LineProblem {
                    number: i + 1,
                    line: line.clone(),
                    problem,
                }),
            }
        }
        match problems.is_empty() {
            true => Ok(CustomCommands { list }),
            false => Err(problems),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.list.is_empty()
    }

    /// The names, for completion.
    pub fn names(&self) -> Vec<&str> {
        let mut names: Vec<&str> = self.list.iter().map(|c| c.name.as_str()).collect();
        names.sort_unstable();
        names.dedup();
        names
    }

    /// What a typed line turns into, when it is a custom command: `None`
    /// when it is not one, and a problem when a required value is
    /// missing. A command for this channel wins over one for any.
    pub fn expand(&self, typed: &str, context: &Context) -> Option<Result<String, CommandProblem>> {
        let rest = typed.trim_start().strip_prefix('/')?;
        if rest.starts_with('/') {
            return None;
        }
        let (name, args) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
        let name = name.to_lowercase();
        let here = context.channel.as_deref();
        let command = self
            .list
            .iter()
            .find(|c| c.name == name && c.channel.is_some() && c.channel.as_deref() == here)
            .or_else(|| {
                self.list
                    .iter()
                    .find(|c| c.name == name && c.channel.is_none())
            })?;
        Some(command.run(args, context))
    }
}

impl CustomCommand {
    /// Reads one line: `None` for a comment or nothing.
    pub fn parse(line: &str) -> Result<Option<Self>, CommandProblem> {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            return Ok(None);
        }
        let Some(rest) = line.strip_prefix('/') else {
            return Err(problem(
                "custom.name",
                &[("line", line)],
                format!("“{line}” has to start with /name"),
            ));
        };
        let (head, body) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
        let (name, channel) = match head.split_once('#') {
            Some((name, channel)) => (name, Some(channel.to_lowercase())),
            None => (head, None),
        };
        let name = name.to_lowercase();
        if name.is_empty()
            || !name
                .chars()
                .all(|c| c.is_alphanumeric() || c == '_' || c == '-')
        {
            return Err(problem(
                "custom.name",
                &[("line", line)],
                format!("“{line}” has to start with /name"),
            ));
        }
        let mut reader = Reader {
            chars: body.trim().chars().collect(),
            at: 0,
        };
        let body = reader.parts(&[])?;
        Ok(Some(CustomCommand {
            name,
            channel: channel.filter(|c| !c.is_empty()),
            source: line.to_owned(),
            body,
        }))
    }

    /// Runs it with the words typed after it.
    pub fn run(&self, args: &str, context: &Context) -> Result<String, CommandProblem> {
        let words: Vec<&str> = args.split_whitespace().collect();
        let values = Values {
            words: &words,
            context,
            command: &self.name,
        };
        let mut seed = seed();
        let out = values.parts(&self.body, &mut seed)?;
        Ok(out.trim().to_owned())
    }
}

/// Reads the text of a command.
struct Reader {
    chars: Vec<char>,
    at: usize,
}

impl Reader {
    fn peek(&self) -> Option<char> {
        self.chars.get(self.at).copied()
    }

    fn next(&mut self) -> Option<char> {
        let c = self.peek();
        self.at += usize::from(c.is_some());
        c
    }

    /// Text and replacements up to one of `stops` (not taken) or the end.
    fn parts(&mut self, stops: &[char]) -> Result<Vec<Part>, CommandProblem> {
        let mut parts = Vec::new();
        let mut text = String::new();
        while let Some(c) = self.peek() {
            if stops.contains(&c) {
                break;
            }
            self.at += 1;
            match c {
                '\\' => match self.next() {
                    Some(escaped) => text.push(escaped),
                    None => text.push('\\'),
                },
                '$' => {
                    if let Some(literal) = self.literal()? {
                        text.push_str(&literal);
                        continue;
                    }
                    if !text.is_empty() {
                        parts.push(Part::Text(std::mem::take(&mut text)));
                    }
                    parts.push(self.replacement()?);
                }
                c => text.push(c),
            }
        }
        if !text.is_empty() {
            parts.push(Part::Text(text));
        }
        Ok(parts)
    }

    /// `$"text"`, `` $`text` `` or `$'text'`, after the `$`.
    fn literal(&mut self) -> Result<Option<String>, CommandProblem> {
        let Some(quote) = self.peek().filter(|c| matches!(c, '"' | '`' | '\'')) else {
            return Ok(None);
        };
        self.at += 1;
        let mut out = String::new();
        loop {
            match self.next() {
                None => {
                    return Err(problem(
                        "custom.quote",
                        &[],
                        "a quote is opened and not closed".to_owned(),
                    ));
                }
                // Doubled, the quote stands for itself.
                Some(c) if c == quote && self.peek() == Some(quote) => {
                    self.at += 1;
                    out.push(quote);
                }
                Some(c) if c == quote => return Ok(Some(out)),
                Some(c) => out.push(c),
            }
        }
    }

    /// What follows a `$`: `$1`, `$1-`, `$(id)`, `$name(…)`, each maybe
    /// after a second `$` that makes it required.
    fn replacement(&mut self) -> Result<Part, CommandProblem> {
        let required = self.peek() == Some('$');
        if required {
            self.at += 1;
        }
        match self.peek() {
            Some(c @ '1'..='9') => {
                self.at += 1;
                let rest = self.peek() == Some('-');
                if rest {
                    self.at += 1;
                }
                Ok(Part::Value {
                    id: Id::Word {
                        index: c.to_digit(10).unwrap_or(1) as usize,
                        rest,
                    },
                    required,
                })
            }
            Some('(') => {
                self.at += 1;
                let id = self.until_close()?;
                Ok(Part::Value {
                    id: identifier(id.trim())?,
                    required,
                })
            }
            Some(c) if c.is_ascii_alphabetic() => {
                let start = self.at;
                while self.peek().is_some_and(|c| c.is_ascii_alphanumeric()) {
                    self.at += 1;
                }
                let name: String = self.chars[start..self.at].iter().collect();
                let Some(function) = Function::named(&name.to_lowercase()) else {
                    return Err(problem(
                        "custom.function",
                        &[("name", &name)],
                        format!("“${name}” is not a function this knows"),
                    ));
                };
                if self.next() != Some('(') {
                    return Err(problem(
                        "custom.syntax",
                        &[("detail", &format!("${name}"))],
                        format!("“${name}” needs its arguments in brackets"),
                    ));
                }
                let args = self.arguments(function)?;
                let (least, most) = function.arity();
                if args.len() < least || args.len() > most {
                    return Err(problem(
                        "custom.syntax",
                        &[("detail", &format!("${name}(…)"))],
                        format!("“${name}(…)” has the wrong number of arguments"),
                    ));
                }
                Ok(Part::Call {
                    function,
                    args,
                    required,
                })
            }
            _ => {
                let shown: String = self.chars[self.at.saturating_sub(1)..]
                    .iter()
                    .take(6)
                    .collect();
                Err(problem(
                    "custom.syntax",
                    &[("detail", &shown)],
                    format!("“{shown}” is not a replacement; write \\$ for a dollar"),
                ))
            }
        }
    }

    /// The text up to the closing bracket, taken.
    fn until_close(&mut self) -> Result<String, CommandProblem> {
        let mut out = String::new();
        loop {
            match self.next() {
                Some(')') => return Ok(out),
                Some(c) => out.push(c),
                None => {
                    return Err(problem(
                        "custom.bracket",
                        &[],
                        "a bracket is opened and not closed".to_owned(),
                    ));
                }
            }
        }
    }

    /// A function's arguments, after its `(`, up to and taking its `)`.
    fn arguments(&mut self, function: Function) -> Result<Vec<Arg>, CommandProblem> {
        let mut args = Vec::new();
        loop {
            let first = args.is_empty() && function.takes_identifier();
            // An identifier unless it starts with `$`, as in Chatty.
            if first && self.peek() != Some('$') {
                let start = self.at;
                while self.peek().is_some_and(|c| c != ',' && c != ')') {
                    self.at += 1;
                }
                let id: String = self.chars[start..self.at].iter().collect();
                args.push(Arg::Id(identifier(id.trim())?));
            } else {
                args.push(Arg::Parts(self.parts(&[',', ')'])?));
            }
            match self.next() {
                Some(',') => continue,
                Some(')') => return Ok(args),
                _ => {
                    return Err(problem(
                        "custom.bracket",
                        &[],
                        "a bracket is opened and not closed".to_owned(),
                    ));
                }
            }
        }
    }
}

/// `1`, `2-`, `chan`, …
fn identifier(written: &str) -> Result<Id, CommandProblem> {
    let (number, rest) = match written.strip_suffix('-') {
        Some(number) => (number, true),
        None => (written, false),
    };
    if let Ok(index) = number.parse::<usize>()
        && index >= 1
    {
        return Ok(Id::Word { index, rest });
    }
    let name = written.to_lowercase();
    if NAMED.contains(&name.as_str()) {
        return Ok(Id::Named(name));
    }
    Err(problem(
        "custom.identifier",
        &[("name", written)],
        format!("“$({written})” is not something this knows"),
    ))
}

/// What the values are, while a command runs.
struct Values<'a> {
    words: &'a [&'a str],
    context: &'a Context,
    command: &'a str,
}

impl Values<'_> {
    fn id(&self, id: &Id) -> String {
        match id {
            Id::Word { index, rest } => match rest {
                true => self.words.get(index - 1..).unwrap_or(&[]).join(" "),
                false => self
                    .words
                    .get(index - 1)
                    .map(|w| (*w).to_owned())
                    .unwrap_or_default(),
            },
            Id::Named(name) => {
                let c = self.context;
                match name.as_str() {
                    "chan" | "stream" => c.channel.clone().unwrap_or_default(),
                    "chans" => c.channels.join(" "),
                    "my-nick" => c.me.clone().unwrap_or_default(),
                    "streamviewers" => c.viewers.map(|v| v.to_string()).unwrap_or_default(),
                    "streamuptime" => c.uptime.clone().unwrap_or_default(),
                    "streamtitle" => c.title.clone().unwrap_or_default(),
                    "streamgame" => c.game.clone().unwrap_or_default(),
                    "streamstatus" => match (&c.viewers, &c.title, &c.game) {
                        (None, _, _) => "Offline".to_owned(),
                        (Some(_), Some(title), Some(game)) => format!("{title} ({game})"),
                        (Some(_), Some(title), None) => title.clone(),
                        _ => String::new(),
                    },
                    _ => String::new(),
                }
            }
        }
    }

    fn missing(&self, what: String) -> CommandProblem {
        problem(
            "custom.missing",
            &[("command", self.command), ("value", &what)],
            format!("/{} needs {what}", self.command),
        )
    }

    fn parts(&self, parts: &[Part], seed: &mut u64) -> Result<String, CommandProblem> {
        let mut out = String::new();
        for part in parts {
            match part {
                Part::Text(text) => out.push_str(text),
                Part::Value { id, required } => {
                    let value = self.id(id);
                    if *required && value.is_empty() {
                        return Err(self.missing(shown_id(id)));
                    }
                    out.push_str(&value);
                }
                Part::Call {
                    function,
                    args,
                    required,
                } => {
                    let value = self.call(*function, args, seed)?;
                    if *required && value.is_empty() {
                        return Err(self.missing(format!("${:?}(…)", function).to_lowercase()));
                    }
                    out.push_str(&value);
                }
            }
        }
        Ok(out)
    }

    fn arg(&self, arg: &Arg, seed: &mut u64) -> Result<String, CommandProblem> {
        match arg {
            Arg::Id(id) => Ok(self.id(id)),
            Arg::Parts(parts) => self.parts(parts, seed),
        }
    }

    fn call(
        &self,
        function: Function,
        args: &[Arg],
        seed: &mut u64,
    ) -> Result<String, CommandProblem> {
        let arg = |i: usize, seed: &mut u64| -> Result<String, CommandProblem> {
            match args.get(i) {
                Some(a) => self.arg(a, seed),
                None => Ok(String::new()),
            }
        };
        Ok(match function {
            Function::If => match arg(0, seed)?.is_empty() {
                false => arg(1, seed)?,
                true => arg(2, seed)?,
            },
            Function::IfEq => match arg(0, seed)? == arg(1, seed)? {
                true => arg(2, seed)?,
                false => arg(3, seed)?,
            },
            Function::Lower => arg(0, seed)?.to_lowercase(),
            Function::Upper => arg(0, seed)?.to_uppercase(),
            Function::Trim => arg(0, seed)?.trim().to_owned(),
            Function::Join => {
                let separator = arg(1, seed)?;
                arg(0, seed)?
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join(&separator)
            }
            Function::Rand => {
                let pick = (next(seed) % args.len() as u64) as usize;
                arg(pick, seed)?
            }
            Function::RandNum => {
                let number = |s: String| s.trim().parse::<i64>().unwrap_or(0);
                let (low, high) = match args.len() {
                    1 => (0, number(arg(0, seed)?) - 1),
                    _ => (number(arg(0, seed)?), number(arg(1, seed)?)),
                };
                let (low, high) = (low.min(high), low.max(high));
                let span = (high - low + 1) as u64;
                (low + (next(seed) % span) as i64).to_string()
            }
        })
    }
}

/// How a missing value is named to the reader.
fn shown_id(id: &Id) -> String {
    match id {
        Id::Word { index, rest: false } => format!("${index}"),
        Id::Word { index, rest: true } => format!("${index}-"),
        Id::Named(name) => format!("$({name})"),
    }
}

/// A start for the few random choices a command makes.
fn seed() -> u64 {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0x2545_f491_4f6c_dd1d);
    nanos | 1
}

/// A small xorshift: enough to pick a line, nothing to rely on.
fn next(seed: &mut u64) -> u64 {
    let mut x = *seed;
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;
    *seed = x;
    x
}

#[cfg(test)]
mod tests {
    use super::*;

    fn commands(lines: &[&str]) -> CustomCommands {
        CustomCommands::read(&lines.iter().map(|l| (*l).to_owned()).collect::<Vec<_>>()).unwrap()
    }

    fn context(channel: &str) -> Context {
        Context {
            channel: Some(channel.to_owned()),
            channels: vec!["harborlight".into(), "brightmaple".into()],
            me: Some("robin".into()),
            viewers: Some(1234),
            uptime: Some("1:15".into()),
            title: Some("Rust".into()),
            game: Some("Software".into()),
        }
    }

    fn run(list: &CustomCommands, typed: &str) -> Option<Result<String, &'static str>> {
        list.expand(typed, &context("harborlight"))
            .map(|r| r.map_err(|e| e.code))
    }

    #[test]
    fn chattys_examples() {
        let list = commands(&[
            "# a comment",
            "/hello Hello World!",
            "/slap /me slaps $$1 around a bit with a large trout",
            "/info /echo [Stream Status] $(streamstatus) [Uptime] $(streamuptime)",
        ]);
        assert_eq!(run(&list, "/hello"), Some(Ok("Hello World!".into())));
        assert_eq!(
            run(&list, "/SLAP moobot"),
            Some(Ok("/me slaps moobot around a bit with a large trout".into()))
        );
        assert_eq!(run(&list, "/slap"), Some(Err("custom.missing")));
        assert_eq!(
            run(&list, "/info"),
            Some(Ok(
                "/echo [Stream Status] Rust (Software) [Uptime] 1:15".into()
            ))
        );
        assert_eq!(run(&list, "/ban someone"), None, "not ours");
        assert_eq!(run(&list, "//hello"), None, "a literal line");
        assert_eq!(run(&list, "hello"), None);
    }

    #[test]
    fn words_and_named_values() {
        let list = commands(&[
            "/t $1|$2|$2-|$(10)|$(1-)",
            "/where $(chan) of $(chans), I am $(my-nick), $(streamviewers) watching",
        ]);
        assert_eq!(run(&list, "/t a b c"), Some(Ok("a|b|b c||a b c".into())));
        assert_eq!(
            run(&list, "/where"),
            Some(Ok(
                "harborlight of harborlight brightmaple, I am robin, 1234 watching".into()
            ))
        );
    }

    #[test]
    fn a_command_for_this_channel_wins() {
        let list = commands(&["/hi Hi all!", "/hi#harborlight Ahoj $1!"]);
        assert_eq!(run(&list, "/hi Eve"), Some(Ok("Ahoj Eve!".into())));
        assert_eq!(
            list.expand("/hi", &context("brightmaple"))
                .unwrap()
                .unwrap(),
            "Hi all!"
        );
    }

    #[test]
    fn functions() {
        let list = commands(&[
            "/a $if(1,yes $1,nope)",
            "/b $ifeq(1,on,ON,$upper(1))",
            "/c $join(1-,/) $lower($1) [$trim($\"  x  \")]",
            "/d $rand(one,one,one)",
            "/e $randnum(5,5) $randnum(1)",
            "/f costs \\$5 and $\"$1\" and $'it''s'",
        ]);
        assert_eq!(run(&list, "/a cheese"), Some(Ok("yes cheese".into())));
        assert_eq!(run(&list, "/a"), Some(Ok("nope".into())));
        assert_eq!(run(&list, "/b on"), Some(Ok("ON".into())));
        assert_eq!(run(&list, "/b off"), Some(Ok("OFF".into())));
        assert_eq!(run(&list, "/c A b C"), Some(Ok("A/b/C a [x]".into())));
        assert_eq!(run(&list, "/d"), Some(Ok("one".into())));
        assert_eq!(run(&list, "/e"), Some(Ok("5 0".into())));
        assert_eq!(
            run(&list, "/f"),
            Some(Ok("costs $5 and $1 and it's".into()))
        );
    }

    #[test]
    fn what_cannot_be_read_says_why_and_where() {
        let code = |line: &str| CustomCommand::parse(line).unwrap_err().code;
        assert_eq!(code("hello world"), "custom.name");
        assert_eq!(code("/ x"), "custom.name");
        assert_eq!(code("/x $request(http://x)"), "custom.function");
        assert_eq!(code("/x $(nope)"), "custom.identifier");
        assert_eq!(code("/x $(1"), "custom.bracket");
        assert_eq!(code("/x $\"open"), "custom.quote");
        assert_eq!(code("/x $ 5"), "custom.syntax");
        assert_eq!(code("/x $lower(1,2)"), "custom.syntax");
        let problems =
            CustomCommands::read(&["/ok fine".into(), "nope".into(), "/x $(1".into()]).unwrap_err();
        let at: Vec<usize> = problems.iter().map(|p| p.number).collect();
        assert_eq!(at, [2, 3]);
        assert_eq!(commands(&["/b x", "/a y", "/a#c z"]).names(), ["a", "b"]);
    }
}
