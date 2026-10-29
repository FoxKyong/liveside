//! Third-party emotes: 7TV, BetterTTV and FrankerFaceZ.
//!
//! Twitch only tells us about its own emotes, so a message that reads
//! "catJAM" to everyone else arrives as plain text. These providers each
//! serve a global set and a per-channel set; this module fetches them and
//! rewrites message text into emote fragments.
//!
//! Response shapes were checked against the live APIs on 2026-09-22.

use std::collections::HashMap;

use liveside_proto::model::{EmoteProvider, EmoteRef, Fragment};
use serde::Deserialize;

use crate::Result;

/// Emotes usable in one place (globally, or in one channel), by code.
///
/// Codes are case-sensitive, as on Twitch: `Kappa` is an emote, `kappa` is
/// not.
#[derive(Debug, Default, Clone)]
pub struct EmoteSet {
    by_code: HashMap<String, EmoteRef>,
}

impl EmoteSet {
    pub fn get(&self, code: &str) -> Option<&EmoteRef> {
        self.by_code.get(code)
    }

    pub fn len(&self) -> usize {
        self.by_code.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_code.is_empty()
    }

    /// Adds emotes that this set does not have yet, so the caller decides
    /// precedence by the order it merges.
    pub fn merge(&mut self, other: EmoteSet) {
        for (code, emote) in other.by_code {
            self.by_code.entry(code).or_insert(emote);
        }
    }

    fn insert(&mut self, emote: EmoteRef) {
        self.by_code.insert(emote.code.clone(), emote);
    }
}

/// Fetches emote sets. Cheap to clone.
#[derive(Debug, Clone)]
pub struct Emotes {
    http: reqwest::Client,
}

impl Emotes {
    pub fn new(http: reqwest::Client) -> Self {
        Self { http }
    }

    /// Global emotes of all three providers. A provider that fails is
    /// skipped and reported in the returned messages: chat must work even
    /// when 7TV is down.
    pub async fn global(&self) -> (EmoteSet, Vec<Problem>) {
        collect([
            ("7TV", self.seventv_global().await),
            ("BTTV", self.bttv_global().await),
            ("FFZ", self.ffz_global().await),
        ])
    }

    /// Emotes specific to one channel, in the same precedence order.
    pub async fn channel(&self, broadcaster_id: &str) -> (EmoteSet, Vec<Problem>) {
        collect([
            ("7TV", self.seventv_channel(broadcaster_id).await),
            ("BTTV", self.bttv_channel(broadcaster_id).await),
            ("FFZ", self.ffz_channel(broadcaster_id).await),
        ])
    }

    async fn get<T: serde::de::DeserializeOwned>(&self, url: &str) -> Result<T> {
        let response = self.http.get(url).send().await?;
        if !response.status().is_success() {
            return Err(crate::Error::Api(format!(
                "HTTP {} from {url}",
                response.status()
            )));
        }
        Ok(response.json().await?)
    }

    /// A channel that the provider does not know is not a problem: most
    /// channels have no FFZ room or no BTTV emotes, and saying so on every
    /// join would be noise.
    async fn optional<T: serde::de::DeserializeOwned + Default>(&self, url: &str) -> Result<T> {
        match self.get(url).await {
            Err(crate::Error::Api(message)) if message.starts_with("HTTP 404") => Ok(T::default()),
            other => other,
        }
    }

    async fn seventv_global(&self) -> Result<EmoteSet> {
        let set: SevenTvSet = self.get("https://7tv.io/v3/emote-sets/global").await?;
        Ok(set.into_emotes())
    }

    async fn seventv_channel(&self, broadcaster_id: &str) -> Result<EmoteSet> {
        let url = format!("https://7tv.io/v3/users/twitch/{broadcaster_id}");
        let user: SevenTvUser = self.optional(&url).await?;
        Ok(user
            .emote_set
            .map(SevenTvSet::into_emotes)
            .unwrap_or_default())
    }

    async fn bttv_global(&self) -> Result<EmoteSet> {
        let emotes: Vec<BttvEmote> = self
            .get("https://api.betterttv.net/3/cached/emotes/global")
            .await?;
        let mut set = EmoteSet::default();
        for emote in emotes {
            set.insert(emote.into_ref());
        }
        Ok(set)
    }

