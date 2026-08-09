//! DuckDuckGo Lite: the keyless default. Scraped HTML, so it is the one
//! provider that can be blocked or reshaped without warning — see
//! [`crate::extract::classify_page`], which turns that into a reported failure
//! instead of a silent empty result.

use std::time::Duration;

use reqwest::{redirect::Policy, Client};

use crate::extract;
use crate::types::{SearchOptions, SearchResult, SearchStatus};
use webfetch_core::http::{read_body_capped, transient_send_error, transient_status};

const MAX_ATTEMPTS: u32 = 3;

/// DDG answers a throttled client with `202 Accepted` and a challenge body
/// rather than a 4xx, so the status is worth naming explicitly.
const CHALLENGE_STATUS: u16 = 202;

pub async fn search(
    options: &SearchOptions,
    max: usize,
) -> anyhow::Result<(Vec<SearchResult>, SearchStatus)> {
    let html = fetch_lite(&options.query, options).await?;
    let results = extract::parse_ddg_lite(&html, max);
    let status = extract::classify_page(&html, results.len());
    Ok((results, status))
}

/// Fetch the raw DuckDuckGo Lite results page for a query, retrying transient
/// failures (connection/timeout, 5xx, 429) with exponential backoff.
pub async fn fetch_lite(query: &str, options: &SearchOptions) -> anyhow::Result<String> {
    let builder = Client::builder()
        .timeout(Duration::from_secs(options.timeout_secs))
        .user_agent(webfetch_core::http::USER_AGENT)
        // Follow at most one hop; DDG uses a redirect to hand off to its
        // challenge flow, and an unbounded chain is not something a search
        // client should walk.
        .redirect(Policy::limited(1))
        .gzip(true);
    // Trust the OS store (+ SSL_CERT_FILE / --ca-cert) so the request succeeds
    // behind a TLS-intercepting proxy, not just with the bundled webpki roots.
    let client = options.tls.apply(builder)?.build()?;

    let mut url = format!(
        "https://lite.duckduckgo.com/lite/?q={}",
        urlencoding::encode(query)
    );
    // DDG safe-search toggle: kp=1 strict, kp=-1 off.
    if let Some(safe) = options.safe_search {
        url.push_str(if safe { "&kp=1" } else { "&kp=-1" });
    }

    let mut delay = Duration::from_millis(200);
    for attempt_no in 1..=MAX_ATTEMPTS {
        match attempt(&client, &url).await {
            Ok(body) => return Ok(body),
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

/// One request attempt; the bool reports whether a failure is worth retrying.
async fn attempt(client: &Client, url: &str) -> Result<String, (anyhow::Error, bool)> {
    let resp = match client
        .get(url)
        .header("Accept", "text/html,application/xhtml+xml")
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
    if status.as_u16() == CHALLENGE_STATUS {
        return Err((
            anyhow::anyhow!("DuckDuckGo returned {status} (rate limited or challenged)"),
            true,
        ));
    }
    let resp = match resp.error_for_status() {
        Ok(r) => r,
        Err(e) => {
            let transient = transient_status(status);
            return Err((e.into(), transient));
        }
    };
    // Capped read: the search path previously called `text()`, which is
    // unbounded, while the fetch path capped at 5 MiB.
    read_body_capped(resp).await
}
