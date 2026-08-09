//! Web search with reference-style URL preservation.
//!
//! Results reuse the same reference-style URL preservation as the fetch path:
//! each hit's title carries an inline `[N]` marker and the full URLs are
//! collected into a reference block, keeping the context window tight while
//! staying citable.
//!
//! DuckDuckGo Lite is the default backend and needs no key, so the tool works
//! with zero configuration. Because it is scraped HTML it is also the least
//! reliable: [`types::SearchStatus`] reports a challenge or unparseable page as
//! a failure instead of an empty result, and [`providers::Provider`] offers
//! keyed alternatives for callers who need a real API contract.

// Shared primitives from webfetch-core; re-exported so internal modules can
// keep using `crate::compress` / `crate::refs`.
pub use webfetch_core::{compress, http, refs, tls};

pub mod extract;
pub mod providers;
pub mod types;

use crate::compress::estimate_tokens;
pub use providers::Provider;
use types::{Reference, SearchOptions, SearchOutput, SearchResult, SearchStatus};

/// Build the reference block (index → URL) from parsed results.
pub fn build_refs(results: &[SearchResult]) -> Vec<Reference> {
    results
        .iter()
        .map(|r| Reference {
            index: r.ref_index,
            url: r.url.clone(),
        })
        .collect()
}

/// Render the inline body: each result as `title [N]` followed by its snippet.
/// URLs are intentionally absent here — they live in the reference block.
pub fn format_results(results: &[SearchResult]) -> String {
    results
        .iter()
        .map(|r| {
            if r.snippet.is_empty() {
                format!("{} [{}]", r.title, r.ref_index)
            } else {
                format!("{} [{}]\n{}", r.title, r.ref_index, r.snippet)
            }
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// Render the reference block appended to text output.
/// Thin wrapper over [`crate::refs::render_block`].
pub fn render_references(refs: &[Reference]) -> String {
    crate::refs::render_block(refs)
}

/// Render the human-readable form of a search: results, then the reference
/// block, then a one-line note when the search did not actually answer.
pub fn render_output(output: &SearchOutput) -> String {
    let mut s = format_results(&output.results);
    let refs = render_references(&output.references);
    if !refs.is_empty() {
        s.push_str(&format!("\n\n{refs}"));
    }
    if let Some(note) = status_note(output) {
        if !s.is_empty() {
            s.push_str("\n\n");
        }
        s.push_str(&note);
    }
    s
}

/// A plain-language explanation of a non-`Ok` status. `None` when results came
/// back normally.
pub fn status_note(output: &SearchOutput) -> Option<String> {
    match output.status {
        SearchStatus::Ok => None,
        SearchStatus::Empty => Some(format!(
            "No results for `{}` (provider: {}).",
            output.query, output.provider
        )),
        SearchStatus::Blocked => Some(format!(
            "Search was blocked or returned an unrecognized page (provider: {}). \
             This is not the same as having no results — the query was not answered.",
            output.provider
        )),
    }
}

/// Assemble an output from parsed results.
pub fn build_output(
    query: &str,
    results: Vec<SearchResult>,
    status: SearchStatus,
    provider: &str,
) -> SearchOutput {
    let references = build_refs(&results);
    let body = format_results(&results);
    let refs_block = render_references(&references);
    let full = if refs_block.is_empty() {
        body
    } else {
        format!("{body}\n\n{refs_block}")
    };

    SearchOutput {
        query: query.to_string(),
        token_estimate: estimate_tokens(&full),
        result_count: results.len(),
        status,
        provider: provider.to_string(),
        references,
        results,
    }
}

/// Parse an already-fetched DuckDuckGo Lite page into a [`SearchOutput`]
/// (no network). Kept for tests and offline callers.
pub fn build_output_from_ddg(query: &str, html: &str, max_results: usize) -> SearchOutput {
    let results = extract::parse_ddg_lite(html, max_results);
    let status = extract::classify_page(html, results.len());
    build_output(query, results, status, "duckduckgo")
}

/// Run a query against the configured provider, falling back to
/// `options.fallback` when the primary errors or is blocked.
///
/// A fallback is never silent: [`SearchOutput::provider`] records which backend
/// actually answered, so a caller can tell a Brave answer from a scraped one.
pub async fn run_search(options: SearchOptions) -> anyhow::Result<SearchOutput> {
    let primary = attempt_provider(&options.provider, &options).await;

    let primary_answered = matches!(&primary, Ok(out) if !out.status.is_failure());
    if primary_answered {
        return primary;
    }

    if let Some(fallback) = &options.fallback {
        if let Ok(out) = attempt_provider(fallback, &options).await {
            if !out.status.is_failure() {
                return Ok(out);
            }
        }
    }
    // No fallback, or the fallback failed too: report the primary's outcome so
    // the error the caller sees is the one they configured for.
    primary
}

async fn attempt_provider(
    provider: &Provider,
    options: &SearchOptions,
) -> anyhow::Result<SearchOutput> {
    let (results, status) = provider.search(options).await?;
    Ok(build_output(
        &options.query,
        results,
        status,
        provider.label(),
    ))
}
