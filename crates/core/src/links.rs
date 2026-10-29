//! Finding links in chat text.
//!
//! Twitch marks mentions and emotes but not links, so the engine finds them
//! itself and every frontend gets the same `Fragment::Link`s. Only words
//! that start with `http://` or `https://` count: a bare `example.com` is
//! left as text, since guessing would turn `file.txt` into a website.
//!
//! The text stays exactly as it was typed and is always shown whole. The
//! `url` beside it is the parsed form, with the host in punycode, which is
//! what a browser is handed and what a frontend shows when it asks.

use liveside_proto::model::{ChatMessage, Fragment, Line, LineContent};

/// Finds the links in a line's message, if it has one.
pub fn mark_line(line: &mut Line) {
    if let LineContent::Message(message) = &mut line.content {
        mark(message);
    }
}

/// Splits the message's text fragments around the links in them.
pub fn mark(message: &mut ChatMessage) {
    if message
        .fragments
        .iter()
        .any(|fragment| matches!(fragment, Fragment::Text { text } if text.contains("http")))
    {
        message.fragments = marked(std::mem::take(&mut message.fragments));
    }
}

fn marked(fragments: Vec<Fragment>) -> Vec<Fragment> {
    let mut out = Vec::with_capacity(fragments.len());
    for fragment in fragments {
        match fragment {
            Fragment::Text { text } => split(&text, &mut out),
            other => out.push(other),
        }
    }
    out
}

fn split(text: &str, out: &mut Vec<Fragment>) {
    let mut plain = String::new();
    let mut rest = text;
    while !rest.is_empty() {
        // A word, and the spaces before it.
        let start = rest
            .find(|c: char| !c.is_whitespace())
            .unwrap_or(rest.len());
        plain.push_str(&rest[..start]);
        rest = &rest[start..];
        let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
        let word = &rest[..end];
        rest = &rest[end..];
        match find(word) {
            Some((before, link, url, after)) => {
                plain.push_str(before);
                if !plain.is_empty() {
                    out.push(Fragment::Text {
                        text: std::mem::take(&mut plain),
                    });
                }
                out.push(Fragment::Link {
                    text: link.to_owned(),
                    url,
                });
                plain.push_str(after);
            }
            None => plain.push_str(word),
        }
    }
    if !plain.is_empty() {
        out.push(Fragment::Text { text: plain });
    }
}

/// The link in one word: what comes before it (an opening bracket or
/// quote), the link as typed, its parsed form, and what follows it
/// (punctuation that ends a sentence rather than the address).
fn find(word: &str) -> Option<(&str, &str, String, &str)> {
    let lower = word.to_ascii_lowercase();
    let at = lower.find("https://").or_else(|| lower.find("http://"))?;
    let (before, candidate) = word.split_at(at);
    // "(see https://…)" and "<https://…>" — but not "xhttps://".
    if !before.chars().all(|c| "([{<\"'".contains(c)) {
        return None;
    }
    let mut end = candidate.len();
    loop {
        let last = candidate[..end].chars().next_back()?;
        let keep = match last {
            '.' | ',' | ';' | ':' | '!' | '?' | '"' | '\'' | '>' => false,
            // A closing bracket belongs to the address only when the
            // address opened one, as Wikipedia's do.
            ')' => candidate[..end].matches('(').count() >= candidate[..end].matches(')').count(),
            ']' => candidate[..end].matches('[').count() >= candidate[..end].matches(']').count(),
            '}' => candidate[..end].matches('{').count() >= candidate[..end].matches('}').count(),
            _ => true,
        };
        if keep {
            break;
        }
        end -= last.len_utf8();
    }
    let link = &candidate[..end];
    let parsed = url::Url::parse(link).ok()?;
    if !matches!(parsed.scheme(), "http" | "https") || parsed.host_str().is_none_or(str::is_empty) {
        return None;
    }
    Some((before, link, parsed.to_string(), &candidate[end..]))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parts(text: &str) -> Vec<Fragment> {
        let mut out = Vec::new();
        split(text, &mut out);
        out
    }

    fn link(text: &str, url: &str) -> Fragment {
        Fragment::Link {
            text: text.into(),
            url: url.into(),
        }
    }

    fn plain(text: &str) -> Fragment {
        Fragment::Text { text: text.into() }
    }

    #[test]
    fn a_link_in_a_sentence() {
        assert_eq!(
            parts("check https://example.com/some/page for the schedule"),
            vec![
                plain("check "),
                link(
                    "https://example.com/some/page",
                    "https://example.com/some/page"
                ),
                plain(" for the schedule"),
            ]
        );
    }

    #[test]
    fn punctuation_after_a_link_is_not_part_of_it() {
        assert_eq!(
            parts("see (https://example.com/a)."),
            vec![
                plain("see ("),
                link("https://example.com/a", "https://example.com/a"),
                plain(")."),
            ]
        );
        // ... but a bracket the address opened is.
        assert_eq!(
            parts("https://en.wikipedia.org/wiki/Rust_(language)"),
            vec![link(
                "https://en.wikipedia.org/wiki/Rust_(language)",
                "https://en.wikipedia.org/wiki/Rust_(language)"
            )]
        );
    }

    #[test]
    fn the_url_has_the_host_in_punycode_and_the_text_is_kept() {
        // A Cyrillic "а" in place of the Latin one.
        assert_eq!(
            parts("https://pаypal.com/login"),
            vec![link(
                "https://pаypal.com/login",
                "https://xn--pypal-4ve.com/login"
            )]
        );
    }

    #[test]
    fn only_http_and_https_with_a_host() {
        for text in [
            "ftp://example.com",
            "javascript:alert(1)",
            "https://",
            "xhttps://example.com",
            "example.com",
            "http",
        ] {
            assert_eq!(parts(text), vec![plain(text)], "{text}");
        }
    }

    #[test]
    fn spacing_is_kept() {
        assert_eq!(
            parts("  a  http://x.cz  b "),
            vec![
                plain("  a  "),
                link("http://x.cz", "http://x.cz/"),
                plain("  b "),
            ]
        );
    }

    #[test]
    fn other_fragments_are_left_alone() {
        let fragments = marked(vec![
            plain("hi "),
            Fragment::Mention {
                text: "@x".into(),
                user: liveside_proto::model::User {
                    id: "1".into(),
                    login: "x".into(),
                    display_name: "x".into(),
                },
            },
            plain(" https://a.cz"),
        ]);
        assert_eq!(fragments.len(), 4);
        assert_eq!(fragments[3], link("https://a.cz", "https://a.cz/"));
    }
}
