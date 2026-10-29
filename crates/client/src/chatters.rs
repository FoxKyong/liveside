//! Who is active in a channel, as the user list shows them.
//!
//! Twitch only tells moderators who is present (Get Chatters needs the
//! `moderator:read:chatters` scope *and* moderator status in that channel),
//! so this is mostly whoever has talked while we were joined. For a chat
//! client that is usually the more useful list anyway.
//!
//! Both frontends build the list from the messages they still hold, so
//! someone whose last message has scrolled out of the buffer drops off it.

use std::collections::HashSet;

use liveside_proto::Login;
use liveside_proto::model::Badge;

/// Someone active in a channel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chatter {
    pub name: String,
    /// Badge symbol shown before the name, e.g. `@` for a moderator.
    /// What a terminal shows, and what a window shows until the pictures
    /// have arrived.
    pub symbol: char,
    /// Sort order: broadcaster first, then moderators, VIPs, subscribers.
    pub rank: u8,
    /// The badges of their most recent message, for a frontend that can
    /// draw Twitch's own pictures. Empty for someone who has not talked.
    pub badges: Vec<Badge>,
}

/// The badge symbol and rank of whoever wears the highest of these badges.
pub fn rank_of(badges: &[Badge]) -> (char, u8) {
    let mut best = (' ', 0);
    for badge in badges {
        let candidate = match badge.set_id.as_str() {
            "broadcaster" => ('~', 5),
            "staff" | "admin" => ('&', 5),
            "moderator" => ('@', 4),
            "vip" => ('!', 3),
            "subscriber" | "founder" => ('%', 2),
            _ => continue,
        };
        if candidate.1 > best.1 {
            best = candidate;
        }
    }
    best
}

/// Collects the people to show, newest speaker first, then sorts them.
#[derive(Debug, Default)]
pub struct Gathering {
    seen: HashSet<Login>,
    list: Vec<Chatter>,
}

impl Gathering {
    /// Records someone who has talked. Messages are passed newest first,
    /// so the badges and the spelling of the name come from their most
    /// recent message and later mentions of the same person are ignored.
    pub fn talked(&mut self, login: &Login, display_name: &str, badges: &[Badge]) {
        if !self.seen.insert(login.clone()) {
            return;
        }
        let (symbol, rank) = rank_of(badges);
        self.list.push(Chatter {
            name: display_name.to_owned(),
            symbol,
            rank,
            badges: badges.to_vec(),
        });
    }

    /// Records someone the engine says is in the channel but who has not
    /// talked: only a login, so no badges and no spelling of their name.
    pub fn present(&mut self, login: &Login) {
        if self.seen.insert(login.clone()) {
            self.list.push(Chatter {
                name: login.to_string(),
                symbol: ' ',
                rank: 0,
                badges: Vec::new(),
            });
        }
    }

    /// The list as it is shown: ranked first, then alphabetical.
    pub fn finish(mut self) -> Vec<Chatter> {
        self.list.sort_by(|a, b| {
            b.rank
                .cmp(&a.rank)
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        });
        self.list
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn badge(set_id: &str) -> Badge {
        Badge {
            set_id: set_id.to_owned(),
            id: "1".to_owned(),
            info: String::new(),
            image: String::new(),
            title: String::new(),
        }
    }

    #[test]
    fn the_highest_badge_names_the_symbol() {
        assert_eq!(
            rank_of(&[badge("subscriber"), badge("moderator")]),
            ('@', 4)
        );
        assert_eq!(rank_of(&[badge("glitchcon2020")]), (' ', 0));
    }

    #[test]
    fn the_most_recent_message_spells_the_name() {
        let mut gathering = Gathering::default();
        gathering.talked(&Login::from("ann"), "Ann", &[badge("moderator")]);
        gathering.talked(&Login::from("ann"), "ann", &[]);
        assert_eq!(
            gathering.finish(),
            vec![Chatter {
                name: "Ann".to_owned(),
                symbol: '@',
                rank: 4,
                badges: vec![badge("moderator")],
            }]
        );
    }

    #[test]
    fn people_who_have_talked_keep_their_badges_over_a_bare_login() {
        let mut gathering = Gathering::default();
        gathering.talked(&Login::from("ann"), "Ann", &[badge("vip")]);
        gathering.present(&Login::from("ann"));
        gathering.present(&Login::from("bob"));
        let list = gathering.finish();
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].symbol, '!');
    }

    #[test]
    fn rank_comes_before_the_alphabet() {
        let mut gathering = Gathering::default();
        gathering.talked(&Login::from("zoe"), "Zoe", &[badge("broadcaster")]);
        gathering.talked(&Login::from("ann"), "Ann", &[]);
        gathering.talked(&Login::from("bob"), "Bob", &[]);
        let list = gathering.finish();
        let names: Vec<&str> = list.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["Zoe", "Ann", "Bob"]);
    }
}
