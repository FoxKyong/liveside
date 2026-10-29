//! Identifiers and small value types.

use std::fmt;

use serde::{Deserialize, Serialize};

macro_rules! string_id {
    ($(#[$doc:meta])* $name:ident) => {
        $(#[$doc])*
        #[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(pub String);

        impl $name {
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl From<&str> for $name {
            fn from(s: &str) -> Self {
                Self(s.to_owned())
            }
        }

        impl From<String> for $name {
            fn from(s: String) -> Self {
                Self(s)
            }
        }
    };
}

string_id!(
    /// Twitch user ID (numeric string). Channels are identified by their
    /// broadcaster's user ID, which, unlike the login, never changes.
    UserId
);

string_id!(
    /// Twitch login: lowercase account name as used in URLs, e.g. `joshimuz`.
    Login
);

string_id!(
    /// Twitch chat message ID (UUID).
    MessageId
);

/// Identifies a request sent by a frontend, so the engine's response can be
/// matched to it. Unique per frontend connection, chosen by the frontend.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RequestId(pub u64);

/// Position in the engine's event stream. Strictly increasing; a frontend
/// that reattaches uses it to know which events it has already seen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Seq(pub u64);

/// Milliseconds since the Unix epoch, UTC.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Timestamp(pub i64);

impl Timestamp {
    /// The current time.
    pub fn now() -> Self {
        let since_epoch = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default();
        Self(since_epoch.as_millis() as i64)
    }
}

/// RGB colour. Serialized as `"#rrggbb"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Rgb {
    /// Parses `#rrggbb` (the `#` is optional). Returns `None` for anything
    /// else, including Twitch's empty string for users without a colour.
    pub fn parse_hex(s: &str) -> Option<Self> {
        let hex = s.strip_prefix('#').unwrap_or(s);
        if hex.len() != 6 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        let byte = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).ok();
        Some(Self {
            r: byte(0)?,
            g: byte(2)?,
            b: byte(4)?,
        })
    }
}

impl fmt::Display for Rgb {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "#{:02x}{:02x}{:02x}", self.r, self.g, self.b)
    }
}

impl Serialize for Rgb {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for Rgb {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = <std::borrow::Cow<'de, str>>::deserialize(deserializer)?;
        Rgb::parse_hex(&s).ok_or_else(|| {
            serde::de::Error::custom(format!("invalid colour {s:?}, expected #rrggbb"))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rgb_parses_and_prints() {
        let c = Rgb::parse_hex("#1E90FF").unwrap();
        assert_eq!(
            c,
            Rgb {
                r: 0x1e,
                g: 0x90,
                b: 0xff
            }
        );
        assert_eq!(c.to_string(), "#1e90ff");
        assert_eq!(Rgb::parse_hex("1e90ff"), Some(c));
    }

    #[test]
    fn rgb_rejects_invalid() {
        for s in ["", "#", "#12345", "#1234567", "#12345g", "#ééé"] {
            assert_eq!(Rgb::parse_hex(s), None, "{s:?}");
        }
    }

    #[test]
    fn rgb_serde_roundtrip() {
        let c = Rgb { r: 1, g: 2, b: 255 };
        let json = serde_json::to_string(&c).unwrap();
        assert_eq!(json, r##""#0102ff""##);
        assert_eq!(serde_json::from_str::<Rgb>(&json).unwrap(), c);
        assert!(serde_json::from_str::<Rgb>(r#""red""#).is_err());
    }
}
