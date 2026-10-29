//! The address book: names, of people or of `#channels`, each with the
//! categories it is in. Rules refer to the categories (`cat:`,
//! `chanCat:`), and `/join [category]` joins every channel in one.
//!
//! Kept in the settings file as Chatty keeps it in its own, a name and its
//! categories:
//!
//! ```toml
//! [addressbook]
//! alice = ["friends", "vip"]
//! "#harborlight" = ["czech"]
//! ```
//!
//! Names are logins, lowercase; a channel's has a `#` in front, so a
//! person and their channel can be in different categories. Categories
//! are lowercase words without spaces or commas.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct AddressBook {
    entries: BTreeMap<String, Vec<String>>,
}

/// What is wrong with a name or a category, with a code and arguments for
/// a frontend to say it in its language.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BookError {
    pub code: &'static str,
    pub args: Vec<(&'static str, String)>,
    pub message: String,
}

impl std::fmt::Display for BookError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

fn bad_name(name: &str) -> BookError {
    BookError {
        code: "addressbook.name",
        args: vec![("name", name.to_owned())],
        message: format!("“{name}” is not the name of a person or a #channel"),
    }
}

fn bad_category(category: &str) -> BookError {
    BookError {
        code: "addressbook.category",
        args: vec![("category", category.to_owned())],
        message: format!("“{category}” is not a category: one word, without commas"),
    }
}

/// A name as the book keeps it: lowercase, `#` kept for a channel, `@`
/// dropped for a person.
pub fn name(written: &str) -> Result<String, BookError> {
    let written = written.trim();
    let (channel, login) = match written.strip_prefix('#') {
        Some(login) => (true, login),
        None => (false, written.trim_start_matches('@')),
    };
    let login = login.to_lowercase();
    let valid = (1..=25).contains(&login.len())
        && login
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_');
    if !valid {
        return Err(bad_name(written));
    }
    Ok(match channel {
        true => format!("#{login}"),
        false => login,
    })
}

/// Categories written as Chatty writes them, `a,b,c`, lowercase.
pub fn categories(written: &str) -> Result<Vec<String>, BookError> {
    let mut out = Vec::new();
    for category in written.split(',') {
        let category = category.trim().to_lowercase();
        if category.is_empty() {
            continue;
        }
        check_category(&category)?;
        out.push(category);
    }
    Ok(out)
}

fn check_category(category: &str) -> Result<(), BookError> {
    if category.is_empty() || category.contains(|c: char| c.is_whitespace() || c == ',') {
        return Err(bad_category(category));
    }
    Ok(())
}

/// How `/join [...]` narrows a category down.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Entries {
    All,
    /// Only `#channel` names.
    Channels,
    /// Only names of people, whose channels are joined.
    People,
}

