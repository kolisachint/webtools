use webfetch::compress::{compress_text, estimate_tokens, truncate_to_tokens};
use webfetch::convert::convert;
use webfetch::convert::text::{html_to_text_with_refs, render_references};
use webfetch::media::{classify, Media};
use webfetch::types::{ContentType, FetchOptions};
use webfetch::{convert_body, convert_html};

const DOCS: &str = include_str!("fixtures/docs.html");
const BLOG: &str = include_str!("fixtures/blog.html");
const SPA: &str = include_str!("fixtures/spa-shell.html");

// --- compression -----------------------------------------------------------

#[test]
fn test_compress_collapses_whitespace() {
    let output = compress_text("hello   world\n\n\n  test");
    assert_eq!(output, "hello world test");
}

#[test]
fn test_compress_removes_decorative() {
    // Regression: stripping the glyph must not leave a double space behind.
    let output = compress_text("Click ▶ to play");
    assert_eq!(output, "Click to play");
}

#[test]
fn test_truncate_to_tokens() {
    let text = "a".repeat(100);
    let out = truncate_to_tokens(&text, 5); // ~20 chars
    assert!(out.starts_with(&"a".repeat(20)));
    assert!(out.contains("truncated"));
    assert!(estimate_tokens(&text) == 25);
}

// --- reference-style URL preservation (the core feature) --------------------

#[test]
fn test_links_become_inline_references() {
    let base = "https://docs.example.com/page";
    let (text, refs) = html_to_text_with_refs(DOCS, base);

    // Anchor text is kept and followed by a compact [N] marker.
    assert!(text.contains("users endpoint [1]"), "text was: {text}");
    assert!(text.contains("OAuth2 [2]"), "text was: {text}");

    // Relative URLs are resolved against the base.
    assert_eq!(refs[0].url, "https://docs.example.com/api/v2/users");
    assert_eq!(refs[1].url, "https://auth.example.com/oauth2");
}

#[test]
fn test_duplicate_urls_share_one_reference() {
    let base = "https://docs.example.com/page";
    let (text, refs) = html_to_text_with_refs(DOCS, base);

    // The users endpoint appears twice but must reuse index [1].
    let occurrences = text.matches("[1]").count();
    assert_eq!(occurrences, 2, "text was: {text}");

    // Three distinct URLs total: users, oauth2, guide.
    assert_eq!(refs.len(), 3, "refs: {refs:?}");
    assert_eq!(refs[2].url, "https://docs.example.com/guide");
}

#[test]
fn test_references_block_rendering() {
    let refs = vec![
        webfetch::types::UrlReference {
            index: 1,
            url: "https://a.test/x".into(),
            text: "x".into(),
        },
        webfetch::types::UrlReference {
            index: 2,
            url: "https://b.test/y".into(),
            text: "y".into(),
        },
    ];
    let block = render_references(&refs);
    assert_eq!(
        block,
        "References:\n[1] https://a.test/x\n[2] https://b.test/y"
    );
}

#[test]
fn test_text_output_appends_reference_block() {
    let converted = convert(BLOG, "https://blog.example.com/post", ContentType::Text);
    assert!(converted.content.contains("references page [1]"));
    assert!(converted.content.contains("References:"));
    assert!(converted
        .content
        .contains("[1] https://blog.example.com/refs"));
    // Whitespace inside the paragraph was compressed.
    assert!(converted.content.contains("on our references page"));
}

#[test]
fn test_skippable_elements_excluded() {
    let (text, _) = html_to_text_with_refs(DOCS, "https://docs.example.com/");
    assert!(!text.contains("ignore me"));
}

// --- truncation must not destroy the reference block (priority 1) -----------

