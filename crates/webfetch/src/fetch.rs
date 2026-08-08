use reqwest::header::{CONTENT_TYPE, LOCATION};
use reqwest::{redirect::Policy, Client};
use std::net::SocketAddr;
use std::time::{Duration, Instant};

use crate::guard;
use crate::tls::TlsConfig;
use webfetch_core::charset;
use webfetch_core::http::{
    read_body_capped_bytes, transient_send_error, transient_status, USER_AGENT,
};

const MAX_ATTEMPTS: u32 = 3;
const MAX_REDIRECTS: usize = 5;

/// Multiplier turning the per-request `--timeout` into a budget for the whole
/// fetch.
///
/// `--timeout` bounds one request. With retries and redirects a single fetch
/// could issue `MAX_ATTEMPTS * (MAX_REDIRECTS + 1)` requests, so `--timeout 10`
/// could keep running for minutes — not what anyone setting a timeout expects.
/// The whole fetch now shares one deadline, and each request gets whatever is
/// left of it.
const TOTAL_BUDGET_MULTIPLIER: u32 = 3;

/// Outcome of an HTTP fetch: the body, the URL we actually landed on after
/// following redirects, and the response's `Content-Type` (if any).
#[derive(Debug, Clone)]
pub struct FetchedPage {
    pub body: String,
    pub final_url: String,
    pub content_type: Option<String>,
    /// Set when the page declared a charset this build cannot decode, so the
    /// body was read as UTF-8 and may be garbled.
    pub undecodable_charset: Option<String>,
}

/// Find `<meta charset=…>` in the head of a body whose header declared nothing.
///
/// Only the first 2 KiB are searched: the declaration is required to appear
/// early, and scanning a whole 5 MiB body for it would be wasted work.
fn sniff_meta_charset(raw: &[u8]) -> Option<String> {
    const WINDOW: usize = 2048;
    let head = &raw[..raw.len().min(WINDOW)];
    let text = String::from_utf8_lossy(head).to_ascii_lowercase();
    let at = text.find("charset")? + "charset".len();
    let rest = text[at..].trim_start().strip_prefix('=')?.trim_start();
    let value: String = rest
        .trim_start_matches(['"', '\''])
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .collect();
    (!value.is_empty()).then_some(value)
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
///
/// Note that IP pinning only takes effect on a direct connection: when
/// `HTTP(S)_PROXY` is set, the proxy resolves the host itself and the pinned
/// addresses are never used. See `docs/product.md`.
fn build_client(
    url: &reqwest::Url,
    timeout: Duration,
    pinned: &[SocketAddr],
    tls: &TlsConfig,
) -> anyhow::Result<Client> {
    let mut builder = Client::builder()
        .timeout(timeout)
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

/// One request attempt. The bool in the error reports whether the failure is
/// transient (worth retrying): connection/timeout errors, 5xx, and 429.
async fn attempt(client: &Client, url: &str) -> Result<Hop, (anyhow::Error, bool)> {
    let resp = match client
        .get(url)
        .header("Accept", "text/html,application/xhtml+xml,*/*;q=0.8")
        .header("Accept-Language", "en-US,en;q=0.9")
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => {
            let transient = transient_send_error(&e);
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
            let transient = transient_status(status);
            return Err((e.into(), transient));
        }
    };

    let final_url = resp.url().to_string();
    let content_type = resp
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    // Decode with the response's declared charset rather than assuming UTF-8:
    // a windows-1252 / ISO-8859-1 page is otherwise returned full of
    // replacement characters.
    let raw = read_body_capped_bytes(resp).await?;
    let declared = content_type
        .as_deref()
        .and_then(charset::from_content_type)
        .or_else(|| sniff_meta_charset(&raw));
    let (body, undecodable_charset) = charset::decode(&raw, declared.as_deref());

    Ok(Hop::Page(FetchedPage {
        body,
        final_url,
        content_type,
        undecodable_charset,
    }))
}

/// Issue one hop's request, retrying transient failures with exponential
/// backoff (200ms, 400ms) while the overall deadline allows.
async fn fetch_with_retries(client: &Client, url: &str, deadline: Instant) -> anyhow::Result<Hop> {
    let mut delay = Duration::from_millis(200);
    for attempt_no in 1..=MAX_ATTEMPTS {
        match attempt(client, url).await {
            Ok(hop) => return Ok(hop),
            Err((err, transient)) => {
                if attempt_no == MAX_ATTEMPTS || !transient {
                    return Err(err);
                }
                if Instant::now() + delay >= deadline {
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
/// chain at [`MAX_REDIRECTS`], the body at
/// [`webfetch_core::http::MAX_BODY_BYTES`], and the whole operation at
/// [`TOTAL_BUDGET_MULTIPLIER`] times `timeout_secs`.
pub async fn fetch_page(
    url: &str,
    timeout_secs: u64,
    tls: &TlsConfig,
) -> anyhow::Result<FetchedPage> {
    let per_request = Duration::from_secs(timeout_secs);
    let deadline = Instant::now() + per_request * TOTAL_BUDGET_MULTIPLIER;

    let mut current = reqwest::Url::parse(url)?;
    let mut hops = 0usize;

    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            anyhow::bail!(
                "fetch exceeded its total budget ({}s across redirects and retries)",
                timeout_secs * TOTAL_BUDGET_MULTIPLIER as u64
            );
        }

        // Validate + resolve the host for THIS hop, then pin the connection to
        // exactly those addresses.
        let pinned = guard::validate_url(&current).await?;
        let client = build_client(&current, per_request.min(remaining), &pinned, tls)?;

        match fetch_with_retries(&client, current.as_str(), deadline).await? {
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
