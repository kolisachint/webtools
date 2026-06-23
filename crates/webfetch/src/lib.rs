//! webfetch — token-efficient web content fetcher.
//!
//! The defining feature is **reference-style URL preservation**: instead of
//! stripping links to their domain (losing the ability to cite or follow
//! them) or expanding full URLs inline (wasting tokens), links are replaced
//! with compact `[N]` markers and collected into a recoverable reference list.

// Shared primitives live in webfetch-core; re-export them so both this
// crate's internal modules (via `crate::compress` / `crate::refs`) and
// external callers keep a stable path.
pub use webfetch_core::{compress, refs, tls};

pub mod convert;
pub mod extract;
pub mod fetch;
pub mod guard;
pub mod media;
pub mod types;

pub use fetch::fetch_page;
use media::Media;
use types::{ContentType, FetchOptions, FetchResult, Metadata};

use scraper::Html;

/// Convert already-fetched HTML into a [`FetchResult`] without any network I/O.
///
/// Useful for tests and for callers that obtain HTML by other means. Always
/// treats the input as HTML; use [`convert_body`] for media-aware handling.
pub fn convert_html(html: &str, source_url: &str, options: &FetchOptions) -> FetchResult {
    convert_body(html, source_url, Some("text/html"), options)
}

/// Convert a fetched body to a [`FetchResult`], choosing how to treat it based
/// on its `Content-Type` (or a sniff of the body). HTML is extracted; JSON is
/// pretty-printed; other text is passed through verbatim; binary is summarized.
pub fn convert_body(
    body: &str,
    source_url: &str,
    content_type_header: Option<&str>,
    options: &FetchOptions,
) -> FetchResult {
    let media = media::classify(content_type_header, body);

    let (title, mut content, references, metadata, output_type) = match &media {
        Media::Html => {
            let doc = Html::parse_document(body);
            let title = extract::extract_title(&doc);
            let metadata = extract::extract_metadata(&doc);
            let converted = convert::convert(body, source_url, options.content_type);
            (
                title,
                converted.content,
                converted.references,
                metadata,
                options.content_type,
            )
        }
        Media::Json => {
            // Pretty-print so an agent reads clean JSON; fall back to raw.
            let pretty = serde_json::from_str::<serde_json::Value>(body)
                .ok()
                .and_then(|v| serde_json::to_string_pretty(&v).ok())
                .unwrap_or_else(|| body.trim().to_string());
            (
                String::new(),
                pretty,
                Vec::new(),
                Metadata::default(),
                ContentType::Structured,
            )
        }
        Media::Text => (
            String::new(),
            body.trim().to_string(),
            Vec::new(),
            Metadata::default(),
            ContentType::Text,
        ),
        Media::Other(ct) => (
            String::new(),
            format!(
                "[non-text content: {ct}, {} bytes — not rendered]",
                body.len()
            ),
            Vec::new(),
            Metadata::default(),
            options.content_type,
        ),
    };

    // Drop a leading body line that merely repeats the title (common when the
    // title was derived from the page's first <h1>, which also opens the body).
    content = strip_duplicate_title(&title, content);

    if let Some(max) = options.max_tokens {
        // In reference-style text output the `References:` block is appended to
        // the end of `content`; truncate the body but keep that block intact so
        // inline `[N]` markers still resolve (see truncate_preserving_refs).
        let refs_block = if output_type == ContentType::Text {
            convert::text::render_references(&references)
        } else {
            String::new()
        };
        content = compress::truncate_preserving_refs(&content, &refs_block, max);
    }

    FetchResult {
        token_estimate: compress::estimate_tokens(&content),
        title,
        final_url: source_url.to_string(),
        content,
        content_type: output_type,
        media: media.label(),
        references,
        metadata,
        source: source_url.to_string(),
    }
}

/// When the title was derived from the page's first heading, the body repeats
/// it as its opening line. Drop that leading line when it normalizes to the
/// same text as `title`. Conservative: only an exact normalized match of the
/// *first* line is removed, so genuine content is never lost.
fn strip_duplicate_title(title: &str, content: String) -> String {
    if title.is_empty() {
        return content;
    }
    let mut parts = content.splitn(2, '\n');
    let first = parts.next().unwrap_or("");
    if compress::compress_text(first) == compress::compress_text(title) {
        return parts
            .next()
            .unwrap_or("")
            .trim_start_matches('\n')
            .to_string();
    }
    content
}

/// Fetch a URL and convert it according to `options`.
pub async fn fetch_and_convert(options: FetchOptions) -> anyhow::Result<FetchResult> {
    let page = fetch::fetch_page(&options.url, options.timeout_secs, &options.tls).await?;
    let mut result = convert_body(
        &page.body,
        &page.final_url,
        page.content_type.as_deref(),
        &options,
    );
    result.final_url = page.final_url;
    Ok(result)
}

/// Parse a content-type string ("text" | "markdown" | "structured").
pub fn parse_content_type(s: &str) -> ContentType {
    ContentType::parse(s)
}