    async fn bttv_channel(&self, broadcaster_id: &str) -> Result<EmoteSet> {
        let url = format!("https://api.betterttv.net/3/cached/users/twitch/{broadcaster_id}");
        let user: BttvUser = self.optional(&url).await?;
        let mut set = EmoteSet::default();
        for emote in user.channel_emotes.into_iter().chain(user.shared_emotes) {
            set.insert(emote.into_ref());
        }
        Ok(set)
    }

    async fn ffz_global(&self) -> Result<EmoteSet> {
        let global: FfzGlobal = self
            .get("https://api.frankerfacez.com/v1/set/global")
            .await?;
        let mut set = EmoteSet::default();
        // Only the sets FFZ marks as default are shown everywhere.
        for id in &global.default_sets {
            if let Some(ffz_set) = global.sets.get(&id.to_string()) {
                ffz_set.add_to(&mut set);
            }
        }
        Ok(set)
    }

    async fn ffz_channel(&self, broadcaster_id: &str) -> Result<EmoteSet> {
        let url = format!("https://api.frankerfacez.com/v1/room/id/{broadcaster_id}");
        let room: FfzRoom = self.optional(&url).await?;
        let mut set = EmoteSet::default();
        for ffz_set in room.sets.values() {
            ffz_set.add_to(&mut set);
        }
        Ok(set)
    }
}

/// A provider that did not answer, named apart from its error so that the
/// sentence can be written in the user's language.
#[derive(Debug, Clone)]
pub struct Problem {
    pub provider: &'static str,
    pub error: String,
}

