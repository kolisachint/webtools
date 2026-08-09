//! Parse DuckDuckGo Lite's table layout into structured results, and tell a
//! real results page apart from a bot challenge.

use once_cell::sync::Lazy;
use scraper::{Html, Selector};
use url::Url;

use super::types::{SearchResult, SearchStatus};
use crate::compress::compress_text;

/// One selector list matching result links *and* snippets, so `select` walks
/// them in document order and each snippet lands on the link that precedes it.
static RESULT_PARTS: Lazy<Selector> = Lazy::new(|| {
    Selector::parse("a.result-link, a.result__a, .result-snippet, .result__snippet")
        .expect("static selector")
});

/// Markers that identify a results page even when it holds no results, so an
/// empty search is not mistaken for a block.
static RESULTS_PAGE: Lazy<Selector> =
    Lazy::new(|| Selector::parse(".result-count, .no-results, .results").expect("static selector"));

/// Phrases DuckDuckGo serves on its anomaly/challenge page — which comes back
/// with HTTP 200, so the status line alone never reveals it.
const CHALLENGE_MARKERS: [&str; 5] = [
    "bots use duckduckgo",
    "/anomaly",
    "anomaly.js",
    "unusual traffic",
    "are you a robot",
];

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

/// Parse a DDG Lite results page.
///
/// Links and snippets are read from a single document-order traversal and a
/// snippet attaches to the most recent link. The previous version zipped two
/// independent iterators, so one result without a snippet row — PDFs and some
/// news rows have none — shifted every following snippet onto the wrong URL,
/// handing the caller a description of a page it was not citing.
pub fn parse_ddg_lite(html: &str, max_results: usize) -> Vec<SearchResult> {
    let document = Html::parse_document(html);
    let mut results: Vec<SearchResult> = Vec::new();

    for el in document.select(&RESULT_PARTS) {
        if el.value().name() == "a" {
            let title = compress_text(&el.text().collect::<String>());
            let url = resolve_result_url(el.value().attr("href").unwrap_or(""));
            if title.is_empty() && url.is_empty() {
                continue;
            }
            results.push(SearchResult {
                title,
                snippet: String::new(),
                url,
                ref_index: results.len() + 1,
            });
        } else if let Some(last) = results.last_mut() {
            // A snippet belongs to the link above it; ignore a stray second one
            // and any snippet appearing before the first result.
            if last.snippet.is_empty() {
                last.snippet = compress_text(&el.text().collect::<String>());
            }
        }
    }

    results.truncate(max_results);
    results
}

/// Classify a fetched results page.
///
/// A challenge page comes back as HTTP 200 with no result rows, which the old
/// parser reported as `result_count: 0` — indistinguishable from a query that
/// genuinely has no hits, so an agent concluded "nothing exists" and moved on.
/// An unrecognized page is treated as [`SearchStatus::Blocked`] rather than
/// empty: a page we cannot parse is a failure, and reporting it as success is
/// the bug this exists to fix.
pub fn classify_page(html: &str, result_count: usize) -> SearchStatus {
    if result_count > 0 {
        return SearchStatus::Ok;
    }
    let haystack = html.to_ascii_lowercase();
    if CHALLENGE_MARKERS.iter().any(|m| haystack.contains(m)) {
        return SearchStatus::Blocked;
    }
    // No results, but the page is structurally a results page: a real "no hits"
    // answer.
    if Html::parse_document(html)
        .select(&RESULTS_PAGE)
        .next()
        .is_some()
    {
        return SearchStatus::Empty;
    }
    SearchStatus::Blocked
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snippet_stays_with_its_own_link() {
        // The middle result has no snippet row.
        let html = r#"<table>
            <tr><td><a href="https://a.test/1" class="result-link">Alpha</a></td></tr>
            <tr><td class="result-snippet">About ALPHA.</td></tr>
            <tr><td><a href="https://b.test/2" class="result-link">Bravo</a></td></tr>
            <tr><td><a href="https://c.test/3" class="result-link">Charlie</a></td></tr>
            <tr><td class="result-snippet">About CHARLIE.</td></tr>
        </table>"#;
        let results = parse_ddg_lite(html, 10);
        assert_eq!(results.len(), 3);
        assert_eq!(results[0].snippet, "About ALPHA.");
        assert_eq!(results[1].snippet, "", "Bravo must not inherit a snippet");
        assert_eq!(results[2].snippet, "About CHARLIE.");
    }

    #[test]
    fn a_stray_leading_snippet_is_ignored() {
        let html = r#"<table>
            <tr><td class="result-snippet">Orphan.</td></tr>
            <tr><td><a href="https://a.test/1" class="result-link">Alpha</a></td></tr>
            <tr><td class="result-snippet">About ALPHA.</td></tr>
        </table>"#;
        let results = parse_ddg_lite(html, 10);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].snippet, "About ALPHA.");
    }

    #[test]
    fn alternate_class_names_are_recognized() {
        // DDG has served both `result-link` and `result__a` across endpoints.
        let html = r#"<a href="https://a.test/1" class="result__a">Alpha</a>
                      <div class="result__snippet">About ALPHA.</div>"#;
        let results = parse_ddg_lite(html, 10);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].snippet, "About ALPHA.");
    }

    #[test]
    fn challenge_page_is_blocked_not_empty() {
        let html = "<html><body><h1>Unfortunately, bots use DuckDuckGo too.</h1>\
                    <form action=\"/anomaly\"></form></body></html>";
        assert!(parse_ddg_lite(html, 10).is_empty());
        assert_eq!(classify_page(html, 0), SearchStatus::Blocked);
    }

    #[test]
    fn genuine_no_results_page_is_empty() {
        let html = "<html><body><table><tr><td class=\"result-count\">No results.</td>\
                    </tr></table></body></html>";
        assert_eq!(classify_page(html, 0), SearchStatus::Empty);
    }

    #[test]
    fn unrecognized_markup_is_blocked() {
        // Silently reporting "no results" for a page we cannot parse is exactly
        // the failure mode this guards against.
        assert_eq!(
            classify_page("<html><body>something else entirely</body></html>", 0),
            SearchStatus::Blocked
        );
    }

    #[test]
    fn results_present_is_always_ok() {
        assert_eq!(classify_page("<html>whatever</html>", 3), SearchStatus::Ok);
    }
}
