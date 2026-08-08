use websearch::extract::{parse_ddg_lite, resolve_result_url};
use websearch::types::SearchStatus;
use websearch::{build_output_from_ddg, build_refs, format_results, render_references};

const DDG: &str = include_str!("fixtures/ddg_lite.html");

// --- URL resolution ---------------------------------------------------------

#[test]
fn test_resolve_uddg_redirect() {
    let href =
        "//duckduckgo.com/l/?uddg=https%3A%2F%2Freact.dev%2Fblog%2F2024%2F12%2F01%2Freact-19&rut=x";
    assert_eq!(
        resolve_result_url(href),
        "https://react.dev/blog/2024/12/01/react-19"
    );
}

#[test]
fn test_resolve_protocol_relative() {
    assert_eq!(
        resolve_result_url("//example.com/path"),
        "https://example.com/path"
    );
}

#[test]
fn test_resolve_absolute_passthrough() {
    assert_eq!(
        resolve_result_url("https://example.com/x"),
        "https://example.com/x"
    );
}

// --- parsing ----------------------------------------------------------------

#[test]
fn test_parse_extracts_results() {
    let results = parse_ddg_lite(DDG, 10);
    assert_eq!(results.len(), 3);

    let first = &results[0];
    assert_eq!(first.title, "React 19 – React");
    assert_eq!(first.url, "https://react.dev/blog/2024/12/01/react-19");
    assert_eq!(first.ref_index, 1);
    // Snippet whitespace collapsed and the decorative glyph stripped.
    assert_eq!(
        first.snippet,
        "React 19 introduces the new use hook for data fetching and more APIs."
    );

    assert_eq!(
        results[1].url,
        "https://nextjs.org/blog/partial-prerendering"
    );
    assert_eq!(results[1].ref_index, 2);
}

#[test]
fn test_max_results_caps_output() {
    let results = parse_ddg_lite(DDG, 2);
    assert_eq!(results.len(), 2);
}

// --- reference-style output --------------------------------------------------

#[test]
fn test_build_refs_indices_match() {
    let results = parse_ddg_lite(DDG, 10);
    let refs = build_refs(&results);
    assert_eq!(refs.len(), 3);
    assert_eq!(refs[0].index, 1);
    assert_eq!(refs[2].url, "https://example.com/third");
}

#[test]
fn test_format_results_uses_inline_markers() {
    let results = parse_ddg_lite(DDG, 10);
    let body = format_results(&results);
    assert!(body.contains("React 19 – React [1]"), "body: {body}");
    assert!(body.contains("Partial Prerendering – Next.js [2]"));
    // URLs must NOT appear inline — they belong in the reference block.
    assert!(!body.contains("https://react.dev"), "body: {body}");
}

#[test]
fn test_render_references_block() {
    let results = parse_ddg_lite(DDG, 2);
    let refs = build_refs(&results);
    let block = render_references(&refs);
    assert_eq!(
        block,
        "References:\n[1] https://react.dev/blog/2024/12/01/react-19\n[2] https://nextjs.org/blog/partial-prerendering"
    );
}

#[test]
fn test_build_output_end_to_end() {
    let out = build_output_from_ddg("react 19", DDG, 5);
    assert_eq!(out.query, "react 19");
    assert_eq!(out.result_count, 3);
    assert_eq!(out.references.len(), 3);
    assert!(out.token_estimate > 0);
    assert_eq!(out.status, SearchStatus::Ok);
    assert_eq!(out.provider, "duckduckgo");

    // Round-trips through JSON cleanly.
    let json = serde_json::to_string(&out).unwrap();
    let v: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(v["results"][0]["ref_index"], 1);
    assert_eq!(v["status"], "ok");
}

/// A page we cannot parse is a failure, not a query with no answer. Reporting
/// it as an empty success is what let a rate-limited search look like "the web
/// has nothing on this".
#[test]
fn test_unparseable_page_is_reported_as_blocked() {
    let out = build_output_from_ddg("nothing", "<html><body>no results</body></html>", 5);
    assert_eq!(out.result_count, 0);
    assert!(out.references.is_empty());
    assert_eq!(out.status, SearchStatus::Blocked);
    assert!(out.status.is_failure());
}

#[test]
fn test_real_results_page_with_no_hits_is_empty() {
    let html = "<html><body><table><tr><td class=\"result-count\">No results.</td>\
                </tr></table></body></html>";
    let out = build_output_from_ddg("zzz", html, 5);
    assert_eq!(out.status, SearchStatus::Empty);
    assert!(!out.status.is_failure());
}
