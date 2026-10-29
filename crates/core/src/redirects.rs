//! Following a link's redirects from this machine, without opening it.
//!
//! What it protects against is the browser rendering a page; what it does
//! not protect is the reader's address, which every host on the way sees.
//! The settings say so, and it is off by default. See
//! `docs/architecture.md`, section *Links*.
//!
//! The rules:
//!
//! - `HEAD`, and `GET` only when a server refuses that. Only the status and
//!   `Location` are read; a body is never read.
//! - No cookies, no referrer, no proxy, no automatic redirects: each hop
//!   is a request of its own, so each is checked.
//! - At most [`MAX_HOPS`] redirects and [`HOP_TIMEOUT`] for each.
//! - Only `http` and `https`; a `Location` elsewhere ends it.
//! - No address that is never on the internet: loopback, private,
//!   link-local, unique-local, multicast and the like are refused. The
//!   check is made in the DNS resolver the HTTP client connects through,
//!   so the address checked is the address connected to and DNS cannot
//!   hand over a different one in between. A link in chat must not be able
//!   to make the reader's machine talk to their router.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use liveside_proto::model::LinkState;

/// Redirects followed before giving up.
pub const MAX_HOPS: usize = 10;

/// How long one request may take.
pub const HOP_TIMEOUT: Duration = Duration::from_secs(5);

/// Whether an address is somewhere on the internet rather than in the
/// private or local network, or a reserved range. Judged by the range
/// alone: which networks the reader's router can reach is not something
/// this machine can know, so no private address is followed at all.
pub fn is_public(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => is_public_v4(v4),
        IpAddr::V6(v6) => {
            // An IPv4 address written as IPv6 is judged as what it is.
            if let Some(v4) = v6.to_ipv4_mapped() {
                return is_public_v4(v4);
            }
            let first = v6.segments()[0];
            // NAT64 (64:ff9b::/96) carries an IPv4 address in its tail.
            if v6.segments()[..6] == [0x64, 0xff9b, 0, 0, 0, 0] {
                let [.., a, b, c, d] = v6.octets();
                return is_public_v4(Ipv4Addr::new(a, b, c, d));
            }
            !(v6.is_unspecified()
                || v6.is_loopback()
                || v6.is_multicast()
                || first & 0xfe00 == 0xfc00 // unique local, fc00::/7
                || first & 0xffc0 == 0xfe80 // link-local, fe80::/10
                || first & 0xffc0 == 0xfec0 // site-local, fec0::/10
                || first == 0x2001 && v6.segments()[1] == 0x0db8) // documentation
        }
    }
}

fn is_public_v4(ip: Ipv4Addr) -> bool {
    let [a, b, ..] = ip.octets();
    !(ip.is_unspecified()
        || ip.is_loopback()
        || ip.is_private()
        || ip.is_link_local()
        || ip.is_broadcast()
        || ip.is_multicast()
        || ip.is_documentation()
        || a == 0 // "this network"
        || a >= 240 // reserved
        || (a == 100 && (64..128).contains(&b)) // carrier-grade NAT
        || (a == 198 && (b == 18 || b == 19))) // benchmarking
}

/// Refused because it is not an address on the internet.
#[derive(Debug)]
struct NotPublic(String);

impl std::fmt::Display for NotPublic {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} is not an address on the internet", self.0)
    }
}

impl std::error::Error for NotPublic {}

/// Resolves names as the system does, then keeps only public addresses.
struct PublicOnly;

impl reqwest::dns::Resolve for PublicOnly {
    fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        let host = name.as_str().to_owned();
        Box::pin(async move {
            let found: Vec<SocketAddr> =
                tokio::net::lookup_host((host.as_str(), 0)).await?.collect();
            let public: Vec<SocketAddr> = found
                .into_iter()
                .filter(|addr| is_public(addr.ip()))
                .collect();
            if public.is_empty() {
                return Err(Box::new(NotPublic(host)) as Box<dyn std::error::Error + Send + Sync>);
            }
            Ok(Box::new(public.into_iter()) as reqwest::dns::Addrs)
        })
    }
}

/// Follows links. One client for every link, so connections are reused
/// where a shortener sends many links to one place.
pub struct Follower {
    client: reqwest::Client,
    /// Tests talk to a server on this machine; nothing else may.
    allow_private: bool,
}

impl Default for Follower {
    fn default() -> Self {
        Self::new(false)
    }
}