#[test]
fn test_truncation_keeps_complete_reference_block() {
    // A long body with several links; total output exceeds the token budget.
    let mut html = String::from("<html><head><title>Big</title></head><body><article>");
    for i in 0..40 {
        html.push_str(&format!(
            "<p>Paragraph number {i} with some filler text to push the body over \
             the token budget, and a <a href=\"https://example.com/page{i}\">link {i}</a> \
             that must remain resolvable.</p>"
        ));
    }
    html.push_str("</article></body></html>");

    let opts = FetchOptions {
        max_tokens: Some(120),
        ..FetchOptions::default()
    };
    let r = convert_body(&html, "https://example.com/", Some("text/html"), &opts);

    // The body was truncated...
    assert!(r.content.contains("…[truncated]"), "content: {}", r.content);
    // ...yet a complete References: block still terminates the output.
    assert!(r.content.contains("References:"), "content: {}", r.content);
    let tail = &r.content[r.content.find("References:").unwrap()..];

    // Every inline [N] marker that survived must resolve to a reference line.
    let re_marker = regex_lite_markers(&r.content[..r.content.find("References:").unwrap()]);
    for n in re_marker {
        assert!(
            tail.contains(&format!("[{n}] ")),
            "marker [{n}] has no reference line; tail: {tail}"
        );
    }
    // And the last reference line is intact (ends with a full URL, not cut off).
    assert!(
        tail.trim_end().ends_with(|c: char| !c.is_whitespace()),
        "refs block looks truncated: {tail}"
    );
    assert!(tail.contains("https://example.com/page"));
}

/// Collect the distinct `[N]` reference indices appearing in `text`.
fn regex_lite_markers(text: &str) -> Vec<usize> {
    let mut out = Vec::new();
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'[' {
            let mut j = i + 1;
            while j < bytes.len() && bytes[j].is_ascii_digit() {
                j += 1;
            }
            if j > i + 1 && j < bytes.len() && bytes[j] == b']' {
                if let Ok(n) = text[i + 1..j].parse::<usize>() {
                    if !out.contains(&n) {
                        out.push(n);
                    }
                }
            }
            i = j;
        } else {
            i += 1;
        }
    }
    out
}

// --- title de-duplication (priority 1) --------------------------------------

#[test]
fn test_leading_title_not_duplicated_in_body() {
    let html = "<html><head><title>Widgets Guide</title></head>\
                <body><article><h1>Widgets Guide</h1>\
                <p>Body content here.</p></article></body></html>";
    let r = convert_html(html, "https://example.com/p", &FetchOptions::default());
    assert_eq!(r.title, "Widgets Guide");
    // The body must not open with a second copy of the title.
    assert!(
        !r.content.trim_start().starts_with("Widgets Guide"),
        "content began with a duplicate title: {:?}",
        r.content
    );
    assert!(r.content.contains("Body content here."));
}

#[test]
fn test_distinct_heading_is_kept() {
    // Title from <title>, different first <h1>: nothing should be dropped.
    let html = "<html><head><title>Site Name</title></head>\
                <body><article><h1>Real Article Heading</h1>\
                <p>Words.</p></article></body></html>";
    let r = convert_html(html, "https://example.com/p", &FetchOptions::default());
    assert_eq!(r.title, "Site Name");
    assert!(r.content.contains("Real Article Heading"));
}

// --- deeply nested DOM (priority 2: O(n) content_root) ----------------------

#[test]
fn test_deeply_nested_dom_extracts_content() {
    // Build a deeply nested div chain ending in real text; the largest
    // text-bearing container heuristic must still find the content.
    let depth = 400;
    let mut html = String::from("<html><body>");
    for _ in 0..depth {
        html.push_str("<div>");
    }
    html.push_str("<p>deep content marker</p>");
    for _ in 0..depth {
        html.push_str("</div>");
    }
    html.push_str("</body></html>");

    let converted = convert(&html, "https://example.com/", ContentType::Text);
    assert!(
        converted.content.contains("deep content marker"),
        "content: {}",
        converted.content
    );
}

// --- format dispatch --------------------------------------------------------

#[test]
fn test_markdown_keeps_links_inline() {
    let converted = convert(BLOG, "https://blog.example.com/post", ContentType::Markdown);
    assert!(converted
        .content
        .contains("[references page](https://blog.example.com/refs)"));
    assert!(converted.content.contains("# Why References Matter"));
}

