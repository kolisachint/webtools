//! Types for the web-search layer.

use serde::{Deserialize, Serialize};

/// The slim reference entry shared with the fetch path.
pub use crate::providers::Provider;
pub use crate::refs::Reference;
pub use crate::tls::TlsConfig;

/// A single search hit, carrying its reference index so the inline body can
/// cite `[N]` while the full URL lives in the reference block.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SearchResult {
    pub title: String,
    pub snippet: String,
    pub url: String,
    pub ref_index: usize,
}

/// Whether a search actually answered.
///
/// Without this, a bot-challenge page (served with HTTP 200 and no result rows)
/// and a query with genuinely no hits were the same observable outcome: zero
/// results and a success exit. Callers could not tell "the web has no answer"
/// from "I was refused", and agents reliably concluded the former.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SearchStatus {
    /// Results were parsed.
    Ok,
    /// A real results page that reported no hits.
    Empty,
    /// A challenge, rate-limit, or otherwise unparseable page. A failure.
    Blocked,
}

impl SearchStatus {
    /// Did the search fail to answer? Drives the CLI exit code and the MCP
    /// `isError` flag.
    pub fn is_failure(self) -> bool {
        matches!(self, SearchStatus::Blocked)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchOutput {
    pub query: String,
    pub results: Vec<SearchResult>,
    pub references: Vec<Reference>,
    pub token_estimate: usize,
    pub result_count: usize,
    /// Whether the search answered — see [`SearchStatus`].
    pub status: SearchStatus,
    /// Which backend produced these results, so a silent fallback from a keyed
    /// provider to scraped DuckDuckGo is visible to the caller.
    pub provider: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SearchOptions {
    pub query: String,
    pub max_results: Option<usize>,
    pub safe_search: Option<bool>,
    pub timeout_secs: u64,
    /// TLS trust configuration (OS store is honoured by default; this carries
    /// the explicit `--ca-cert` / `--insecure` overrides).
    #[serde(default)]
    pub tls: TlsConfig,
    /// Which backend to query. Defaults to keyless DuckDuckGo Lite so the tool
    /// needs no configuration; credentials are resolved by the caller (the CLI
    /// reads them from flags, the environment, or the config file) and passed
    /// in already populated — the library never reads a config file itself.
    #[serde(default)]
    pub provider: Provider,
    /// Backend to try when the primary errors or is blocked.
    #[serde(default)]
    pub fallback: Option<Provider>,
}

impl Default for SearchOptions {
    fn default() -> Self {
        Self {
            query: String::new(),
            max_results: Some(5),
            safe_search: None,
            timeout_secs: 10,
            tls: TlsConfig::default(),
            provider: Provider::default(),
            fallback: None,
        }
    }
}
