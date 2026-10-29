//! Login with the OAuth Device Code Flow, and token storage.
//!
//! Device Code Flow needs no redirect URL and no client secret: the app asks
//! Twitch for a code, the user types it at twitch.tv/activate, and the app
//! polls until Twitch hands over a token. That works in a terminal and over
//! SSH, unlike a browser redirect to localhost.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::{Error, Result};

const DEVICE_URL: &str = "https://id.twitch.tv/oauth2/device";
const TOKEN_URL: &str = "https://id.twitch.tv/oauth2/token";
const VALIDATE_URL: &str = "https://id.twitch.tv/oauth2/validate";
const REVOKE_URL: &str = "https://id.twitch.tv/oauth2/revoke";

/// What the user has to do to finish logging in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceCode {
    /// Page to open, already containing the code where Twitch supports it.
    pub verification_uri: String,
    /// Code the user types at twitch.tv/activate.
    pub user_code: String,
    pub expires_in: u32,
    interval: u32,
    device_code: String,
}

/// Tokens as stored on disk.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tokens {
    pub access_token: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refresh_token: Option<String>,
    pub scopes: Vec<String>,
}

/// Who a token belongs to, as Twitch reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidToken {
    /// The application it was granted to. Twitch refuses it with any
    /// other Client ID.
    pub client_id: String,
    pub user_id: String,
    pub login: String,
    pub scopes: Vec<String>,
    /// Seconds until the token expires.
    pub expires_in: u64,
}

#[derive(Deserialize)]
struct DeviceResponse {
    device_code: String,
    expires_in: u32,
    interval: u32,
    user_code: String,
    verification_uri: String,
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    refresh_token: Option<String>,
    #[serde(default)]
    scope: Vec<String>,
}

#[derive(Deserialize)]
struct ValidateResponse {
    #[serde(default)]
    client_id: String,
    user_id: String,
    login: String,
    #[serde(default)]
    scopes: Vec<String>,
    expires_in: u64,
}

#[derive(Deserialize)]
struct OAuthError {
    message: Option<String>,
    error: Option<String>,
}

/// Talks to Twitch's identity service.
#[derive(Debug, Clone)]
pub struct Auth {
    http: reqwest::Client,
    client_id: String,
}

impl Auth {
    pub fn new(http: reqwest::Client, client_id: impl Into<String>) -> Self {
        Self {
            http,
            client_id: client_id.into(),
        }
    }

    /// Step one: ask Twitch for a code to show the user.
    pub async fn start_device_flow(&self, scopes: &[&str]) -> Result<DeviceCode> {
        let response = self
            .http
            .post(DEVICE_URL)
            .form(&[
                ("client_id", self.client_id.as_str()),
                ("scopes", &scopes.join(" ")),
            ])
            .send()
            .await?;
        let response: DeviceResponse = parse(response).await?;
        Ok(DeviceCode {
            verification_uri: response.verification_uri,
            user_code: response.user_code,
            expires_in: response.expires_in,
            interval: response.interval.max(1),
            device_code: response.device_code,
        })
    }

    /// Step two: poll until the user authorizes, the code expires, or the
    /// request is denied. Waits between attempts as Twitch asks.
    pub async fn poll_for_token(&self, code: &DeviceCode) -> Result<Tokens> {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(code.expires_in.into());
        loop {
            tokio::time::sleep(Duration::from_secs(code.interval.into())).await;
            if tokio::time::Instant::now() > deadline {
                return Err(Error::Auth("The login code expired".into()));
            }
            let response = self
                .http
                .post(TOKEN_URL)
                .form(&[
                    ("client_id", self.client_id.as_str()),
                    ("device_code", &code.device_code),
                    ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
                ])
                .send()
                .await?;
            if response.status().is_success() {
                let token: TokenResponse = response.json().await?;
                return Ok(Tokens {
                    access_token: token.access_token,
                    refresh_token: token.refresh_token,
                    scopes: token.scope,
                });
            }
            let message = oauth_error(response).await;
            // Twitch answers "authorization_pending" until the user is done.
            if message.contains("authorization_pending") || message.contains("pending") {
                continue;
            }
            if message.contains("slow_down") {
                tokio::time::sleep(Duration::from_secs(code.interval.into())).await;
                continue;
            }
            return Err(Error::Auth(message));
        }
    }

    /// Checks a token and reports who it belongs to. Also the cheapest way to
    /// notice an expired or revoked token.
    pub async fn validate(&self, access_token: &str) -> Result<ValidToken> {
        let response = self
            .http
            .get(VALIDATE_URL)
            .header("Authorization", format!("OAuth {access_token}"))
            .send()
            .await?;
        let response: ValidateResponse = parse(response).await?;
        Ok(ValidToken {
            client_id: response.client_id,
            user_id: response.user_id,
            login: response.login,
            scopes: response.scopes,
            expires_in: response.expires_in,
        })
    }

