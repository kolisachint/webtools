use reqwest::header::{CONTENT_TYPE, LOCATION};
use reqwest::{redirect::Policy, Client, Response};
use std::net::SocketAddr;
use std::time::Duration;

use crate::guard;
use crate::tls::TlsConfig;

const USER_AGENT: &str = concat!("webfetch/", env!("CARGO_PKG_VERSION"));
const MAX_ATTEMPTS: u32 = 3;
const MAX_REDIRECTS: usize = 5;

/// Hard cap on the response body we will read (5 MiB). The HTML extractor turns
/// a page into a few KB of text, so a multi-megabyte body is almost never worth
/// the bandwidth, memory, and parse time — and an unbounded read is a DoS lever.
/// Bodies over the cap are *truncated* (not errored): partial content is still
/// useful and the extractor copes with truncated HTML.
const MAX_BODY_BYTES: usize = 5 * 1024 * 1024;

/// Outcome of an HTTP fetch: the body, the URL we actually landed on after
/// following redirects, and the response's `Content-Type` (if any).
pub struct FetchedPage {
    pub body: String,
    pub final_url: String,
    pub content_type: Option<String>,
}

/// One hop's result: either the final page, or a redirect to a raw `Location`.
enum Hop {
    Page(FetchedPage),
    Redirect(String),
}

/// Build a client for a single validated URL. `pinned` are the public IPs the
/// host already resolved to; binding them closes the DNS-rebinding window
/// between validation and connection.
///
/// Redirects are **not** followed by reqwest here ([`Policy::none`]): we follow
/// them manually in [`fetch_page`] so every hop is re-validated *and* pinned to
/// its own resolved addresses. (Reqwest's `resolve_to_addrs` pins only the
/// hosts known at build time, so auto-follow would leave redirect hops
/// unpinned.) A consequence is that connection pooling cannot be shared across
/// hosts via one long-lived client without weakening per-URL IP pinning, so we
/// deliberately do not cache clients — SSRF safety wins over pool reuse.
fn build_client(
    url: &reqwest::Url,
    timeout_secs: u64,
    pinned: &[SocketAddr],
    tls: &TlsConfig,
) -> anyhow::Result<Client> {
    let mut builder = Client::builder()
        .timeout(Duration::from_secs(timeout_secs))
        .redirect(Policy::none())
        .user_agent(USER_AGENT)
        .gzip(true)
        .brotli(true);

    // Trust the OS store (+ SSL_CERT_FILE / --ca-cert) so org/proxy root CAs
    // are accepted, instead of only the bundled webpki roots.
    builder = tls.apply(builder)?;

    if let Some(host) = url.host_str() {
        if !pinned.is_empty() {
            builder = builder.resolve_to_addrs(host, pinned);
        }
    }
    Ok(builder.build()?)
}

/// Append as much of `chunk` to `buf` as fits under `max`. Returns `true` once
/// the cap is reached (the body is truncated and the caller should stop).
fn push_capped(buf: &mut Vec<u8>, chunk: &[u8], max: usize) -> bool {
    let remaining = max.saturating_sub(buf.len());
    if chunk.len() >= remaining {
        buf.extend_from_slice(&chunk[..remaining]);
        true
    } else {
        buf.extend_from_slice(chunk);
        false
    }
}

/// Read a response body, streaming chunks with a running byte cap so an
/// oversized body is bounded before it is ever DOM-parsed. The `bool` reports
/// whether a read error is transient (worth retrying).
async fn read_body_capped(mut resp: Response) -> Result<String, (anyhow::Error, bool)> {
    let mut buf: Vec<u8> = Vec::new();
    // Honour Content-Length to pre-size, but never trust it past the cap.
    if let Some(len) = resp.content_length() {
        buf.reserve(len.min(MAX_BODY_BYTES as u64) as usize);
    }
    loop {
        match resp.chunk().await {
            Ok(Some(chunk)) => {
                if push_capped(&mut buf, &chunk, MAX_BODY_BYTES) {
                    break;
                }
            }
            Ok(None) => break,
            Err(e) => {
                let transient = e.is_timeout();
                return Err((e.into(), transient));
            }
        }
    }
    Ok(String::from_utf8_lossy(&buf).into_owned())
}

