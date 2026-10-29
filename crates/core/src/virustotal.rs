//! Asking VirusTotal about links from chat.
//!
//! The reader's own API key, kept beside the Twitch token in
//! `$XDG_CONFIG_HOME/liveside/virustotal.key` (0600) rather than in
//! `config.toml`, which people copy and paste.
//!
//! The public API allows four requests a minute and 500 a day, and must not
//! be used in commercial products (VirusTotal's documentation, read
//! 2026-10-13), so this is a bring-your-own-key feature. A worker task asks
//! one question at a time and waits when the minute's four are spent; the
//! engine keeps the answers for the session.
//!
//! A lookup submits nothing. Submitting a link for a scan happens only when
//! the reader asks for that link, since a submitted URL becomes visible to
//! VirusTotal's paying customers. VirusTotal follows the redirects on its
//! own servers, so the report also says where the link ends without the
//! reader's address reaching the link's owner.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::time::Duration;

use liveside_proto::Timestamp;
use liveside_proto::model::{LinkReport, LinkSource, LinkState};
use tokio::sync::mpsc;
use tokio::time::Instant;

const API: &str = "https://www.virustotal.com/api/v3";

/// Requests allowed in a minute by the public API.
const PER_MINUTE: usize = 4;

/// How often a submitted scan is asked about, and how many times, before
/// the reader is told to ask again later. Each asking costs a request.
const SCAN_POLL: Duration = Duration::from_secs(20);
const SCAN_POLLS: usize = 6;

/// `$XDG_CONFIG_HOME/liveside/virustotal.key`, beside the settings and the
/// Twitch token.
pub fn default_key_path() -> PathBuf {
    crate::config::default_path().with_file_name("virustotal.key")
}

/// The key, if one is stored: the file's first line, trimmed.
pub fn read_key(path: &Path) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    let key = text.lines().next().unwrap_or_default().trim();
    (!key.is_empty()).then(|| key.to_owned())
}

/// Stores the key, readable only by the user; an empty key removes the
/// file.
pub fn write_key(path: &Path, key: &str) -> std::io::Result<()> {
    let key = key.trim();
    if key.is_empty() {
        return match std::fs::remove_file(path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            other => other,
        };
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?;
    // A file made earlier with wider permissions is narrowed too.
    use std::os::unix::fs::PermissionsExt;
    file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    writeln!(file, "{key}")
}

/// VirusTotal's identifier for a URL: the URL in base64url without padding.
pub fn url_id(url: &str) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let bytes = url.as_bytes();
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = chunk
            .iter()
            .enumerate()
            .fold(0u32, |n, (i, b)| n | (u32::from(*b) << (16 - 8 * i)));
        for i in 0..=chunk.len() {
            out.push(ALPHABET[(n >> (18 - 6 * i)) as usize & 63] as char);
        }
    }
    out
}