/// Merges what the providers returned, in the given order, and describes
/// what failed.
fn collect(results: [(&'static str, Result<EmoteSet>); 3]) -> (EmoteSet, Vec<Problem>) {
    let mut set = EmoteSet::default();
    let mut problems = Vec::new();
    for (provider, result) in results {
        match result {
            Ok(fetched) => set.merge(fetched),
            Err(e) => problems.push(Problem {
                provider,
                error: e.to_string(),
            }),
        }
    }
    (set, problems)
}

/// Rewrites text fragments into emote fragments. `sets` are searched in
/// order, so channel emotes can override global ones.
///
/// Emote codes are whole words, as on Twitch.
pub fn apply(fragments: Vec<Fragment>, sets: &[&EmoteSet]) -> Vec<Fragment> {
    if sets.iter().all(|set| set.is_empty()) {
        return fragments;
    }
    let mut out = Vec::with_capacity(fragments.len());
    for fragment in fragments {
        match fragment {
            Fragment::Text { text } => split_text(&text, sets, &mut out),
            other => out.push(other),
        }
    }
    out
}

fn split_text(text: &str, sets: &[&EmoteSet], out: &mut Vec<Fragment>) {
    let mut pending = String::new();
    for (i, word) in text.split(' ').enumerate() {
        if i > 0 {
            pending.push(' ');
        }
        match sets.iter().find_map(|set| set.get(word)) {
            Some(emote) => {
                if !pending.is_empty() {
                    out.push(Fragment::Text {
                        text: std::mem::take(&mut pending),
                    });
                }
                out.push(Fragment::Emote {
                    emote: emote.clone(),
                });
            }
            None => pending.push_str(word),
        }
    }
    if !pending.is_empty() {
        out.push(Fragment::Text { text: pending });
    }
}

/// URL of an emote image; see [`EmoteRef::image_url`], which holds the
/// templates so that frontends need not depend on this crate.
pub fn image_url(emote: &EmoteRef, scale: u8) -> Option<String> {
    emote.image_url(scale)
}

// --- provider payloads ---

#[derive(Deserialize, Default)]
struct SevenTvUser {
    #[serde(default)]
    emote_set: Option<SevenTvSet>,
}

#[derive(Deserialize)]
struct SevenTvSet {
    #[serde(default)]
    emotes: Vec<SevenTvEmote>,
}

#[derive(Deserialize)]
struct SevenTvEmote {
    id: String,
    name: String,
    /// Flags of this emote *in this set*; bit 0 marks a zero-width emote.
    #[serde(default)]
    flags: u32,
    #[serde(default)]
    data: Option<SevenTvData>,
}

#[derive(Deserialize)]
struct SevenTvData {
    #[serde(default)]
    animated: bool,
    /// Flags of the emote itself; bit 8 (256) marks zero-width.
    #[serde(default)]
    flags: u32,
}

impl SevenTvSet {
    fn into_emotes(self) -> EmoteSet {
        let mut set = EmoteSet::default();
        for emote in self.emotes {
            let data_flags = emote.data.as_ref().map(|d| d.flags).unwrap_or(0);
            set.insert(EmoteRef {
                provider: EmoteProvider::SevenTv,
                id: emote.id,
                code: emote.name,
                animated: emote.data.map(|d| d.animated).unwrap_or(false),
                zero_width: emote.flags & 1 != 0 || data_flags & 256 != 0,
            });
        }
        set
    }
}

#[derive(Deserialize, Default)]
struct BttvUser {
    #[serde(default, rename = "channelEmotes")]
    channel_emotes: Vec<BttvEmote>,
    #[serde(default, rename = "sharedEmotes")]
    shared_emotes: Vec<BttvEmote>,
}

#[derive(Deserialize)]
struct BttvEmote {
    id: String,
    code: String,
    #[serde(default)]
    animated: bool,
}

impl BttvEmote {
    fn into_ref(self) -> EmoteRef {
        EmoteRef {
            provider: EmoteProvider::Bttv,
            id: self.id,
            code: self.code,
            animated: self.animated,
            // BTTV does not mark zero-width emotes in its API; other clients
            // carry a hard-coded list of ids. Not worth copying yet.
            zero_width: false,
        }
    }
}

#[derive(Deserialize)]
struct FfzGlobal {
    #[serde(default)]
    default_sets: Vec<i64>,
    #[serde(default)]
    sets: HashMap<String, FfzSet>,
}

#[derive(Deserialize, Default)]
struct FfzRoom {
    #[serde(default)]
    sets: HashMap<String, FfzSet>,
}

#[derive(Deserialize)]
struct FfzSet {
    #[serde(default)]
    emoticons: Vec<FfzEmote>,
}

#[derive(Deserialize)]
struct FfzEmote {
    id: i64,
    name: String,
    /// Present (as a map of URLs) when the emote has an animated version.
    #[serde(default)]
    animated: Option<serde_json::Value>,
    /// FFZ "modifiers" are drawn over the previous emote.
    #[serde(default)]
    modifier: bool,
}

impl FfzSet {
    fn add_to(&self, set: &mut EmoteSet) {
        for emote in &self.emoticons {
            set.insert(EmoteRef {
                provider: EmoteProvider::Ffz,
                id: emote.id.to_string(),
                code: emote.name.clone(),
                animated: emote.animated.is_some(),
                zero_width: emote.modifier,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(emotes: &[(EmoteProvider, &str, &str)]) -> EmoteSet {
        let mut set = EmoteSet::default();
        for (provider, id, code) in emotes {
            set.insert(EmoteRef {
                provider: *provider,
                id: (*id).to_owned(),
                code: (*code).to_owned(),
                animated: false,
                zero_width: false,
            });
        }
        set
    }

    fn text(s: &str) -> Fragment {
        Fragment::Text { text: s.to_owned() }
    }

    fn rendered(fragments: &[Fragment]) -> String {
        fragments.iter().map(Fragment::text).collect()
    }

    #[test]
    fn rewrites_whole_words_only() {
        let sets = set(&[(EmoteProvider::SevenTv, "1", "catJAM")]);
        let out = apply(vec![text("hey catJAM catJAMMING catJAM")], &[&sets]);
        assert_eq!(rendered(&out), "hey catJAM catJAMMING catJAM");
        let emotes: Vec<_> = out
            .iter()
            .filter(|f| matches!(f, Fragment::Emote { .. }))
            .collect();
        assert_eq!(emotes.len(), 2, "{out:?}");
        // Codes are case-sensitive, like Twitch's.
        let out = apply(vec![text("catjam")], &[&sets]);
        assert!(matches!(out.as_slice(), [Fragment::Text { .. }]));
    }

    #[test]
    fn channel_emotes_win_over_global() {
        let channel = set(&[(EmoteProvider::Bttv, "chan", "Pog")]);
        let global = set(&[(EmoteProvider::SevenTv, "glob", "Pog")]);
        let out = apply(vec![text("Pog")], &[&channel, &global]);
        match &out[0] {
            Fragment::Emote { emote } => assert_eq!(emote.id, "chan"),
            other => panic!("expected an emote, got {other:?}"),
        }
    }

    #[test]
    fn other_fragments_are_untouched() {
        let sets = set(&[(EmoteProvider::Ffz, "1", "Kappa")]);
        let twitch_emote = Fragment::Emote {
            emote: EmoteRef {
                provider: EmoteProvider::Twitch,
                id: "25".into(),
                code: "Kappa".into(),
                animated: false,
                zero_width: false,
            },
        };
        let out = apply(vec![twitch_emote.clone(), text(" Kappa")], &[&sets]);
        assert_eq!(out[0], twitch_emote, "Twitch emotes stay Twitch's");
        assert!(matches!(&out[2], Fragment::Emote { emote } if emote.id == "1"));
    }

    #[test]
    fn empty_sets_change_nothing() {
        let empty = EmoteSet::default();
        let input = vec![text("nothing to do here")];
        assert_eq!(apply(input.clone(), &[&empty]), input);
    }

    #[test]
    fn parses_seventv() {
        let json = r#"{
            "emotes": [
                { "id": "01F", "name": "catJAM", "flags": 0,
                  "data": { "animated": true, "flags": 0 } },
                { "id": "02F", "name": "notL", "flags": 1,
                  "data": { "animated": false, "flags": 0 } },
                { "id": "03F", "name": "RainTime", "flags": 0,
                  "data": { "animated": true, "flags": 256 } }
            ]
        }"#;
        let set = serde_json::from_str::<SevenTvSet>(json)
            .unwrap()
            .into_emotes();
        assert_eq!(set.len(), 3);
        let cat = set.get("catJAM").unwrap();
        assert!(cat.animated && !cat.zero_width);
        assert!(set.get("notL").unwrap().zero_width, "set flag 1");
        assert!(set.get("RainTime").unwrap().zero_width, "emote flag 256");
    }

    #[test]
    fn parses_bttv_and_ffz() {
        let bttv = r#"{"id":"x","channelEmotes":[{"id":"b1","code":"Pog","imageType":"png","animated":false}],
                       "sharedEmotes":[{"id":"b2","code":"Wow","imageType":"gif","animated":true}]}"#;
        let user: BttvUser = serde_json::from_str(bttv).unwrap();
        let mut set = EmoteSet::default();
        for emote in user.channel_emotes.into_iter().chain(user.shared_emotes) {
            set.insert(emote.into_ref());
        }
        assert_eq!(set.len(), 2);
        assert!(set.get("Wow").unwrap().animated);

        let ffz = r#"{"default_sets":[3],"sets":{"3":{"emoticons":[
            {"id":9,"name":"ZrehplaR","modifier":false},
            {"id":10,"name":"ffzHyper","modifier":true,"animated":{"1":"https://x/1"}}
        ]}}}"#;
        let global: FfzGlobal = serde_json::from_str(ffz).unwrap();
        let mut set = EmoteSet::default();
        global.sets.get("3").unwrap().add_to(&mut set);
        assert_eq!(set.get("ZrehplaR").unwrap().id, "9");
        let hyper = set.get("ffzHyper").unwrap();
        assert!(hyper.animated && hyper.zero_width);
    }

    #[test]
    fn image_urls() {
        let emote = |provider, id: &str| EmoteRef {
            provider,
            id: id.to_owned(),
            code: "x".into(),
            animated: false,
            zero_width: false,
        };
        assert_eq!(
            image_url(&emote(EmoteProvider::Twitch, "25"), 2).unwrap(),
            "https://static-cdn.jtvnw.net/emoticons/v2/25/default/dark/2.0"
        );
        assert_eq!(
            image_url(&emote(EmoteProvider::SevenTv, "01F"), 3).unwrap(),
            "https://cdn.7tv.app/emote/01F/3x.webp"
        );
        assert_eq!(
            image_url(&emote(EmoteProvider::Bttv, "b1"), 1).unwrap(),
            "https://cdn.betterttv.net/emote/b1/1x"
        );
        assert_eq!(
            image_url(&emote(EmoteProvider::Ffz, "9"), 3).unwrap(),
            "https://cdn.frankerfacez.com/emote/9/4"
        );
        assert_eq!(image_url(&emote(EmoteProvider::Local, "x"), 1), None);
    }
}
