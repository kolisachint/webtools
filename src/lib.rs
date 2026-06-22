//! webfetch — token-efficient web content fetcher.
//!
//! The defining feature is **reference-style URL preservation**: instead of
//! stripping links to their domain (losing the ability to cite or follow
//! them) or expanding full URLs inline (wasting tokens), links are replaced
//! with compact `[N]` markers and collected into a recoverable reference list.

pub mod compress;
pub mod convert;
pub mod extract;
pub mod fetch;
pub mod refs;
pub mod search;
pub mod types;

pub use fetch::fetch_html;
use types::{ContentType, FetchOptions, FetchResult};

use scraper::Html;

/// Convert already-fetched HTML into a [`FetchResult`] without any network I/O.
///
/// Useful for tests and for callers that obtain HTML by other means.
pub fn convert_html(html: &str, source_url: &str, options: &FetchOptions) -> FetchResult {
    let title = extract::extract_title(&Html::parse_document(html));
    let mut converted = convert::convert(html, source_url, options.content_type);

    if let Some(max) = options.max_tokens {
        converted.content = compress::truncate_to_tokens(&converted.content, max);
    }

    FetchResult {
        token_estimate: compress::estimate_tokens(&converted.content),
        title,
        final_url: source_url.to_string(),
        content: converted.content,
        content_type: options.content_type,
        references: converted.references,
        source: source_url.to_string(),
    }
}

/// Fetch a URL and convert it according to `options`.
pub async fn fetch_and_convert(options: FetchOptions) -> anyhow::Result<FetchResult> {
    let page = fetch::fetch_html(&options.url, options.timeout_secs).await?;
    let mut result = convert_html(&page.html, &page.final_url, &options);
    result.final_url = page.final_url;
    Ok(result)
}

/// Parse a content-type string ("text" | "markdown" | "structured").
pub fn parse_content_type(s: &str) -> ContentType {
    ContentType::parse(s)
}
