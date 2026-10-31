//! What can be said about a link from chat, to the reader and to the chat.
//!
//! The host is the part of an address that decides where a browser ends
//! up, and a host spelled with letters from another alphabet reaches the
//! client already in punycode (`xn--`), which makes a Cyrillic "а" in
//! "pаypal" visible. A moderator can tell the chat what was found, as a
//! reply to the message with the link, so everybody sees it at once.

use liveside_proto::model::LinkState;

use crate::lang::{Strings, fill, from_engine};

/// The host of an address: what follows the scheme, without a user name
/// before an `@` (`https://google.com@evil.example` goes to
/// `evil.example`), the port, the path or the rest.
pub fn host(address: &str) -> &str {
    let rest = address.split_once("://").map_or(address, |(_, rest)| rest);
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    let host = authority
        .rsplit_once('@')
        .map_or(authority, |(_, host)| host);
    if host.starts_with('[') {
        // An IPv6 address keeps its colons.
        return host
            .split_once(']')
            .map_or(host, |(inside, _)| inside.trim_start_matches('['));
    }
    host.split(':').next().unwrap_or_default()
}

/// Whether the browser would see a different host than the reader does:
/// the parsed address has punycode where the text has other letters.
pub fn looks_foreign(url: &str) -> bool {
    host(url).split('.').any(|label| label.starts_with("xn--"))
}

/// From how many of VirusTotal's engines finding something a link is
/// called dangerous. One or two engines alone flag well-known sites now
/// and then (google.com by one of 93, measured), and a warning given for
/// every such false alarm teaches the chat to pass over warnings.
pub const DANGEROUS_FROM: u32 = 3;

/// How much VirusTotal's engines found in a link.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Findings {
    /// Nothing.
    None,
    /// An engine or two, which is as often a false alarm as not.
    Few,
    /// Enough engines to take it seriously.
    Dangerous,
}

/// What `malicious` and `suspicious` findings out of a report come to.
pub fn findings(malicious: u32, suspicious: u32) -> Findings {
    match malicious + suspicious {
        0 => Findings::None,
        n if n < DANGEROUS_FROM => Findings::Few,
        _ => Findings::Dangerous,
    }
}

/// Whether two hosts are the same site: `www.` in front of one of them is
/// where a site sends its bare name, not somewhere else.
fn same_site(a: &str, b: &str) -> bool {
    let bare = |host: &str| {
        let host = host.to_ascii_lowercase();
        host.strip_prefix("www.").map(str::to_owned).unwrap_or(host)
    };
    bare(a) == bare(b)
}

/// A host as it is written into chat: with its dots in brackets, so that
/// nobody's client turns it into a link to click, and a channel that
/// does not allow links still takes the message.
pub fn defanged(host: &str) -> String {
    host.replace('.', "[.]")
}

/// The link a message to the chat is about: as it was written there, and
/// who wrote it.
#[derive(Debug, Clone, Copy)]
pub struct About<'a> {
    /// The link as the chat saw it, which for a look-alike is the
    /// imitation rather than the punycode.
    pub shown: &'a str,
    /// The login of the person whose message it was in.
    pub user: &'a str,
}

/// The most of a link written into a message to the chat. Twitch takes
/// 500 characters, and the link is there to be recognised, not copied.
const SHOWN_AT_MOST: usize = 160;

/// A link as it is written into chat: its host defanged, and a long one
/// cut short.
fn shown_link(shown: &str) -> String {
    let host = host(shown);
    let written = if host.is_empty() {
        shown.to_owned()
    } else {
        shown.replacen(host, &defanged(host), 1)
    };
    if written.chars().count() > SHOWN_AT_MOST {
        let cut: String = written.chars().take(SHOWN_AT_MOST).collect();
        format!("{cut}…")
    } else {
        written
    }
}

/// The whole message: a heading that says it is a check of a link, which
/// link and whose, then the finding; a warning sign first when something
/// is wrong, so that it stands out in a busy chat.
fn message(strings: &'static Strings, about: About, warn: bool, verdict: &str) -> String {
    let said = fill(
        strings.chat_link_heading,
        &[
            ("url", &shown_link(about.shown)),
            ("user", about.user),
            ("verdict", verdict),
        ],
    );
    if warn { format!("⚠ {said}") } else { said }
}

