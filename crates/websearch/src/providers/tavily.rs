//! Tavily.
//!
//! Built for LLM callers: each hit carries cleaned page content, so a search
//! often answers the question without a follow-up `fetch`. The key travels in
//! an `Authorization: Bearer` header, never in the URL or the body.

use crate::types::{SearchOptions, SearchResult, SearchStatus};
use webfetch_core::http::read_body_capped;

const ENDPOINT: &str = "https://api.tavily.com/search";

pub async fn search(
    options: &SearchOptions,
    max: usize,
    api_key: &str,
) -> anyhow::Result<(Vec<SearchResult>, SearchStatus)> {
    let client = super::client(options.timeout_secs, &options.tls)?;

    let payload = serde_json::json!({
        "query": options.query,
        "max_results": max,
    });

    let resp = client
        .post(ENDPOINT)
        .header("Authorization", format!("Bearer {api_key}"))
        .header("Accept", "application/json")
        .json(&payload)
        .send()
        .await?;

    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        anyhow::bail!("tavily search failed ({status}): {}", body.trim());
    }

    let body = read_body_capped(resp).await.map_err(|(e, _)| e)?;
    Ok(parse(&body, max))
}

/// Parse a Tavily response body: `{"results": [{title, url, content}]}`.
pub(crate) fn parse(body: &str, max: usize) -> (Vec<SearchResult>, SearchStatus) {
    let value: serde_json::Value = match serde_json::from_str(body) {
        Ok(v) => v,
        Err(_) => return (Vec::new(), SearchStatus::Blocked),
    };
    let items = value
        .get("results")
        .and_then(|r| r.as_array())
        .cloned()
        .unwrap_or_default();

    let results = super::results_from_json(&items, "title", "url", &["content", "snippet"], max);
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
      "query": "rust async",
      "results": [
        { "title": "Tokio", "url": "https://tokio.rs",
          "content": "Tokio is an   asynchronous runtime for Rust.", "score": 0.98 },
        { "title": "async-std", "url": "https://async.rs", "content": "An async standard library." }
      ]
    }"#;

    #[test]
    fn parses_results_with_content_as_snippet() {
        let (results, status) = parse(SAMPLE, 10);
        assert_eq!(status, SearchStatus::Ok);
        assert_eq!(results.len(), 2);
        // Whitespace inside the content is compressed like every other snippet.
        assert_eq!(
            results[0].snippet,
            "Tokio is an asynchronous runtime for Rust."
        );
        assert_eq!(results[1].ref_index, 2);
    }

    #[test]
    fn no_hits_is_empty() {
        let (_, status) = parse(r#"{"results":[]}"#, 10);
        assert_eq!(status, SearchStatus::Empty);
    }

    #[test]
    fn unparseable_body_is_blocked() {
        assert_eq!(parse("not json", 10).1, SearchStatus::Blocked);
    }
}
