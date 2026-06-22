use webfetch::compress::{compress_text, estimate_tokens, truncate_to_tokens};
use webfetch::convert::convert;
use webfetch::convert::text::{html_to_text_with_refs, render_references};
use webfetch::types::ContentType;

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
