//! Zero-infrastructure web search via DuckDuckGo Lite scraping.
//!
//! No API key, no backend. Results reuse the same reference-style URL
//! preservation as the fetch path: each hit's title carries an inline `[N]`
//! marker and the full URLs are collected into a reference block, keeping the
//! context window tight while staying citable.

// Shared primitives from webfetch-core; re-exported so internal modules can
// keep using `crate::compress` / `crate::refs`.
pub use webfetch_core::{compress, refs, tls};

pub mod extract;
pub mod types;

use std::time::Duration;

use reqwest::Client;

use crate::compress::estimate_tokens;
use types::{Reference, SearchOptions, SearchOutput, SearchResult};

const USER_AGENT: &str =
    "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0 Safari/537.36";
const MAX_ATTEMPTS: u32 = 3;

/// Fetch the raw DuckDuckGo Lite results page for a query, retrying transient
/// failures (connection/timeout, 5xx, 429) with exponential backoff.
pub async fn fetch_ddg_lite(query: &str, options: &SearchOptions) -> anyhow::Result<String> {
    let builder = Client::builder()
        .timeout(Duration::from_secs(options.timeout_secs))
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
        .header("User-Agent", USER_AGENT)
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => {
            let transient = e.is_timeout() || e.is_connect() || e.is_request();
            return Err((e.into(), transient));
        }
    };
    let status = resp.status();
    let resp = match resp.error_for_status() {
        Ok(r) => r,
        Err(e) => {
            let transient = status.is_server_error() || status.as_u16() == 429;
            return Err((e.into(), transient));
        }
    };
    match resp.text().await {
        Ok(body) => Ok(body),
        Err(e) => {
            let transient = e.is_timeout();
            Err((e.into(), transient))
        }
    }
}

/// Build the reference block (index → URL) from parsed results.
pub fn build_refs(results: &[SearchResult]) -> Vec<Reference> {
    results
        .iter()
        .map(|r| Reference {
            index: r.ref_index,
            url: r.url.clone(),
        })
        .collect()
}

/// Render the inline body: each result as `title [N]` followed by its snippet.
/// URLs are intentionally absent here — they live in the reference block.
pub fn format_results(results: &[SearchResult]) -> String {
    results
        .iter()
        .map(|r| {
            if r.snippet.is_empty() {
                format!("{} [{}]", r.title, r.ref_index)
            } else {
                format!("{} [{}]\n{}", r.title, r.ref_index, r.snippet)
            }
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// Render the reference block appended to text output.
/// Thin wrapper over [`crate::refs::render_block`].
pub fn render_references(refs: &[Reference]) -> String {
    crate::refs::render_block(refs)
}

/// Parse an already-fetched results page into a [`SearchOutput`] (no network).
pub fn build_output(query: &str, html: &str, max_results: usize) -> SearchOutput {
    let results = extract::parse_ddg_lite(html, max_results);
    let references = build_refs(&results);

    let body = format_results(&results);
    let refs_block = render_references(&references);
    let full = if refs_block.is_empty() {
        body
    } else {
        format!("{body}\n\n{refs_block}")
    };

    SearchOutput {
        query: query.to_string(),
        token_estimate: estimate_tokens(&full),
        result_count: results.len(),
        references,
        results,
    }
}

/// Fetch and parse a query end to end.
pub async fn run_search(options: SearchOptions) -> anyhow::Result<SearchOutput> {
    let html = fetch_ddg_lite(&options.query, &options).await?;
    let max = options.max_results.unwrap_or(5);
    Ok(build_output(&options.query, &html, max))
}
