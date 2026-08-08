//! webfetch — token-efficient web content fetcher.
//!
//! The defining feature is **reference-style URL preservation**: instead of
//! stripping links to their domain (losing the ability to cite or follow
//! them) or expanding full URLs inline (wasting tokens), links are replaced
//! with compact `[N]` markers and collected into a recoverable reference list.

// Shared primitives live in webfetch-core; re-export them so both this
// crate's internal modules (via `crate::compress` / `crate::refs`) and
// external callers keep a stable path.
pub use webfetch_core::{charset, compress, http, refs, tls};

pub mod convert;
pub mod extract;
pub mod fetch;
pub mod guard;
pub mod limits;
pub mod media;
pub mod types;

pub use fetch::fetch_page;
use media::Media;
use types::{ContentStatus, ContentType, FetchOptions, FetchResult, Metadata, UrlReference};

use scraper::{Html, Selector};

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

    // Refuse a pathologically nested document before parsing it — see
    // `limits`. This is checked here rather than at the HTTP layer so every
    // entry point (network fetch, `--from-file`, library caller) is covered.
    if matches!(media, Media::Html) {
        if let Some(depth) = limits::too_deeply_nested(body) {
            return too_complex_result(source_url, depth, options.content_type);
        }
    }

    let (title, content, references, metadata, output_type) = match &media {
        Media::Html => convert_html_body(body, source_url, content_type_header, options),
        Media::Json => {
            // Pretty-print so an agent reads clean JSON; fall back to raw.
            let pretty = serde_json::from_str::<serde_json::Value>(body)
                .ok()
                .and_then(|v| serde_json::to_string_pretty(&v).ok())
                .unwrap_or_else(|| body.trim().to_string());
            (
                String::new(),
                budget_plain(&pretty, options.max_tokens),
                Vec::new(),
                Metadata::default(),
                ContentType::Structured,
            )
        }
        Media::Text => (
            String::new(),
            budget_plain(body.trim(), options.max_tokens),
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

    FetchResult {
        token_estimate: compress::estimate_tokens(&content),
        status: classify_content(&media, &content, body),
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

/// The result returned for a document refused by [`limits::too_deeply_nested`].
fn too_complex_result(source_url: &str, depth: usize, content_type: ContentType) -> FetchResult {
    let content = format!(
        "[document refused: nesting depth {depth} exceeds the limit of {} — \
         parsing it would take minutes]",
        limits::MAX_NESTING_DEPTH
    );
    FetchResult {
        token_estimate: compress::estimate_tokens(&content),
        status: ContentStatus::TooComplex,
        title: String::new(),
        final_url: source_url.to_string(),
        content,
        content_type,
        media: "html".to_string(),
        references: Vec::new(),
        metadata: Metadata::default(),
        source: source_url.to_string(),
    }
}

/// The HTML branch of [`convert_body`], kept separate because it is the only
/// one that parses a document — and it now parses exactly once. Title, metadata
/// and the conversion itself all read the same tree; the previous version
/// parsed for the first two and then parsed again inside the converter, which
/// measured at roughly a third of the whole pipeline's cost.
#[allow(clippy::type_complexity)]
fn convert_html_body(
    body: &str,
    source_url: &str,
    content_type_header: Option<&str>,
    options: &FetchOptions,
) -> (String, String, Vec<UrlReference>, Metadata, ContentType) {
    let doc = Html::parse_document(body);
    let title = extract::extract_title(&doc);
    let mut metadata = extract::extract_metadata(&doc);
    metadata.charset = undecodable_charset(content_type_header, &doc);

    let converted = convert::convert_parsed(&doc, source_url, options.content_type);
    // Drop a leading body line that merely repeats the title (common when the
    // title was derived from the page's first <h1>, which also opens the body).
    let body_text = strip_duplicate_title(&title, converted.content);

    let (content, references) = match options.content_type {
        // Reference-style text: the body cites `[N]`, so the budget rule is
        // "truncate the body, then keep the references it still cites".
        ContentType::Text => {
            let (content, kept) =
                refs::fit_to_budget(&body_text, &converted.references, options.max_tokens);
            let references = converted
                .references
                .into_iter()
                .filter(|r| kept.contains(&r.index))
                .collect();
            (content, references)
        }
        // Markdown carries its links inline, so there are no markers to match
        // on: keep the references whose URL survives in the truncated text.
        ContentType::Markdown => {
            let content = budget_plain(&body_text, options.max_tokens);
            let references = converted
                .references
                .into_iter()
                .filter(|r| content.contains(&r.url))
                .collect();
            (content, references)
        }
        // The content is JSON; truncating its text would produce invalid JSON,
        // so blocks are dropped instead and the document re-serialized.
        ContentType::Structured => budget_structured(&doc, source_url, options.max_tokens),
    };

    (title, content, references, metadata, options.content_type)
}

/// Truncate free-form text (no reference markers to preserve).
fn budget_plain(text: &str, max_tokens: Option<usize>) -> String {
    match max_tokens {
        Some(max) => compress::truncate_to_tokens(text, max),
        None => text.to_string(),
    }
}

/// Fit a structured document to the token budget by dropping trailing blocks
/// and re-serializing, so the output is always valid JSON.
///
/// Block count is monotone in serialized size, so a binary search finds the
/// largest prefix that fits in a handful of serializations rather than one per
/// dropped block.
fn budget_structured(
    doc: &Html,
    source_url: &str,
    max_tokens: Option<usize>,
) -> (String, Vec<UrlReference>) {
    use convert::structured::{to_json, StructuredDoc};

    let parsed = convert::structured::structured(doc, source_url);

    let render = |n: usize| -> (String, Vec<UrlReference>) {
        let blocks = parsed.blocks[..n].to_vec();
        let cited = refs::cited_indices(
            &blocks
                .iter()
                .map(|b| b.text.as_str())
                .collect::<Vec<_>>()
                .join(" "),
        );
        let references: Vec<UrlReference> = parsed
            .references
            .iter()
            .filter(|r| cited.contains(&r.index))
            .cloned()
            .collect();
        let json = to_json(&StructuredDoc {
            blocks,
            references: references.clone(),
        });
        (json, references)
    };

    let Some(max) = max_tokens else {
        return render(parsed.blocks.len());
    };

    let full = render(parsed.blocks.len());
    if compress::estimate_tokens(&full.0) <= max {
        return full;
    }

    // Largest block count that fits.
    let (mut lo, mut hi) = (0usize, parsed.blocks.len());
    while lo < hi {
        let mid = (lo + hi).div_ceil(2);
        if compress::estimate_tokens(&render(mid).0) <= max {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    render(lo)
}

/// Decide whether an extraction actually produced content, and if not, whether
/// the page looks like it needs a browser.
fn classify_content(media: &Media, content: &str, raw: &str) -> ContentStatus {
    let empty = match media {
        // An empty structured document still serializes to a JSON envelope.
        Media::Html => content.trim().is_empty() || is_empty_structured(content),
        _ => content.trim().is_empty(),
    };
    if !empty {
        return ContentStatus::Ok;
    }
    if matches!(media, Media::Html) && has_scripts(raw) {
        return ContentStatus::NeedsJs;
    }
    ContentStatus::Empty
}

/// A structured render of a page with nothing in it.
fn is_empty_structured(content: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(content)
        .ok()
        .and_then(|v| {
            v.get("blocks")
                .and_then(|b| b.as_array())
                .map(|b| b.is_empty())
        })
        .unwrap_or(false)
}

/// Does the raw document carry scripts? A shell with scripts and no text is a
/// client-rendered page, not an empty one.
///
/// Scans in place rather than lowercasing the whole body: this runs on bodies
/// of up to the 5 MiB cap, and allocating a second copy of one to answer a
/// yes/no question is not worth it.
fn has_scripts(raw: &str) -> bool {
    raw.as_bytes()
        .windows(7)
        .any(|w| w.eq_ignore_ascii_case(b"<script"))
}

/// Report a declared charset this build cannot decode.
///
/// The network path decodes the body before it reaches here (see
/// `webfetch_core::charset`), so this only fires for offline callers passing a
/// header in directly, and only for encodings outside the UTF-8 and
/// windows-1252 families — those are decoded exactly and never reported.
fn undecodable_charset(header: Option<&str>, doc: &Html) -> Option<String> {
    let declared = header.and_then(charset::from_content_type).or_else(|| {
        let sel = Selector::parse("meta[charset]").ok()?;
        doc.select(&sel)
            .next()
            .and_then(|el| el.value().attr("charset"))
            .map(|c| c.to_string())
    })?;

    match charset::classify(&declared) {
        charset::Charset::Unsupported(name) => Some(name),
        _ => None,
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
    // `source` is what was asked for, `final_url` is where it came from. They
    // were both set to the post-redirect URL, which discarded the request.
    result.source = options.url;
    result.final_url = page.final_url;
    // The fetch layer knows what it actually decoded with, including the
    // `<meta charset>` fallback, so its verdict wins over re-deriving one here.
    if page.undecodable_charset.is_some() {
        result.metadata.charset = page.undecodable_charset;
    }
    Ok(result)
}

/// Parse a content-type string ("text" | "markdown" | "structured").
pub fn parse_content_type(s: &str) -> ContentType {
    ContentType::parse(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Only charsets that cannot be decoded exactly are reported; UTF-8 and the
    /// windows-1252 family are handled, so they are not a problem to flag.
    #[test]
    fn only_undecodable_charsets_are_reported() {
        let doc = Html::parse_document("<html></html>");
        assert_eq!(
            undecodable_charset(Some("text/html; charset=utf-8"), &doc),
            None
        );
        assert_eq!(
            undecodable_charset(Some("text/html; charset=ISO-8859-1"), &doc),
            None
        );
        let doc = Html::parse_document(r#"<html><head><meta charset="shift_jis"></head></html>"#);
        assert_eq!(undecodable_charset(None, &doc), Some("shift_jis".into()));
    }

    #[test]
    fn script_shell_is_needs_js_not_empty() {
        let html =
            "<html><body><div id=\"root\"></div><script src=\"/app.js\"></script></body></html>";
        let r = convert_html(html, "https://spa.test/", &FetchOptions::default());
        assert_eq!(r.status, ContentStatus::NeedsJs);
        assert!(r.status.is_failure());
    }

    #[test]
    fn a_page_with_text_is_ok() {
        let html = "<html><body><article><p>Real words here.</p></article></body></html>";
        let r = convert_html(html, "https://x.test/", &FetchOptions::default());
        assert_eq!(r.status, ContentStatus::Ok);
    }
}