/// One request attempt. The bool in the error reports whether the failure is
/// transient (worth retrying): connection/timeout errors, 5xx, and 429.
async fn attempt(client: &Client, url: &str) -> Result<Hop, (anyhow::Error, bool)> {
    let resp = match client.get(url).send().await {
        Ok(r) => r,
        Err(e) => {
            let transient = e.is_timeout() || e.is_connect() || e.is_request();
            return Err((e.into(), transient));
        }
    };

    let status = resp.status();

    // Redirects are surfaced to the caller (which re-validates and pins the
    // target) rather than followed by reqwest.
    if status.is_redirection() {
        return match resp.headers().get(LOCATION).and_then(|v| v.to_str().ok()) {
            Some(loc) => Ok(Hop::Redirect(loc.to_string())),
            None => Err((
                anyhow::anyhow!("redirect ({status}) without a Location header"),
                false,
            )),
        };
    }

    let resp = match resp.error_for_status() {
        Ok(r) => r,
        Err(e) => {
            let transient = status.is_server_error() || status.as_u16() == 429;
            return Err((e.into(), transient));
        }
    };

    let final_url = resp.url().to_string();
    let content_type = resp
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    let body = read_body_capped(resp).await?;
    Ok(Hop::Page(FetchedPage {
        body,
        final_url,
        content_type,
    }))
}

/// Issue one hop's request, retrying transient failures with exponential
/// backoff (200ms, 400ms).
async fn fetch_with_retries(client: &Client, url: &str) -> anyhow::Result<Hop> {
    let mut delay = Duration::from_millis(200);
    for attempt_no in 1..=MAX_ATTEMPTS {
        match attempt(client, url).await {
            Ok(hop) => return Ok(hop),
            Err((err, transient)) => {
                if attempt_no == MAX_ATTEMPTS || !transient {
                    return Err(err);
                }
                tokio::time::sleep(delay).await;
                delay *= 2;
            }
        }
    }
    unreachable!("loop returns on the final attempt")
}

/// Fetch a URL, following redirects manually so the SSRF guard re-validates and
/// re-pins each hop (closing the DNS-rebinding window for redirected hosts too),
/// retrying transient failures with exponential backoff. Caps the redirect
/// chain at [`MAX_REDIRECTS`] and the body at [`MAX_BODY_BYTES`].
pub async fn fetch_page(
    url: &str,
    timeout_secs: u64,
    tls: &TlsConfig,
) -> anyhow::Result<FetchedPage> {
    let mut current = reqwest::Url::parse(url)?;
    let mut hops = 0usize;

    loop {
        // Validate + resolve the host for THIS hop, then pin the connection to
        // exactly those addresses.
        let pinned = guard::validate_url(&current).await?;
        let client = build_client(&current, timeout_secs, &pinned, tls)?;

        match fetch_with_retries(&client, current.as_str()).await? {
            Hop::Page(page) => return Ok(page),
            Hop::Redirect(location) => {
                hops += 1;
                if hops > MAX_REDIRECTS {
                    anyhow::bail!("too many redirects (>{MAX_REDIRECTS})");
                }
                current = current
                    .join(&location)
                    .map_err(|e| anyhow::anyhow!("invalid redirect target `{location}`: {e}"))?;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_capped_truncates_oversized_chunk() {
        let mut buf = Vec::new();
        // A single chunk larger than the cap is clipped to the cap.
        let stopped = push_capped(&mut buf, &[b'x'; 10], 4);
        assert!(stopped);
        assert_eq!(buf.len(), 4);
    }

    #[test]
    fn push_capped_accumulates_until_cap() {
        let mut buf = Vec::new();
        assert!(!push_capped(&mut buf, b"abc", 8));
        assert!(!push_capped(&mut buf, b"de", 8));
        assert_eq!(buf, b"abcde");
        // Next chunk crosses the cap: only the remaining 3 bytes are kept.
        let stopped = push_capped(&mut buf, b"fghij", 8);
        assert!(stopped);
        assert_eq!(buf.len(), 8);
        assert_eq!(buf, b"abcdefgh");
    }

    #[test]
    fn push_capped_small_body_unaffected() {
        let mut buf = Vec::new();
        let stopped = push_capped(&mut buf, b"hello", 1024);
        assert!(!stopped);
        assert_eq!(buf, b"hello");
    }
}