/// Reads a URL object (`GET /urls/{id}`) into what a frontend shows.
pub fn parse_report(body: &serde_json::Value) -> LinkState {
    let attributes = &body["data"]["attributes"];
    let stats = &attributes["last_analysis_stats"];
    let count = |name: &str| stats[name].as_u64().unwrap_or(0) as u32;
    let engines = [
        "malicious",
        "suspicious",
        "harmless",
        "undetected",
        "timeout",
    ]
    .iter()
    .map(|name| count(name))
    .sum();
    let url = attributes["url"].as_str().unwrap_or_default();
    let final_url = attributes["last_final_url"]
        .as_str()
        .filter(|end| *end != url)
        .map(str::to_owned);
    let redirects = attributes["redirection_chain"]
        .as_array()
        .map(|chain| {
            chain
                .iter()
                .filter_map(|hop| hop.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default();
    LinkState::Found {
        malicious: count("malicious"),
        suspicious: count("suspicious"),
        engines,
        analysed: attributes["last_analysis_date"]
            .as_i64()
            .map(|seconds| Timestamp(seconds * 1000)),
        final_url,
        redirects,
    }
}

/// What a refused request means, from VirusTotal's error code.
fn refusal(status: reqwest::StatusCode, body: &serde_json::Value) -> LinkState {
    let code = body["error"]["code"].as_str().unwrap_or_default();
    let message = body["error"]["message"]
        .as_str()
        .map_or_else(|| format!("HTTP {status}"), str::to_owned);
    let (code, detail) = match code {
        "WrongCredentialsError" | "AuthenticationRequiredError" => (
            "link.bad_key",
            "VirusTotal does not accept the API key".to_owned(),
        ),
        "QuotaExceededError" | "TooManyRequestsError" => (
            "link.quota",
            "VirusTotal's limit for this API key is used up for now".to_owned(),
        ),
        _ => ("link.failed", format!("VirusTotal refused it: {message}")),
    };
    LinkState::Failed {
        code: code.to_owned(),
        detail,
    }
}

/// A link to ask about.
#[derive(Debug)]
pub struct Job {
    pub url: String,
    pub scan: bool,
}

/// Starts the worker. Answers, intermediate ones included, come back on
/// `answers`. The key is read for every job, so one saved from the
/// settings works without a restart.
pub fn spawn(key_path: PathBuf, answers: mpsc::Sender<LinkReport>) -> mpsc::UnboundedSender<Job> {
    let (jobs_tx, mut jobs) = mpsc::unbounded_channel::<Job>();
    tokio::spawn(async move {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .unwrap_or_default();
        let mut worker = Worker {
            client,
            key_path,
            answers,
            asked: VecDeque::new(),
        };
        while let Some(job) = jobs.recv().await {
            let state = worker.check(&job).await;
            if !worker.answer(&job.url, state).await {
                break;
            }
        }
    });
    jobs_tx
}

struct Worker {
    client: reqwest::Client,
    key_path: PathBuf,
    answers: mpsc::Sender<LinkReport>,
    /// When the last few requests went out, to stay within the minute.
    asked: VecDeque<Instant>,
}

impl Worker {
    /// Sends an answer; false when the engine is gone.
    async fn answer(&self, url: &str, state: LinkState) -> bool {
        self.answers
            .send(LinkReport {
                url: url.to_owned(),
                source: LinkSource::VirusTotal,
                state,
            })
            .await
            .is_ok()
    }

    async fn check(&mut self, job: &Job) -> LinkState {
        let Some(key) = read_key(&self.key_path) else {
            return LinkState::Failed {
                code: "link.no_key".to_owned(),
                detail: format!(
                    "No VirusTotal API key; put yours in {}",
                    self.key_path.display()
                ),
            };
        };
        let id = url_id(&job.url);
        let (status, body) = match self.get(&key, &format!("{API}/urls/{id}")).await {
            Ok(answer) => answer,
            Err(state) => return state,
        };
        if status.is_success() {
            return parse_report(&body);
        }
        if status != reqwest::StatusCode::NOT_FOUND {
            return refusal(status, &body);
        }
        if !job.scan {
            return LinkState::Unknown;
        }
        self.scan(&key, job).await
    }

    /// Submits the link and waits for the scan to finish.
    async fn scan(&mut self, key: &str, job: &Job) -> LinkState {
        self.pace().await;
        let sent = self
            .client
            .post(format!("{API}/urls"))
            .header("x-apikey", key)
            .form(&[("url", job.url.as_str())])
            .send()
            .await;
        let (status, body) = match read(sent).await {
            Ok(answer) => answer,
            Err(state) => return state,
        };
        if !status.is_success() {
            return refusal(status, &body);
        }
        let Some(analysis) = body["data"]["id"].as_str().map(str::to_owned) else {
            return LinkState::Failed {
                code: "link.failed".to_owned(),
                detail: "VirusTotal did not say which scan it started".to_owned(),
            };
        };
        if !self.answer(&job.url, LinkState::Scanning).await {
            return LinkState::Scanning;
        }
        for _ in 0..SCAN_POLLS {
            tokio::time::sleep(SCAN_POLL).await;
            let (status, body) = match self.get(key, &format!("{API}/analyses/{analysis}")).await {
                Ok(answer) => answer,
                Err(state) => return state,
            };
            if !status.is_success() {
                return refusal(status, &body);
            }
            if body["data"]["attributes"]["status"].as_str() == Some("completed") {
                // The analysis has the verdicts but not the redirects; the
                // URL's own report has both.
                let id = url_id(&job.url);
                return match self.get(key, &format!("{API}/urls/{id}")).await {
                    Ok((status, body)) if status.is_success() => parse_report(&body),
                    Ok((status, body)) => refusal(status, &body),
                    Err(state) => state,
                };
            }
        }
        LinkState::Failed {
            code: "link.slow".to_owned(),
            detail: "VirusTotal is still scanning it; ask again in a while".to_owned(),
        }
    }

    async fn get(
        &mut self,
        key: &str,
        url: &str,
    ) -> Result<(reqwest::StatusCode, serde_json::Value), LinkState> {
        self.pace().await;
        read(self.client.get(url).header("x-apikey", key).send().await).await
    }

    /// Waits until another request fits into the minute.
    async fn pace(&mut self) {
        let minute = Duration::from_secs(60);
        while let Some(oldest) = self.asked.front() {
            if oldest.elapsed() >= minute {
                self.asked.pop_front();
            } else {
                break;
            }
        }
        if self.asked.len() >= PER_MINUTE
            && let Some(oldest) = self.asked.pop_front()
        {
            tokio::time::sleep_until(oldest + minute).await;
        }
        self.asked.push_back(Instant::now());
    }
}

async fn read(
    sent: reqwest::Result<reqwest::Response>,
) -> Result<(reqwest::StatusCode, serde_json::Value), LinkState> {
    let failed = |e: reqwest::Error| LinkState::Failed {
        code: "link.failed".to_owned(),
        detail: format!("Cannot reach VirusTotal: {e}"),
    };
    let response = sent.map_err(failed)?;
    let status = response.status();
    let body = response
        .json::<serde_json::Value>()
        .await
        .unwrap_or(serde_json::Value::Null);
    Ok((status, body))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_id_is_unpadded_base64url() {
        // As VirusTotal itself named a URL it did not know.
        assert_eq!(
            url_id("https://example.com/liveside-test-1791916413"),
            "aHR0cHM6Ly9leGFtcGxlLmNvbS9saXZlc2lkZS10ZXN0LTE3OTE5MTY0MTM"
        );
        assert_eq!(url_id("a"), "YQ");
        assert_eq!(url_id("ab"), "YWI");
        assert_eq!(url_id("abc"), "YWJj");
        assert_eq!(url_id("\u{fb}\u{ff}"), "w7vDvw");
    }

    // Trimmed from VirusTotal's answers on 2026-10-13.
    #[test]
    fn a_report_without_redirects() {
        let body = serde_json::json!({"data": {"attributes": {
            "url": "https://example.com/",
            "last_final_url": "https://example.com/",
            "redirection_chain": null,
            "last_analysis_date": 1791915746,
            "last_analysis_stats": {"malicious": 0, "suspicious": 0,
                "undetected": 31, "harmless": 62, "timeout": 0}
        }}});
        assert_eq!(
            parse_report(&body),
            LinkState::Found {
                malicious: 0,
                suspicious: 0,
                engines: 93,
                analysed: Some(Timestamp(1_791_915_746_000)),
                final_url: None,
                redirects: Vec::new(),
            }
        );
    }

    #[test]
    fn a_report_that_ends_elsewhere() {
        let body = serde_json::json!({"data": {"attributes": {
            "url": "http://github.com/",
            "last_final_url": "https://github.com/",
            "redirection_chain": ["http://github.com/", "https://github.com/"],
            "last_analysis_stats": {"malicious": 2, "suspicious": 1,
                "undetected": 30, "harmless": 60, "timeout": 0}
        }}});
        let LinkState::Found {
            malicious,
            suspicious,
            final_url,
            redirects,
            ..
        } = parse_report(&body)
        else {
            panic!("a report");
        };
        assert_eq!((malicious, suspicious), (2, 1));
        assert_eq!(final_url.as_deref(), Some("https://github.com/"));
        assert_eq!(redirects.len(), 2);
    }

    #[test]
    fn refusals_say_why() {
        let body = |code: &str| serde_json::json!({"error": {"code": code, "message": "x"}});
        let code_of = |state: LinkState| match state {
            LinkState::Failed { code, .. } => code,
            other => panic!("{other:?}"),
        };
        assert_eq!(
            code_of(refusal(
                reqwest::StatusCode::UNAUTHORIZED,
                &body("WrongCredentialsError")
            )),
            "link.bad_key"
        );
        assert_eq!(
            code_of(refusal(
                reqwest::StatusCode::TOO_MANY_REQUESTS,
                &body("QuotaExceededError")
            )),
            "link.quota"
        );
        assert_eq!(
            code_of(refusal(
                reqwest::StatusCode::BAD_REQUEST,
                &body("InvalidArgumentError")
            )),
            "link.failed"
        );
    }

    #[test]
    fn the_key_file_is_private_and_can_be_removed() {
        use std::os::unix::fs::PermissionsExt;
        let path = std::env::temp_dir().join(format!("liveside-vt-key-{}", std::process::id()));
        write_key(&path, "  abc123 \n").unwrap();
        assert_eq!(read_key(&path).as_deref(), Some("abc123"));
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        write_key(&path, "").unwrap();
        assert!(read_key(&path).is_none());
        assert!(!path.exists());
    }
}
