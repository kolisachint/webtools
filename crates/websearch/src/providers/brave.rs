//! Brave Search API.
//!
//! An independent index behind a real API contract, which is the point: unlike
//! scraped HTML it does not silently change shape or serve a challenge page.
//! The key travels in `X-Subscription-Token`, never in the URL.

use crate::types::{SearchOptions, SearchResult, SearchStatus};
use webfetch_core::http::read_body_capped;

const ENDPOINT: &str = "https://api.search.brave.com/res/v1/web/search";

pub async fn search(
    options: &SearchOptions,
    max: usize,
    api_key: &str,
) -> anyhow::Result<(Vec<SearchResult>, SearchStatus)> {
    let client = super::client(options.timeout_secs, &options.tls)?;

    let mut request = client
        .get(ENDPOINT)
        .query(&[("q", options.query.as_str())])
        .query(&[("count", max.min(20).to_string())])
        .header("Accept", "application/json")
        .header("X-Subscription-Token", api_key);
    if let Some(safe) = options.safe_search {
        request = request.query(&[("safesearch", if safe { "strict" } else { "off" })]);
    }

    let resp = request.send().await?;
    let status = resp.status();
    if !status.is_success() {
        // The body may carry Brave's own explanation (quota, bad key); the key
        // itself is a header, so nothing here can echo it back.
        let body = resp.text().await.unwrap_or_default();
        anyhow::bail!("brave search failed ({status}): {}", body.trim());
    }

    let body = read_body_capped(resp).await.map_err(|(e, _)| e)?;
    Ok(parse(&body, max))
}

/// Parse a Brave response body: `{"web": {"results": [{title, url, description}]}}`.
pub(crate) fn parse(body: &str, max: usize) -> (Vec<SearchResult>, SearchStatus) {
    let value: serde_json::Value = match serde_json::from_str(body) {
        Ok(v) => v,
        // A success status with an unparseable body is a failure, not an empty
        // result — the same distinction the DuckDuckGo path makes.
        Err(_) => return (Vec::new(), SearchStatus::Blocked),
    };
    let items = value
        .get("web")
        .and_then(|w| w.get("results"))
        .and_then(|r| r.as_array())
        .cloned()
        .unwrap_or_default();

    let results = super::results_from_json(&items, "title", "url", &["description"], max);
    let status = if results.is_empty() {
        SearchStatus::Empty
    } else {
        SearchStatus::Ok
    };
    (results, status)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{
      "type": "search",
      "web": { "results": [
        { "title": "Rust async book", "url": "https://rust-lang.github.io/async-book/",
          "description": "Asynchronous programming in <strong>Rust</strong>." },
        { "title": "Tokio", "url": "https://tokio.rs", "description": "An async runtime." }
      ]}
    }"#;

    #[test]
    fn parses_results_and_numbers_references() {
        let (results, status) = parse(SAMPLE, 10);
        assert_eq!(status, SearchStatus::Ok);
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].url, "https://rust-lang.github.io/async-book/");
        assert_eq!(results[0].ref_index, 1);
        assert_eq!(results[1].ref_index, 2);
        assert_eq!(results[1].snippet, "An async runtime.");
    }

    #[test]
    fn respects_max_results() {
        let (results, _) = parse(SAMPLE, 1);
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn no_hits_is_empty_not_blocked() {
        let (results, status) = parse(r#"{"web":{"results":[]}}"#, 10);
        assert!(results.is_empty());
        assert_eq!(status, SearchStatus::Empty);
    }

    #[test]
    fn unparseable_body_is_blocked() {
        let (_, status) = parse("<html>gateway error</html>", 10);
        assert_eq!(status, SearchStatus::Blocked);
    }

    #[test]
    fn missing_web_key_degrades_to_empty() {
        let (results, status) = parse(r#"{"query":{"original":"x"}}"#, 10);
        assert!(results.is_empty());
        assert_eq!(status, SearchStatus::Empty);
    }
}