impl Follower {
    fn new(allow_private: bool) -> Self {
        let mut builder = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .referer(false)
            .no_proxy()
            .timeout(HOP_TIMEOUT)
            .user_agent(concat!(
                "Liveside/",
                env!("CARGO_PKG_VERSION"),
                " (link check)"
            ));
        if !allow_private {
            builder = builder.dns_resolver(Arc::new(PublicOnly));
        }
        Self {
            client: builder.build().unwrap_or_default(),
            allow_private,
        }
    }

    /// Where `url` leads.
    pub async fn follow(&self, url: &str) -> LinkState {
        let mut current = match url::Url::parse(url) {
            Ok(parsed) => parsed,
            Err(e) => return failed("link.failed", format!("Not an address: {e}")),
        };
        let mut redirects = vec![current.to_string()];
        for _ in 0..=MAX_HOPS {
            if !matches!(current.scheme(), "http" | "https") {
                return failed(
                    "link.scheme",
                    format!("It leads to a {} address, not a web page", current.scheme()),
                );
            }
            // An address written into the link needs no DNS, so the
            // resolver never sees it.
            let literal = match current.host() {
                Some(url::Host::Ipv4(ip)) => Some(IpAddr::V4(ip)),
                Some(url::Host::Ipv6(ip)) => Some(IpAddr::V6(ip)),
                _ => None,
            };
            if let Some(ip) = literal
                && !self.allow_private
                && !is_public(ip)
            {
                return refused(&ip.to_string());
            }
            let status = match self.ask(&current).await {
                Ok(answer) => answer,
                Err(state) => return state,
            };
            let (status, location) = status;
            let next = location
                .filter(|_| status.is_redirection())
                .and_then(|location| current.join(&location).ok());
            match next {
                Some(next) => {
                    redirects.push(next.to_string());
                    current = next;
                }
                None => {
                    let end = current.to_string();
                    let moved = redirects.len() > 1;
                    return LinkState::Leads {
                        final_url: moved.then_some(end),
                        redirects: if moved { redirects } else { Vec::new() },
                        status: status.as_u16(),
                    };
                }
            }
        }
        failed(
            "link.too_many",
            format!("More than {MAX_HOPS} redirects; not followed further"),
        )
    }

    /// One hop: the status and where it points, if anywhere.
    async fn ask(
        &self,
        url: &url::Url,
    ) -> Result<(reqwest::StatusCode, Option<String>), LinkState> {
        let mut answer = self.request(reqwest::Method::HEAD, url).await?;
        // Some servers refuse HEAD, or answer it differently; GET is asked
        // then, and its body is still never read.
        if answer.0.is_client_error() || answer.0.is_server_error() {
            answer = self.request(reqwest::Method::GET, url).await?;
        }
        Ok(answer)
    }

    async fn request(
        &self,
        method: reqwest::Method,
        url: &url::Url,
    ) -> Result<(reqwest::StatusCode, Option<String>), LinkState> {
        let response = self
            .client
            .request(method, url.as_str())
            .send()
            .await
            .map_err(|e| explain(&e))?;
        let location = response
            .headers()
            .get(reqwest::header::LOCATION)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        // Dropped here, unread.
        Ok((response.status(), location))
    }
}

fn failed(code: &str, detail: String) -> LinkState {
    LinkState::Failed {
        code: code.to_owned(),
        detail,
    }
}

fn refused(host: &str) -> LinkState {
    failed(
        "link.private",
        format!("It leads to a private address ({host}, a local network); not followed"),
    )
}

