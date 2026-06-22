//! Parse DuckDuckGo Lite's table layout into structured results.

use scraper::{Html, Selector};
use url::Url;

use super::types::SearchResult;
use crate::compress::compress_text;

/// Resolve the real destination URL from a DDG Lite result href.
///
/// DDG Lite wraps targets in a redirect like
/// `//duckduckgo.com/l/?uddg=<percent-encoded-url>&rut=…`. We pull the
/// `uddg` parameter back out (already percent-decoded by the URL parser).
/// Protocol-relative hrefs (`//host/path`) get an `https:` scheme; anything
/// already absolute is returned unchanged.
pub fn resolve_result_url(href: &str) -> String {
    let href = href.trim();
    if href.is_empty() {
        return String::new();
    }

    // Normalize protocol-relative URLs so they can be parsed.
    let absolute = if let Some(stripped) = href.strip_prefix("//") {
        format!("https://{stripped}")
    } else {
        href.to_string()
    };

    if let Ok(parsed) = Url::parse(&absolute) {
        if let Some((_, target)) = parsed.query_pairs().find(|(k, _)| k == "uddg") {
            return target.into_owned();
        }
        return parsed.to_string();
    }

    absolute
}

pub fn parse_ddg_lite(html: &str, max_results: usize) -> Vec<SearchResult> {
    let document = Html::parse_document(html);

    // The `<a class="result-link">` carries title + href; the matching
    // `<td class="result-snippet">` holds the snippet. They appear in result
    // order, so zipping by index pairs them up.
    let link_selector = Selector::parse("a.result-link").unwrap();
    let snippet_selector = Selector::parse(".result-snippet").unwrap();

    let links = document.select(&link_selector);
    let mut snippets = document.select(&snippet_selector);

    let mut results = Vec::new();
    for (i, link) in links.enumerate() {
        if i >= max_results {
            break;
        }

        let title = compress_text(&link.text().collect::<String>());
        let url = resolve_result_url(link.value().attr("href").unwrap_or(""));
        let snippet = snippets
            .next()
            .map(|s| compress_text(&s.text().collect::<String>()))
            .unwrap_or_default();

        if title.is_empty() && url.is_empty() {
            continue;
        }

        results.push(SearchResult {
            title,
            snippet,
            url,
            ref_index: results.len() + 1,
        });
    }

    results
}