#[test]
fn test_structured_emits_json_with_references() {
    let converted = convert(
        DOCS,
        "https://docs.example.com/page",
        ContentType::Structured,
    );
    let v: serde_json::Value = serde_json::from_str(&converted.content).unwrap();
    assert!(v["blocks"].is_array());
    assert!(v["references"].is_array());
    assert_eq!(v["references"].as_array().unwrap().len(), 3);
}

#[test]
fn test_spa_shell_yields_empty_body() {
    // No real content; conversion should not panic and produces no references.
    let converted = convert(SPA, "https://spa.example.com/", ContentType::Text);
    assert!(converted.references.is_empty());
    assert!(converted.content.trim().is_empty());
}

// --- media classification + passthrough (non-HTML handling) -----------------

#[test]
fn test_classify_by_header() {
    assert_eq!(classify(Some("text/html; charset=utf-8"), ""), Media::Html);
    assert_eq!(classify(Some("application/json"), ""), Media::Json);
    assert_eq!(classify(Some("text/plain"), ""), Media::Text);
    assert_eq!(
        classify(Some("image/png"), ""),
        Media::Other("image/png".into())
    );
}

#[test]
fn test_classify_by_sniff_when_no_header() {
    assert_eq!(
        classify(None, "  <html><body>hi</body></html>"),
        Media::Html
    );
    assert_eq!(classify(None, "  {\"a\": 1}"), Media::Json);
    assert_eq!(classify(None, "just words"), Media::Text);
    // Looks like JSON but isn't — falls back to text.
    assert_eq!(classify(None, "{not json"), Media::Text);
}

#[test]
fn test_json_passthrough_is_pretty_printed() {
    let opts = FetchOptions::default();
    let r = convert_body(
        "{\"a\":1,\"b\":[2,3]}",
        "https://api.test/x",
        Some("application/json"),
        &opts,
    );
    assert_eq!(r.media, "json");
    assert!(r.references.is_empty());
    // Pretty-printed (indented), not the compact input.
    assert!(r.content.contains("\"a\": 1"), "content: {}", r.content);
}

#[test]
fn test_text_passthrough_is_verbatim() {
    let opts = FetchOptions::default();
    let r = convert_body(
        "# Title\n\nsome *markdown*",
        "https://x.test/readme.md",
        Some("text/markdown"),
        &opts,
    );
    assert_eq!(r.media, "text");
    assert_eq!(r.content, "# Title\n\nsome *markdown*");
}

#[test]
fn test_binary_media_is_summarized_not_rendered() {
    let opts = FetchOptions::default();
    let r = convert_body(
        "\u{0089}PNGblob",
        "https://x.test/a.png",
        Some("image/png"),
        &opts,
    );
    assert_eq!(r.media, "image/png");
    assert!(r.content.contains("not rendered"), "content: {}", r.content);
}

#[test]
fn test_html_path_still_extracts_refs_and_media() {
    let opts = FetchOptions::default();
    let r = convert_body(
        DOCS,
        "https://docs.example.com/page",
        Some("text/html"),
        &opts,
    );
    assert_eq!(r.media, "html");
    assert_eq!(r.references.len(), 3);
    assert!(r.content.contains("users endpoint [1]"));
}

// --- citation metadata ------------------------------------------------------

#[test]
fn test_metadata_extraction() {
    let html = r#"<!DOCTYPE html><html lang="en">
      <head>
        <title>Meta Test</title>
        <meta name="description" content="A short summary.">
        <meta name="author" content="Ada Lovelace">
        <meta property="article:published_time" content="2024-12-01">
        <meta property="og:site_name" content="Example Docs">
      </head>
      <body><article><p>Body.</p></article></body></html>"#;
    let r = convert_html(html, "https://example.com/p", &FetchOptions::default());
    assert_eq!(r.title, "Meta Test");
    assert_eq!(r.metadata.description.as_deref(), Some("A short summary."));
    assert_eq!(r.metadata.author.as_deref(), Some("Ada Lovelace"));
    assert_eq!(r.metadata.published.as_deref(), Some("2024-12-01"));
    assert_eq!(r.metadata.site_name.as_deref(), Some("Example Docs"));
    assert_eq!(r.metadata.lang.as_deref(), Some("en"));
}