/// Says why a request failed, looking through the error's causes for the
/// resolver's refusal.
fn explain(error: &reqwest::Error) -> LinkState {
    let mut cause: Option<&(dyn std::error::Error + 'static)> = Some(error);
    while let Some(e) = cause {
        if let Some(NotPublic(host)) = e.downcast_ref::<NotPublic>() {
            return refused(host);
        }
        cause = e.source();
    }
    if error.is_timeout() {
        return failed(
            "link.timeout",
            format!("No answer within {} seconds", HOP_TIMEOUT.as_secs()),
        );
    }
    failed("link.failed", format!("Cannot reach it: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[test]
    fn the_own_network_is_not_public() {
        for ip in [
            "127.0.0.1",
            "10.1.2.3",
            "172.16.0.1",
            "192.168.1.1",
            "169.254.169.254",
            "100.64.0.1",
            "0.0.0.0",
            "255.255.255.255",
            "224.0.0.1",
            "::1",
            "::",
            "fd00::1",
            "fe80::1",
            "ff02::1",
            "::ffff:192.168.1.1",
            "64:ff9b::a00:1",
        ] {
            assert!(!is_public(ip.parse().unwrap()), "{ip}");
        }
        for ip in [
            "1.1.1.1",
            "93.184.215.14",
            "2606:4700::6810:85e5",
            "::ffff:8.8.8.8",
        ] {
            assert!(is_public(ip.parse().unwrap()), "{ip}");
        }
    }

    #[tokio::test]
    async fn an_address_in_the_link_itself_is_refused() {
        let state = Follower::default().follow("http://192.168.1.1/admin").await;
        assert!(
            matches!(&state, LinkState::Failed { code, .. } if code == "link.private"),
            "{state:?}"
        );
    }

    #[tokio::test]
    async fn a_name_that_resolves_into_the_own_network_is_refused() {
        // localhost comes from /etc/hosts: no network needed.
        let state = Follower::default().follow("http://localhost:9/").await;
        assert!(
            matches!(&state, LinkState::Failed { code, .. } if code == "link.private"),
            "{state:?}"
        );
    }

    #[tokio::test]
    async fn other_schemes_end_it() {
        let state = Follower::default().follow("ftp://example.com/").await;
        assert!(
            matches!(&state, LinkState::Failed { code, .. } if code == "link.scheme"),
            "{state:?}"
        );
    }

    /// A server on this machine answering each request with the next of
    /// `answers`, and noting the request lines.
    async fn server(answers: Vec<&'static str>) -> (String, tokio::task::JoinHandle<Vec<String>>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let task = tokio::spawn(async move {
            let mut seen = Vec::new();
            for answer in answers {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut buf = vec![0; 2048];
                let n = socket.read(&mut buf).await.unwrap();
                let request = String::from_utf8_lossy(&buf[..n]).to_string();
                seen.push(request.lines().next().unwrap_or_default().to_owned());
                assert!(!request.to_ascii_lowercase().contains("\r\nreferer:"));
                assert!(!request.to_ascii_lowercase().contains("\r\ncookie:"));
                socket.write_all(answer.as_bytes()).await.unwrap();
                socket.shutdown().await.ok();
            }
            seen
        });
        (base, task)
    }

    #[tokio::test]
    async fn redirects_are_followed_one_by_one_and_the_body_is_left() {
        let (base, task) = server(vec![
            "HTTP/1.1 301 Moved\r\nLocation: /next\r\nConnection: close\r\nContent-Length: 0\r\n\r\n",
            // HEAD refused: asked again with GET.
            "HTTP/1.1 405 No\r\nConnection: close\r\nContent-Length: 0\r\n\r\n",
            "HTTP/1.1 302 Found\r\nLocation: /end\r\nConnection: close\r\nContent-Length: 5\r\n\r\nhello",
            "HTTP/1.1 200 OK\r\nConnection: close\r\nContent-Length: 0\r\n\r\n",
        ])
        .await;
        let state = Follower::new(true).follow(&format!("{base}/start")).await;
        assert_eq!(
            state,
            LinkState::Leads {
                final_url: Some(format!("{base}/end")),
                redirects: vec![
                    format!("{base}/start"),
                    format!("{base}/next"),
                    format!("{base}/end")
                ],
                status: 200,
            }
        );
        assert_eq!(
            task.await.unwrap(),
            [
                "HEAD /start HTTP/1.1",
                "HEAD /next HTTP/1.1",
                "GET /next HTTP/1.1",
                "HEAD /end HTTP/1.1"
            ]
        );
    }

    #[tokio::test]
    async fn a_link_that_stays_put_says_so() {
        let (base, task) = server(vec![
            "HTTP/1.1 200 OK\r\nConnection: close\r\nContent-Length: 0\r\n\r\n",
        ])
        .await;
        let state = Follower::new(true).follow(&format!("{base}/")).await;
        assert_eq!(
            state,
            LinkState::Leads {
                final_url: None,
                redirects: Vec::new(),
                status: 200
            }
        );
        task.await.unwrap();
    }

    #[tokio::test]
    async fn a_loop_is_given_up() {
        let answer = "HTTP/1.1 302 Found\r\nLocation: /again\r\nConnection: close\r\nContent-Length: 0\r\n\r\n";
        let (base, task) = server(vec![answer; MAX_HOPS + 1]).await;
        let state = Follower::new(true).follow(&format!("{base}/again")).await;
        assert!(
            matches!(&state, LinkState::Failed { code, .. } if code == "link.too_many"),
            "{state:?}"
        );
        task.await.unwrap();
    }
}
