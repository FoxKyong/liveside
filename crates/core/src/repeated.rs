//! Repeated messages: somebody saying the same thing, or nearly, again
//! and again. Each channel remembers what each person said lately, and a
//! message like enough to earlier ones is marked with how many times it
//! has now been said, which the frontends show as "(×3)" and rules match
//! with `config:repeatedmsg`.
//!
//! Written from Chatty's description of its Repeated Chat Message
//! Detection, not from its code. Messages are compared without case and
//! without whitespace, by how few single-character changes turn one into
//! the other, as a share of the longer one.

use std::collections::{HashMap, VecDeque};

use liveside_proto::model::RepeatedConfig;
use liveside_proto::{Timestamp, UserId};

use crate::rules::{Rule, RuleError};

/// How many of a person's messages are kept to compare with.
const KEPT: usize = 20;

/// Longer messages are compared by their start: spam that differs only at
/// the end of a long text is still the same spam, and comparing is
/// quadratic in the length.
const COMPARED: usize = 300;

/// The settings, read.
#[derive(Debug, Clone)]
pub struct Repeated {
    /// From how many times a message is marked; 0 is off.
    pub times: u32,
    /// How alike two messages must be, in percent.
    pub similarity: u32,
    /// How far back to look, in milliseconds.
    pub window: i64,
    /// Shorter messages are not looked at.
    pub min_length: usize,
    /// When set, only messages it matches are looked at.
    pub only: Option<Rule>,
}

impl Default for Repeated {
    fn default() -> Self {
        Self::read(&RepeatedConfig::default()).expect("the defaults read")
    }
}

impl Repeated {
    pub fn read(config: &RepeatedConfig) -> Result<Self, RuleError> {
        let only = match config.only.trim() {
            "" => None,
            rule => Some(Rule::parse(rule)?),
        };
        Ok(Repeated {
            times: config.times,
            similarity: config.similarity.clamp(1, 100),
            window: i64::from(config.minutes.max(1)) * 60_000,
            min_length: config.min_length as usize,
            only,
        })
    }

    pub fn is_on(&self) -> bool {
        self.times > 0
    }
}

/// What one channel remembers.
#[derive(Debug, Default)]
pub struct Memory {
    said: HashMap<UserId, VecDeque<(Timestamp, Vec<char>)>>,
}

impl Memory {
    /// Notes what somebody said, and answers how many times they have
    /// now said it, or something like enough, within the window: 1 for
    /// the first time.
    pub fn said(&mut self, settings: &Repeated, who: &UserId, when: Timestamp, text: &str) -> u32 {
        let text = comparable(text);
        let earlier = self.said.entry(who.clone()).or_default();
        earlier.retain(|(at, _)| when.0 - at.0 <= settings.window);
        let alike = earlier
            .iter()
            .filter(|(_, before)| similarity(before, &text) >= settings.similarity)
            .count() as u32;
        earlier.push_back((when, text));
        while earlier.len() > KEPT {
            earlier.pop_front();
        }
        alike + 1
    }

    /// Forgets what is past the window, and the people with nothing left,
    /// so a busy channel does not keep everyone it ever heard.
    pub fn forget(&mut self, now: Timestamp, window: i64) {
        self.said.retain(|_, earlier| {
            earlier.retain(|(at, _)| now.0 - at.0 <= window);
            !earlier.is_empty()
        });
    }

    #[cfg(test)]
    fn people(&self) -> usize {
        self.said.len()
    }
}

fn comparable(text: &str) -> Vec<char> {
    text.chars()
        .filter(|c| !c.is_whitespace())
        .flat_map(char::to_lowercase)
        .take(COMPARED)
        .collect()
}

/// How alike two texts are, in percent: 100 for the same.
fn similarity(a: &[char], b: &[char]) -> u32 {
    let longer = a.len().max(b.len());
    if longer == 0 {
        return 100;
    }
    // Too different in length to come close: no need to compare.
    let shorter = a.len().min(b.len());
    if shorter * 100 < longer * 50 {
        return (shorter * 100 / longer) as u32;
    }
    let changes = distance(a, b);
    ((longer - changes) * 100 / longer) as u32
}

/// The fewest single-character insertions, deletions and replacements
/// that turn one into the other.
fn distance(a: &[char], b: &[char]) -> usize {
    let mut previous: Vec<usize> = (0..=b.len()).collect();
    let mut current = vec![0; b.len() + 1];
    for (i, ca) in a.iter().enumerate() {
        current[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let replace = previous[j] + usize::from(ca != cb);
            current[j + 1] = replace.min(previous[j + 1] + 1).min(current[j] + 1);
        }
        std::mem::swap(&mut previous, &mut current);
    }
    previous[b.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings(similarity: u32) -> Repeated {
        Repeated::read(&RepeatedConfig {
            similarity,
            ..RepeatedConfig::default()
        })
        .unwrap()
    }

    #[test]
    fn alike_is_counted_without_case_or_spaces() {
        let chars = |s: &str| comparable(s);
        assert_eq!(
            similarity(&chars("Buy Followers"), &chars("buyfollowers")),
            100
        );
        assert_eq!(distance(&chars("kitten"), &chars("sitting")), 3);
        assert_eq!(similarity(&chars("abcdefghij"), &chars("abcdefghiX")), 90);
        assert_eq!(similarity(&chars("a"), &chars("abcdefghij")), 10);
        assert_eq!(similarity(&[], &[]), 100);
    }

    #[test]
    fn a_person_repeating_themselves_is_counted_within_the_window() {
        let s = settings(90);
        let mut memory = Memory::default();
        let bot: UserId = "1".into();
        let other: UserId = "2".into();
        let minute = 60_000;
        assert_eq!(memory.said(&s, &bot, Timestamp(0), "buy followers at x"), 1);
        assert_eq!(
            memory.said(&s, &other, Timestamp(1), "buy followers at x"),
            1,
            "somebody else"
        );
        assert_eq!(
            memory.said(&s, &bot, Timestamp(2), "BUY followers  at x"),
            2
        );
        assert_eq!(
            memory.said(&s, &bot, Timestamp(3), "buy followers at y"),
            3,
            "nearly"
        );
        assert_eq!(memory.said(&s, &bot, Timestamp(4), "something else"), 1);
        // An hour after the third, the two before it are past the window
        // and the third is just inside it.
        let later = Timestamp(60 * minute + 3);
        assert_eq!(memory.said(&s, &bot, later, "buy followers at x"), 2);
        memory.forget(Timestamp(200 * minute), s.window);
        assert_eq!(memory.people(), 0);
    }

    #[test]
    fn only_so_many_are_kept_and_the_rule_is_checked() {
        let s = settings(100);
        let mut memory = Memory::default();
        let who: UserId = "1".into();
        let mut last = 0;
        for i in 0..50 {
            last = memory.said(&s, &who, Timestamp(i), "same");
        }
        assert_eq!(last as usize, KEPT + 1);
        let wrong = RepeatedConfig {
            only: "msgs:x".into(),
            ..RepeatedConfig::default()
        };
        assert_eq!(Repeated::read(&wrong).unwrap_err().code, "rule.unsupported");
        assert!(
            !Repeated::read(&RepeatedConfig {
                times: 0,
                ..RepeatedConfig::default()
            })
            .unwrap()
            .is_on()
        );
    }
}