impl AddressBook {
    /// The book with every name and category checked and put in the form
    /// it is kept in: what the settings file says, made usable.
    pub fn checked(self) -> Result<Self, BookError> {
        let mut book = AddressBook::default();
        for (written, categories) in self.entries {
            let name = name(&written)?;
            let mut kept: Vec<String> = Vec::new();
            for category in categories {
                let category = category.trim().to_lowercase();
                check_category(&category)?;
                kept.push(category);
            }
            book.entries.entry(name).or_default().extend(kept);
        }
        for categories in book.entries.values_mut() {
            categories.sort();
            categories.dedup();
        }
        Ok(book)
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn contains(&self, name: &str) -> bool {
        self.entries.contains_key(name)
    }

    /// The categories a name is in; none for a name not in the book.
    pub fn categories_of(&self, name: &str) -> &[String] {
        self.entries.get(name).map(Vec::as_slice).unwrap_or(&[])
    }

    /// Every category in use, in order.
    pub fn all_categories(&self) -> BTreeSet<&str> {
        self.entries
            .values()
            .flatten()
            .map(String::as_str)
            .collect()
    }

    /// The names in a category, in order.
    pub fn names_in(&self, category: &str, which: Entries) -> Vec<&str> {
        self.entries
            .iter()
            .filter(|(name, categories)| {
                let channel = name.starts_with('#');
                let wanted = match which {
                    Entries::All => true,
                    Entries::Channels => channel,
                    Entries::People => !channel,
                };
                wanted && categories.iter().any(|c| c == category)
            })
            .map(|(name, _)| name.as_str())
            .collect()
    }

    /// Adds a name, or categories to a name already there. Whether
    /// anything changed.
    pub fn add(&mut self, name: &str, categories: &[String]) -> bool {
        let before = self.entries.get(name).cloned();
        let entry = self.entries.entry(name.to_owned()).or_default();
        entry.extend(categories.iter().cloned());
        entry.sort();
        entry.dedup();
        before.as_ref() != self.entries.get(name)
    }

    /// Gives a name exactly these categories, adding it if needed.
    pub fn set(&mut self, name: &str, categories: &[String]) -> bool {
        let mut new = categories.to_vec();
        new.sort();
        new.dedup();
        self.entries.insert(name.to_owned(), new.clone()) != Some(new)
    }

    /// Takes a name out of the book. Whether it was in it.
    pub fn remove(&mut self, name: &str) -> bool {
        self.entries.remove(name).is_some()
    }

    /// Takes categories away from a name, which stays. Whether it was in
    /// the book.
    pub fn remove_categories(&mut self, name: &str, categories: &[String]) -> bool {
        let Some(entry) = self.entries.get_mut(name) else {
            return false;
        };
        entry.retain(|c| !categories.contains(c));
        true
    }

    /// Chatty's `change`: groups of categories, each led by `+` (add),
    /// `-` (take away) or `!` (flip): `+a,b -c !d`.
    pub fn change(&mut self, name: &str, groups: &[&str]) -> Result<(), BookError> {
        let mut entry: BTreeSet<String> = self.categories_of(name).iter().cloned().collect();
        for group in groups {
            let mut chars = group.chars();
            let how = chars.next();
            let list = categories(chars.as_str())?;
            for category in list {
                match how {
                    Some('+') => {
                        entry.insert(category);
                    }
                    Some('-') => {
                        entry.remove(&category);
                    }
                    Some('!') => {
                        if !entry.remove(&category) {
                            entry.insert(category);
                        }
                    }
                    _ => return Err(bad_category(group)),
                }
            }
            if !matches!(how, Some('+' | '-' | '!')) {
                return Err(bad_category(group));
            }
        }
        self.entries
            .insert(name.to_owned(), entry.into_iter().collect());
        Ok(())
    }

    /// Renames a category everywhere, merging it into one of that name if
    /// there is one. How many names had it.
    pub fn rename_category(&mut self, old: &str, new: &str) -> usize {
        let mut count = 0;
        for categories in self.entries.values_mut() {
            if let Some(at) = categories.iter().position(|c| c == old) {
                categories[at] = new.to_owned();
                categories.sort();
                categories.dedup();
                count += 1;
            }
        }
        count
    }

    /// Takes a category away from every name. How many had it.
    pub fn remove_category(&mut self, category: &str) -> usize {
        let mut count = 0;
        for categories in self.entries.values_mut() {
            let before = categories.len();
            categories.retain(|c| c != category);
            count += usize::from(categories.len() != before);
        }
        count
    }

    /// The book as lines, `name cat1,cat2`, for a list to edit.
    pub fn lines(&self) -> Vec<String> {
        self.entries
            .iter()
            .map(|(name, categories)| match categories.is_empty() {
                true => name.clone(),
                false => format!("{name} {}", categories.join(",")),
            })
            .collect()
    }

    /// Reads one such line.
    pub fn line(written: &str) -> Result<(String, Vec<String>), BookError> {
        let written = written.trim();
        let (who, rest) = written
            .split_once(char::is_whitespace)
            .unwrap_or((written, ""));
        let rest = rest.trim();
        if rest.contains(char::is_whitespace) {
            return Err(bad_category(rest));
        }
        Ok((name(who)?, categories(rest)?))
    }

    /// The book from lines, empty ones left out. The first line that
    /// cannot be read, numbered from one, and what is wrong with it.
    pub fn from_lines(lines: &[String]) -> Result<Self, (usize, BookError)> {
        let mut book = AddressBook::default();
        for (i, line) in lines.iter().enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            let (name, categories) = Self::line(line).map_err(|e| (i + 1, e))?;
            book.add(&name, &categories);
        }
        Ok(book)
    }

    /// The book in the form the protocol carries it.
    pub fn to_map(&self) -> BTreeMap<String, Vec<String>> {
        self.entries.clone()
    }

    pub fn from_map(map: BTreeMap<String, Vec<String>>) -> Result<Self, BookError> {
        AddressBook { entries: map }.checked()
    }
}

/// Splits what follows `/join` into what is to be joined: channels
/// separated by commas or spaces, and `[category ...]` kept whole.
pub use liveside_proto::join_targets;