/// What to tell the chat about the letters of a link's address: that it
/// imitates a name with letters from other alphabets, and where it really
/// goes, or that it uses none. It says nothing about whether the site is
/// safe, which the letters cannot tell.
pub fn letters_for_chat(strings: &'static Strings, about: About, url: &str) -> String {
    let host = defanged(host(url));
    if looks_foreign(url) {
        let verdict = fill(strings.chat_link_foreign, &[("host", &host)]);
        message(strings, about, true, &verdict)
    } else {
        let verdict = fill(strings.chat_link_plain, &[("host", &host)]);
        message(strings, about, false, &verdict)
    }
}

/// What to do about telling the chat what VirusTotal says of a link.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ForChat {
    /// This is the answer to send.
    Say(String),
    /// No answer yet; asked, or to be asked.
    Wait,
    /// There will be no answer, for this reason, in the reader's language.
    Cannot(String),
}

/// What VirusTotal's answer about the link `url` comes to, for the chat.
pub fn virustotal_for_chat(
    strings: &'static Strings,
    about: About,
    url: &str,
    state: Option<&LinkState>,
) -> ForChat {
    match state {
        None | Some(LinkState::Checking | LinkState::Scanning | LinkState::Leads { .. }) => {
            ForChat::Wait
        }
        Some(LinkState::Unknown) => {
            ForChat::Say(message(strings, about, false, strings.chat_vt_unknown))
        }
        Some(LinkState::Failed { code, detail }) => {
            ForChat::Cannot(from_engine(strings, code, &[], detail))
        }
        Some(LinkState::Found {
            malicious,
            suspicious,
            engines,
            final_url,
            ..
        }) => {
            let engines = engines.to_string();
            let found = findings(*malicious, *suspicious);
            let mut verdict = match found {
                Findings::None => fill(strings.chat_vt_clean, &[("engines", &engines)]),
                Findings::Few => fill(
                    strings.chat_vt_doubtful,
                    &[
                        ("count", &(malicious + suspicious).to_string()),
                        ("engines", &engines),
                    ],
                ),
                Findings::Dangerous => fill(
                    strings.chat_vt_dangerous,
                    &[
                        ("malicious", &malicious.to_string()),
                        ("suspicious", &suspicious.to_string()),
                        ("engines", &engines),
                    ],
                ),
            };
            // Where it ends is the other half of the answer, when it is
            // another site: from http to https, or to www., is no news.
            if let Some(end) = final_url
                .as_deref()
                .filter(|end| !same_site(host(end), host(url)))
            {
                verdict.push_str(" · ");
                verdict.push_str(&fill(
                    strings.chat_vt_ends,
                    &[("host", &defanged(host(end)))],
                ));
            }
            // The advice comes last, after everything it is about.
            let dangerous = found == Findings::Dangerous;
            if dangerous {
                verdict = fill(strings.chat_vt_warn, &[("verdict", &verdict)]);
            }
            ForChat::Say(message(strings, about, dangerous, &verdict))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lang::Language;

    #[test]
    fn the_host_is_what_decides_where_it_goes() {
        assert_eq!(host("https://example.com/a?b#c"), "example.com");
        assert_eq!(host("https://example.com:8443/"), "example.com");
        assert_eq!(host("https://google.com@evil.example/x"), "evil.example");
        assert_eq!(host("http://user:pw@a.cz"), "a.cz");
        assert_eq!(host("http://[::1]:8080/"), "::1");
        assert_eq!(host("https://pаypal.com/login"), "pаypal.com");
    }

    #[test]
    fn punycode_is_noticed() {
        assert!(looks_foreign("https://xn--pypal-4ve.com/login"));
        assert!(!looks_foreign("https://paypal.com/login"));
        // Only in the host: a path may say anything.
        assert!(!looks_foreign("https://a.cz/xn--b"));
    }

    #[test]
    fn the_chat_is_told_where_a_lookalike_really_goes_without_a_link() {
        let strings = Language::English.strings();
        let about = About {
            shown: "https://pаypal.com/login",
            user: "scammer",
        };
        let said = letters_for_chat(strings, about, "https://xn--pypal-4ve.com/login");
        // Which link, as the chat saw it, whose, and where it really goes,
        // none of it clickable; a warning first.
        assert!(said.starts_with("⚠ Link check: "), "{said}");
        assert!(said.contains("https://pаypal[.]com/login"), "{said}");
        assert!(said.contains("@scammer"), "{said}");
        assert!(said.contains("xn--pypal-4ve[.]com"), "{said}");
        assert!(!said.contains("xn--pypal-4ve.com"), "{said}");
        let plain = About {
            shown: "https://paypal.com/login",
            user: "friend",
        };
        let said = letters_for_chat(strings, plain, "https://paypal.com/login");
        assert!(said.starts_with("Link check: "), "{said}");
        assert!(said.contains("paypal[.]com"), "{said}");
    }

    #[test]
    fn a_long_link_is_cut_short_for_the_chat() {
        let strings = Language::English.strings();
        let long = format!("https://example.com/{}", "a".repeat(600));
        let about = About {
            shown: &long,
            user: "someone",
        };
        let said = letters_for_chat(strings, about, &long);
        assert!(said.chars().count() < 500, "{}", said.chars().count());
        assert!(said.contains('…'), "{said}");
    }

    #[test]
    fn an_engine_or_two_is_no_warning() {
        let strings = Language::English.strings();
        let url = "https://google.com";
        let about = About {
            shown: url,
            user: "someone",
        };
        let report = |malicious| LinkState::Found {
            malicious,
            suspicious: 0,
            engines: 93,
            analysed: None,
            final_url: Some("https://www.google.com/".into()),
            redirects: Vec::new(),
        };
        let ForChat::Say(said) = virustotal_for_chat(strings, about, url, Some(&report(1))) else {
            panic!("a report is told");
        };
        assert!(!said.starts_with('⚠'), "{said}");
        assert!(said.contains("false alarm"), "{said}");
        // To www. is the same site, and not worth a word.
        assert!(!said.contains("ends at"), "{said}");
        let ForChat::Say(said) = virustotal_for_chat(strings, about, url, Some(&report(3))) else {
            panic!("a report is told");
        };
        assert!(said.starts_with("⚠ "), "{said}");
        // The advice is the last thing said.
        assert!(said.ends_with("Better not open it."), "{said}");
    }

    #[test]
    fn only_a_finished_check_is_told() {
        let strings = Language::English.strings();
        let url = "https://short.example/a";
        let about = About {
            shown: url,
            user: "someone",
        };
        assert_eq!(
            virustotal_for_chat(strings, about, url, None),
            ForChat::Wait
        );
        assert_eq!(
            virustotal_for_chat(strings, about, url, Some(&LinkState::Checking)),
            ForChat::Wait
        );
        let found = LinkState::Found {
            malicious: 3,
            suspicious: 1,
            engines: 90,
            analysed: None,
            final_url: Some("https://evil.example/x".into()),
            redirects: Vec::new(),
        };
        let ForChat::Say(said) = virustotal_for_chat(strings, about, url, Some(&found)) else {
            panic!("a report is told");
        };
        assert!(said.starts_with("⚠ "), "{said}");
        assert!(
            said.contains("short[.]example") && said.contains("@someone"),
            "{said}"
        );
        assert!(said.contains('3') && said.contains("90"), "{said}");
        assert!(said.contains("evil[.]example"), "{said}");
        // Only from http to https on the same host: nothing to add.
        let ForChat::Say(said) =
            virustotal_for_chat(strings, about, "http://evil.example/x", Some(&found))
        else {
            panic!("a report is told");
        };
        assert!(!said.contains("evil[.]example"), "{said}");
        let failed = LinkState::Failed {
            code: "link.quota".into(),
            detail: "quota".into(),
        };
        assert!(matches!(
            virustotal_for_chat(strings, about, url, Some(&failed)),
            ForChat::Cannot(_)
        ));
    }
}
