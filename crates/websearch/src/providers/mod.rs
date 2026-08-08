//! Search backends.
//!
//! DuckDuckGo Lite needs no key and stays the default, so `webtools` keeps
//! working with no configuration at all. It is also scraped HTML, which means
//! it is rate-limited aggressively and can change shape without notice — the
//! keyed providers exist so a caller who cares about reliability can pay for a
//! real API contract instead.
//!
//! No provider ever puts its key in a URL: keys travel in headers, so they
//! cannot leak through `reqwest` error messages (which embed the request URL),
//! proxy logs, or server access logs.

pub mod brave;
pub mod ddg;
pub mod searxng;
pub mod tavily;

use std::time::Duration;

use reqwest::Client;
use serde::Deserialize;

use crate::tls::TlsConfig;
use crate::types::{SearchOptions, SearchResult, SearchStatus};

/// Which backend answers a search, and the credential it needs.
#[derive(Clone, Default, Deserialize)]
#[serde(tag = "name", rename_all = "lowercase")]
pub enum Provider {
    /// Scraped DuckDuckGo Lite. No key, no contract. The default, so the tool
    /// works with no configuration at all.
    #[default]
    #[serde(alias = "ddg")]
    Duckduckgo,
    /// Brave Search API. Key travels in `X-Subscription-Token`.
    Brave { api_key: String },
    /// Tavily, which returns cleaned page content alongside each hit.
    Tavily { api_key: String },
    /// A self-hosted SearXNG instance's JSON API.
    Searxng {
        base_url: String,
        #[serde(default)]
        api_key: Option<String>,
    },
}

/// Redacting `Debug`: `SearchOptions` derives `Debug` and is routinely printed
/// in error paths, so a derived impl here would put API keys on stderr.
impl std::fmt::Debug for Provider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Provider::Duckduckgo => write!(f, "Duckduckgo"),
            Provider::Brave { .. } => write!(f, "Brave {{ api_key: <redacted> }}"),
            Provider::Tavily { .. } => write!(f, "Tavily {{ api_key: <redacted> }}"),
            Provider::Searxng { base_url, api_key } => write!(
                f,
                "Searxng {{ base_url: {base_url:?}, api_key: {} }}",
                if api_key.is_some() {
                    "<redacted>"
                } else {
                    "None"
                }
            ),
        }
    }
}

impl Provider {
    /// Stable identifier recorded on the output so a fallback is visible.
    pub fn label(&self) -> &'static str {
        match self {
            Provider::Duckduckgo => "duckduckgo",
            Provider::Brave { .. } => "brave",
            Provider::Tavily { .. } => "tavily",
            Provider::Searxng { .. } => "searxng",
        }
    }

    /// Parse a provider name with no credential attached. Used by the CLI's
    /// `--search-provider` flag, which selects an entry the config file (or an
    /// environment variable) supplies the key for.
    pub fn parse_name(s: &str) -> Option<&'static str> {
        match s.trim().to_ascii_lowercase().as_str() {
            "duckduckgo" | "ddg" => Some("duckduckgo"),
            "brave" => Some("brave"),
            "tavily" => Some("tavily"),
            "searxng" | "searx" => Some("searxng"),
            _ => None,
        }
    }

    /// Run a query against this backend.
    pub async fn search(
        &self,
        options: &SearchOptions,
    ) -> anyhow::Result<(Vec<SearchResult>, SearchStatus)> {
        let max = options.max_results.unwrap_or(5);
        match self {
            Provider::Duckduckgo => ddg::search(options, max).await,
            Provider::Brave { api_key } => brave::search(options, max, api_key).await,
            Provider::Tavily { api_key } => tavily::search(options, max, api_key).await,
            Provider::Searxng { base_url, api_key } => {
                searxng::search(options, max, base_url, api_key.as_deref()).await
            }
        }
    }
}

/// Build the HTTP client every provider uses: shared user agent, shared TLS
/// trust configuration, and the caller's timeout.
pub(crate) fn client(timeout_secs: u64, tls: &TlsConfig) -> anyhow::Result<Client> {
    let builder = Client::builder()
        .timeout(Duration::from_secs(timeout_secs))
        .user_agent(webfetch_core::http::USER_AGENT)
        .gzip(true);
    Ok(tls.apply(builder)?.build()?)
}

/// Turn a provider's JSON hit list into results, numbering references as we go.
///
/// Field names are read defensively out of a `serde_json::Value` rather than a
/// typed struct: a provider adding or renaming a peripheral field then degrades
/// one hit instead of failing the whole search.
pub(crate) fn results_from_json(
    items: &[serde_json::Value],
    title_key: &str,
    url_key: &str,
    snippet_keys: &[&str],
    max: usize,
) -> Vec<SearchResult> {
    let mut out = Vec::new();
    for item in items {
        if out.len() >= max {
            break;
        }
        let url = item
            .get(url_key)
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        if url.is_empty() {
            continue;
        }
        let title = item
            .get(title_key)
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let snippet = snippet_keys
            .iter()
            .find_map(|k| item.get(*k).and_then(|v| v.as_str()))
            .map(crate::compress::compress_text)
            .unwrap_or_default();
        out.push(SearchResult {
            title: crate::compress::compress_text(&title),
            snippet,
            url,
            ref_index: out.len() + 1,
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_never_prints_a_key() {
        let p = Provider::Brave {
            api_key: "SECRET-KEY-VALUE".into(),
        };
        let shown = format!("{p:?}");
        assert!(!shown.contains("SECRET-KEY-VALUE"), "leaked: {shown}");
        assert!(shown.contains("<redacted>"));

        let p = Provider::Searxng {
            base_url: "https://searx.test".into(),
            api_key: Some("SECRET-KEY-VALUE".into()),
        };
        let shown = format!("{p:?}");
        assert!(!shown.contains("SECRET-KEY-VALUE"), "leaked: {shown}");
    }

    #[test]
    fn provider_names_parse() {
        assert_eq!(Provider::parse_name("Brave"), Some("brave"));
        assert_eq!(Provider::parse_name(" ddg "), Some("duckduckgo"));
        assert_eq!(Provider::parse_name("searx"), Some("searxng"));
        assert_eq!(Provider::parse_name("nope"), None);
    }

    #[test]
    fn json_results_skip_entries_without_a_url() {
        let items: Vec<serde_json::Value> = serde_json::from_str(
            r#"[{"title":"A","url":"https://a.test","description":"about a"},
                {"title":"No URL","description":"skipped"},
                {"title":"B","url":"https://b.test"}]"#,
        )
        .unwrap();
        let got = results_from_json(&items, "title", "url", &["description"], 10);
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].ref_index, 1);
        assert_eq!(got[0].snippet, "about a");
        assert_eq!(got[1].ref_index, 2, "indices stay contiguous after a skip");
        assert_eq!(got[1].snippet, "");
    }

    #[test]
    fn json_results_respect_max() {
        let items: Vec<serde_json::Value> =
            serde_json::from_str(r#"[{"url":"https://a"},{"url":"https://b"}]"#).unwrap();
        assert_eq!(results_from_json(&items, "t", "url", &[], 1).len(), 1);
    }
}