/// What `[category #]` asks for: a category, and which of its names.
pub fn category_target(written: &str) -> Option<Result<(String, Entries), BookError>> {
    let inside = written.trim().strip_prefix('[')?.strip_suffix(']')?;
    let mut words = inside.split_whitespace();
    let Some(category) = words.next() else {
        return Some(Err(bad_category("")));
    };
    let mut which = Entries::All;
    for word in words {
        which = match word {
            "#" => Entries::Channels,
            "!#" => Entries::People,
            other => {
                return Some(Err(BookError {
                    code: "join.category_option",
                    args: vec![("option", other.to_owned())],
                    message: format!(
                        "“{other}” is not something [category ...] understands here; \
                         there are # and !#"
                    ),
                }));
            }
        };
    }
    let category = category.to_lowercase();
    if category == "*" {
        return Some(Err(BookError {
            code: "join.favourites",
            args: Vec::new(),
            message: "[*] joins Chatty's favourites, which this does not have; \
                      use a category"
                .to_owned(),
        }));
    }
    if let Err(e) = check_category(&category) {
        return Some(Err(e));
    }
    Some(Ok((category, which)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cats(list: &[&str]) -> Vec<String> {
        list.iter().map(|c| (*c).to_owned()).collect()
    }

    #[test]
    fn names_and_categories_are_kept_in_one_form() {
        assert_eq!(name("@Alice").unwrap(), "alice");
        assert_eq!(name("#Harborlight").unwrap(), "#harborlight");
        assert_eq!(name("two words").unwrap_err().code, "addressbook.name");
        assert_eq!(name("").unwrap_err().code, "addressbook.name");
        assert_eq!(
            categories("VIP,,friends").unwrap(),
            cats(&["vip", "friends"])
        );
        let book =
            AddressBook::from_map([("Alice".to_owned(), cats(&["VIP", "friends", "vip"]))].into())
                .unwrap();
        assert_eq!(book.categories_of("alice"), ["friends", "vip"]);
        assert!(AddressBook::from_map([("a".to_owned(), cats(&["two words"]))].into()).is_err());
    }

    #[test]
    fn chattys_commands_edit_it() {
        let mut book = AddressBook::default();
        assert!(book.add("josh", &cats(&["vip"])));
        assert!(!book.add("josh", &cats(&["vip"])), "nothing new");
        book.set("josh", &cats(&["rainbow"]));
        book.add("josh", &cats(&["vip"]));
        assert_eq!(book.categories_of("josh"), ["rainbow", "vip"]);
        book.remove_categories("josh", &cats(&["vip"]));
        assert_eq!(book.categories_of("josh"), ["rainbow"]);
        book.change("test", &["+a,b", "-c", "!d"]).unwrap();
        assert_eq!(book.categories_of("test"), ["a", "b", "d"]);
        book.change("test", &["!d", "-a"]).unwrap();
        assert_eq!(book.categories_of("test"), ["b"]);
        assert!(book.change("test", &["b"]).is_err());
        book.add("#chan", &cats(&["b"]));
        assert_eq!(book.rename_category("b", "rainbow"), 2);
        assert_eq!(book.categories_of("test"), ["rainbow"]);
        assert_eq!(book.categories_of("josh"), ["rainbow"], "merged, once");
        assert_eq!(book.names_in("rainbow", Entries::Channels), ["#chan"]);
        assert_eq!(book.names_in("rainbow", Entries::People), ["josh", "test"]);
        assert_eq!(book.remove_category("rainbow"), 3);
        assert!(book.all_categories().is_empty());
        assert!(book.remove("josh"));
        assert!(!book.remove("josh"));
    }

    #[test]
    fn it_is_edited_as_lines() {
        let lines = vec![
            "alice friends,vip".to_owned(),
            String::new(),
            "#harborlight czech".to_owned(),
            "bob".to_owned(),
        ];
        let book = AddressBook::from_lines(&lines).unwrap();
        assert_eq!(
            book.lines(),
            ["#harborlight czech", "alice friends,vip", "bob"]
        );
        let (at, error) =
            AddressBook::from_lines(&["ok a".to_owned(), "x a, b".to_owned()]).unwrap_err();
        assert_eq!((at, error.code), (2, "addressbook.category"));
    }

    #[test]
    fn a_category_to_join_is_read_from_brackets() {
        assert!(category_target("harborlight").is_none());
        assert_eq!(
            category_target("[VIP #]").unwrap().unwrap(),
            ("vip".to_owned(), Entries::Channels)
        );
        assert_eq!(
            category_target("[vip !#]").unwrap().unwrap().1,
            Entries::People
        );
        assert_eq!(
            category_target("[vip live]").unwrap().unwrap_err().code,
            "join.category_option"
        );
        assert_eq!(
            category_target("[*]").unwrap().unwrap_err().code,
            "join.favourites"
        );
    }
}