    /// Tells Twitch to forget a token. Deleting the file on this machine
    /// only hides it: the token would go on working until it expired,
    /// and everything it has subscribed to would go on running. Logging
    /// out should mean logged out, so this is asked for first.
    ///
    /// Twitch answers 200 for a token it no longer knows, and the caller
    /// is logging out either way, so a failure is worth saying and not
    /// worth stopping for.
    pub async fn revoke(&self, access_token: &str) -> Result<()> {
        let response = self
            .http
            .post(REVOKE_URL)
            .form(&[
                ("client_id", self.client_id.as_str()),
                ("token", access_token),
            ])
            .send()
            .await?;
        if response.status().is_success() {
            return Ok(());
        }
        Err(Error::Auth(oauth_error(response).await))
    }

    /// Exchanges a refresh token for a fresh access token. Public clients have
    /// no secret, so Twitch may refuse; the caller then logs in again.
    pub async fn refresh(&self, refresh_token: &str) -> Result<Tokens> {
        let response = self
            .http
            .post(TOKEN_URL)
            .form(&[
                ("client_id", self.client_id.as_str()),
                ("grant_type", "refresh_token"),
                ("refresh_token", refresh_token),
            ])
            .send()
            .await?;
        let response: TokenResponse = parse(response).await?;
        Ok(Tokens {
            access_token: response.access_token,
            refresh_token: response.refresh_token,
            scopes: response.scope,
        })
    }
}

async fn parse<T: serde::de::DeserializeOwned>(response: reqwest::Response) -> Result<T> {
    if response.status().is_success() {
        return Ok(response.json().await?);
    }
    Err(Error::Auth(oauth_error(response).await))
}

/// Twitch's identity service reports errors in `message` or `error`.
async fn oauth_error(response: reqwest::Response) -> String {
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    match serde_json::from_str::<OAuthError>(&body) {
        Ok(error) => error
            .message
            .or(error.error)
            .unwrap_or_else(|| format!("HTTP {status}")),
        Err(_) if body.is_empty() => format!("HTTP {status}"),
        Err(_) => format!("HTTP {status}: {body}"),
    }
}

/// Where tokens are kept: `$XDG_CONFIG_HOME/liveside/token.json`, readable only by
/// the user.
///
/// A token lets anyone chat as the account, so this file is as sensitive as a
/// password. Moving it into the system keyring is still open.
pub fn default_token_path() -> PathBuf {
    let dir = match std::env::var_os("XDG_CONFIG_HOME") {
        Some(config) if !config.is_empty() => PathBuf::from(config),
        _ => {
            let home = std::env::var_os("HOME").unwrap_or_default();
            PathBuf::from(home).join(".config")
        }
    };
    dir.join("liveside").join("token.json")
}

pub fn load_tokens(path: &Path) -> Result<Option<Tokens>> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(Some(serde_json::from_str(&text)?)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(Error::Storage(format!(
            "cannot read {}: {e}",
            path.display()
        ))),
    }
}

pub fn save_tokens(path: &Path, tokens: &Tokens) -> Result<()> {
    if let Some(dir) = path.parent() {
        create_private_dir(dir)?;
    }
    let text = serde_json::to_string_pretty(tokens)?;
    write_private(path, &text)
        .map_err(|e| Error::Storage(format!("cannot write {}: {e}", path.display())))
}

pub fn delete_tokens(path: &Path) -> Result<()> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(Error::Storage(format!(
            "cannot remove {}: {e}",
            path.display()
        ))),
    }
}

#[cfg(unix)]
fn create_private_dir(dir: &Path) -> Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)
        .or_else(ignore_exists)
        .map_err(|e| Error::Storage(format!("cannot create {}: {e}", dir.display())))
}

#[cfg(not(unix))]
fn create_private_dir(dir: &Path) -> Result<()> {
    std::fs::create_dir_all(dir)
        .map_err(|e| Error::Storage(format!("cannot create {}: {e}", dir.display())))
}

fn ignore_exists(e: std::io::Error) -> std::io::Result<()> {
    if e.kind() == std::io::ErrorKind::AlreadyExists {
        Ok(())
    } else {
        Err(e)
    }
}

/// Writes so that only the owner can read the file, from the moment it exists.
#[cfg(unix)]
fn write_private(path: &Path, text: &str) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?;
    file.write_all(text.as_bytes())
}

#[cfg(not(unix))]
fn write_private(path: &Path, text: &str) -> std::io::Result<()> {
    std::fs::write(path, text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_round_trip_through_a_private_file() {
        let dir = std::env::temp_dir().join(format!("liveside-auth-test-{}", std::process::id()));
        let path = dir.join("token.json");
        let tokens = Tokens {
            access_token: "abc".into(),
            refresh_token: Some("def".into()),
            scopes: vec!["user:read:chat".into()],
        };
        save_tokens(&path, &tokens).unwrap();
        assert_eq!(load_tokens(&path).unwrap(), Some(tokens));

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(
                mode & 0o777,
                0o600,
                "token file must not be readable by others"
            );
            let dir_mode = std::fs::metadata(&dir).unwrap().permissions().mode();
            assert_eq!(dir_mode & 0o777, 0o700);
        }

        delete_tokens(&path).unwrap();
        assert_eq!(load_tokens(&path).unwrap(), None);
        // Deleting what is not there is not an error.
        delete_tokens(&path).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }
}
