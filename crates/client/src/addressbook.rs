//! `/ab`: editing the address book by typing, as in Chatty (whose other
//! name for it, `/users`, is the user list here).
//! Each change is saved at once; a frontend then hands the engine the
//! book with the rules, since rules and `/join` read it there.

use liveside_core::addressbook::{self as book, AddressBook};
use liveside_proto::model::NoticeLevel;

use crate::lang::{Strings, fill};
use crate::settings::Settings;

/// What `/ab` did: what to say, and whether the book changed, in which
/// case the engine is to be given it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Answer {
    pub level: NoticeLevel,
    pub said: String,
    pub changed: bool,
}

fn said(level: NoticeLevel, said: String) -> Answer {
    Answer {
        level,
        said,
        changed: false,
    }
}

/// How an entry reads after a change, or when asked for.
fn entry(text: &Strings, book: &AddressBook, name: &str) -> String {
    if !book.contains(name) {
        return fill(text.ab_not_in, &[("name", name)]);
    }
    match book.categories_of(name) {
        [] => fill(text.ab_entry_none, &[("name", name)]),
        categories => fill(
            text.ab_entry,
            &[("name", name), ("categories", &categories.join(", "))],
        ),
    }
}

/// Runs `/ab <args>`.
pub fn command(settings: &mut Settings, args: &str, text: &'static Strings) -> Answer {
    let words: Vec<&str> = args.split_whitespace().collect();
    let Some((&action, rest)) = words.split_first() else {
        return said(NoticeLevel::Error, text.ab_usage.to_owned());
    };
    let action = action.to_ascii_lowercase();
    let wrong = |e: book::BookError| said(NoticeLevel::Error, crate::lang::book_problem(text, &e));
    // A change saved, or the save that failed.
    let saved = |done: Result<String, String>| match done {
        Ok(said) => Answer {
            level: NoticeLevel::Info,
            said,
            changed: true,
        },
        Err(e) => self::said(
            NoticeLevel::Error,
            fill(text.ignore_failed, &[("error", &e)]),
        ),
    };

    match (action.as_str(), rest) {
        ("info", []) => {
            let book = settings.addressbook();
            if book.is_empty() {
                return said(NoticeLevel::Info, text.ab_info_empty.to_owned());
            }
            let categories: Vec<&str> = book.all_categories().into_iter().collect();
            said(
                NoticeLevel::Info,
                fill(
                    text.ab_info,
                    &[
                        ("count", &book.len().to_string()),
                        ("categories", &categories.join(", ")),
                    ],
                ),
            )
        }
        ("get", [who]) => match book::name(who) {
            Ok(name) => said(
                NoticeLevel::Info,
                entry(text, settings.addressbook(), &name),
            ),
            Err(e) => wrong(e),
        },
        ("add" | "set" | "remove", [who, more @ ..]) if more.len() <= 1 => {
            let name = match book::name(who) {
                Ok(name) => name,
                Err(e) => return wrong(e),
            };
            let categories = match more.first().map(|c| book::categories(c)).transpose() {
                Ok(categories) => categories,
                Err(e) => return wrong(e),
            };
            if action == "remove" && !settings.addressbook().contains(&name) {
                return said(NoticeLevel::Error, fill(text.ab_not_in, &[("name", &name)]));
            }
            let done = settings.change_addressbook(|book| match (action.as_str(), &categories) {
                ("add", list) => {
                    book.add(&name, list.as_deref().unwrap_or(&[]));
                }
                ("set", list) => {
                    book.set(&name, list.as_deref().unwrap_or(&[]));
                }
                (_, Some(list)) => {
                    book.remove_categories(&name, list);
                }
                (_, None) => {
                    book.remove(&name);
                }
            });
            let report = match (action.as_str(), &categories) {
                ("remove", None) => fill(text.ab_removed, &[("name", &name)]),
                _ => entry(text, settings.addressbook(), &name),
            };
            saved(done.map(|()| report))
        }
        ("change", [who, groups @ ..]) if !groups.is_empty() => {
            let name = match book::name(who) {
                Ok(name) => name,
                Err(e) => return wrong(e),
            };
            match settings.change_addressbook(|book| book.change(&name, groups)) {
                Ok(Err(e)) => wrong(e),
                Ok(Ok(())) => {
                    let report = entry(text, settings.addressbook(), &name);
                    saved(Ok(report))
                }
                Err(e) => saved(Err(e)),
            }
        }
        ("renamecategory", [old, new]) => {
            let (old, new) = match (book::categories(old), book::categories(new)) {
                (Ok(old), Ok(new)) if old.len() == 1 && new.len() == 1 => {
                    (old[0].clone(), new[0].clone())
                }
                (Err(e), _) | (_, Err(e)) => return wrong(e),
                _ => return said(NoticeLevel::Error, text.ab_usage.to_owned()),
            };
            let done = settings.change_addressbook(|book| book.rename_category(&old, &new));
            saved(done.map(|count| {
                fill(
                    text.ab_renamed,
                    &[("old", &old), ("new", &new), ("count", &count.to_string())],
                )
            }))
        }
        ("removecategory", [category]) => {
            let category = match book::categories(category) {
                Ok(list) if list.len() == 1 => list[0].clone(),
                Err(e) => return wrong(e),
                _ => return said(NoticeLevel::Error, text.ab_usage.to_owned()),
            };
            let done = settings.change_addressbook(|book| book.remove_category(&category));
            saved(done.map(|count| {
                fill(
                    text.ab_category_removed,
                    &[("category", &category), ("count", &count.to_string())],
                )
            }))
        }
        _ => said(NoticeLevel::Error, text.ab_usage.to_owned()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lang::Language;
    use crate::settings::Frontend;

    fn settings(name: &str) -> Settings {
        let path =
            std::env::temp_dir().join(format!("liveside-ab-{name}-{}.toml", std::process::id()));
        let _ = std::fs::remove_file(&path);
        Settings::new(
            crate::Config::default(),
            path,
            Language::English,
            Frontend::Terminal,
        )
    }

    fn run(settings: &mut Settings, args: &str) -> String {
        command(settings, args, Language::English.strings()).said
    }

    #[test]
    fn chattys_examples_work_and_are_saved() {
        let mut s = settings("examples");
        assert_eq!(run(&mut s, "add josh vip"), "Address book: josh is in vip");
        assert_eq!(
            run(&mut s, "set josh rainbow"),
            "Address book: josh is in rainbow"
        );
        assert_eq!(
            run(&mut s, "add @Josh VIP"),
            "Address book: josh is in rainbow, vip"
        );
        assert_eq!(
            run(&mut s, "remove josh vip"),
            "Address book: josh is in rainbow"
        );
        assert_eq!(
            run(&mut s, "change #harborlight +a,b -c !d"),
            "Address book: #harborlight is in a, b, d"
        );
        assert_eq!(
            run(&mut s, "info"),
            "Address book: 2 names; categories: a, b, d, rainbow"
        );
        assert_eq!(
            run(&mut s, "renameCategory rainbow a"),
            "Address book: category rainbow is now a (1 names)"
        );
        let saved = crate::Config::load(s.path()).unwrap();
        assert_eq!(saved.addressbook.categories_of("josh"), ["a"]);
        assert_eq!(
            saved.addressbook.categories_of("#harborlight"),
            ["a", "b", "d"]
        );
        assert_eq!(
            run(&mut s, "removeCategory a"),
            "Address book: category a taken from 2 names"
        );
        assert_eq!(
            run(&mut s, "remove josh"),
            "Address book: josh is no longer in it"
        );
        assert_eq!(run(&mut s, "get josh"), "Address book: josh is not in it");
        let row = s.rows.iter().find(|r| r.key == "addressbook").unwrap();
        assert_eq!(
            row.value,
            crate::settings::Value::Rules(vec!["#harborlight b,d".to_owned()]),
            "the settings show it"
        );
        let _ = std::fs::remove_file(s.path());
    }

    #[test]
    fn what_cannot_be_done_says_why_and_changes_nothing() {
        let mut s = settings("wrong");
        let answer = command(&mut s, "add two-words!", Language::English.strings());
        assert_eq!(answer.level, NoticeLevel::Error);
        assert!(!answer.changed);
        assert!(answer.said.contains("not the name"), "{}", answer.said);
        assert!(run(&mut s, "").starts_with("Usage: /ab"));
        assert!(run(&mut s, "change josh vip").contains("not a category"));
        assert_eq!(
            run(&mut s, "remove nobody"),
            "Address book: nobody is not in it"
        );
        assert!(!s.path().exists(), "nothing was written");
    }
}
