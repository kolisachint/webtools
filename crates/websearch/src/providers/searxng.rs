//! A self-hosted SearXNG instance's JSON API.
//!
//! The option for networks where the public search APIs are unreachable, or
//! where queries must not leave the estate. `base_url` points at the instance
//! root; the optional key travels as a bearer header.

use crate::types::{SearchOptions, SearchResult, SearchStatus};
use webfetch_core::http::read_body_capped;

pub async fn search(
    options: &SearchOptions,
    max: usize,
    base_url: &str,
    api_key: Option<&str>,
) -> anyhow::Result<(Vec<SearchResult>, SearchStatus)> {
    let client = super::client(options.timeout_secs, &options.tls)?;

    let endpoint = format!("{}/search", base_url.trim_end_matches('/'));
    let mut request = client
        .get(&endpoint)
        .query(&[("q", options.query.as_str()), ("format", "json")])
        .header("Accept", "application/json");
    if let Some(safe) = options.safe_search {
        request = request.query(&[("safesearch", if safe { "2" } else { "0" })]);
    }
    if let Some(key) = api_key {
        request = request.header("Authorization", format!("Bearer {key}"));
    }

    let resp = request.send().await?;
    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        anyhow::bail!("searxng search failed ({status}): {}", body.trim());
    }

    let body = read_body_capped(resp).await.map_err(|(e, _)| e)?;
    Ok(parse(&body, max))
}

/// Parse a SearXNG response body: `{"results": [{title, url, content}]}`.
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

    let results = super::results_from_json(&items, "title", "url", &["content"], max);
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

    #[test]
    fn parses_results() {
        let body = r#"{"query":"x","results":[
            {"url":"https://a.test/1","title":"Alpha","content":"About alpha."},
            {"url":"https://b.test/2","title":"Bravo","content":"About bravo."}]}"#;
        let (results, status) = parse(body, 10);
        assert_eq!(status, SearchStatus::Ok);
        assert_eq!(results.len(), 2);
        assert_eq!(results[1].title, "Bravo");
    }

    #[test]
    fn html_error_page_is_blocked() {
        assert_eq!(parse("<html>502</html>", 10).1, SearchStatus::Blocked);
    }
}
