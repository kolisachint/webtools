//! Shared reference-style URL preservation.
//!
//! Both the fetch path and the search path cite URLs with inline `[N]` markers
//! and collect the full URLs into a trailing block. This module owns the one
//! canonical rendering of that block, and the budgeting rule that keeps the
//! block and the body it belongs to inside a token cap together.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::compress::{estimate_tokens, truncate_to_tokens};

/// Anything that can be listed in a reference block: an index and a URL.
pub trait Referable {
    fn index(&self) -> usize;
    fn url(&self) -> &str;
}

impl<T: Referable> Referable for &T {
    fn index(&self) -> usize {
        (*self).index()
    }
    fn url(&self) -> &str {
        (*self).url()
    }
}

/// A slim reference entry (index → URL) for an output's reference block.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Reference {
    pub index: usize,
    pub url: String,
}

impl Referable for Reference {
    fn index(&self) -> usize {
        self.index
    }
    fn url(&self) -> &str {
        &self.url
    }
}

/// Render references into the canonical block:
///
/// ```text
/// References:
/// [1] https://example.com/a
/// [2] https://example.com/b
/// ```
///
/// Returns an empty string when there are no references.
pub fn render_block<T: Referable>(references: &[T]) -> String {
    if references.is_empty() {
        return String::new();
    }
    let mut s = String::from("References:\n");
    for r in references {
        s.push_str(&format!("[{}] {}\n", r.index(), r.url()));
    }
    s.truncate(s.trim_end().len());
    s
}

/// The smallest body budget we will ever leave, so a page dominated by links
/// still shows *some* body rather than collapsing to a bare reference list.
const MIN_BODY_TOKENS: usize = 64;

/// How many times we re-shrink the body budget trying to fit body + block.
/// Each pass drops uncited references, which shrinks the block, which frees
/// budget — the sequence is monotone and converges in two or three passes.
const FIT_PASSES: usize = 6;

/// Collect the distinct `[N]` reference indices cited in `text`, in order.
pub fn cited_indices(text: &str) -> BTreeSet<usize> {
    let mut out = BTreeSet::new();
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != b'[' {
            i += 1;
            continue;
        }
        let mut j = i + 1;
        while j < bytes.len() && bytes[j].is_ascii_digit() {
            j += 1;
        }
        if j > i + 1 && j < bytes.len() && bytes[j] == b']' {
            if let Ok(n) = text[i + 1..j].parse::<usize>() {
                out.insert(n);
            }
        }
        i = j.max(i + 1);
    }
    out
}

/// Join a body and a rendered reference block into final output.
fn assemble(body: &str, block: &str) -> String {
    if block.is_empty() {
        body.to_string()
    } else {
        format!("{body}\n\n{block}")
    }
}

/// Fit `body` plus its reference block inside `max_tokens`.
///
/// Returns the assembled content and the reference indices it kept.
///
/// The old rule reserved room for the *whole* reference block and then appended
/// it regardless of size, so a link-dense page blew straight through the cap
/// (a 120-link page answered `--max-tokens 200` with ~3300 tokens). The rule
/// here is the other way round: truncate the body first, then keep only the
/// references the surviving text still cites. Dropping a reference nobody cites
/// costs nothing and is usually enough on its own; if the block still does not
/// fit, references are dropped from the tail so the cap holds.
///
/// With `max_tokens == None` nothing is truncated and every reference is kept.
pub fn fit_to_budget<T: Referable>(
    body: &str,
    references: &[T],
    max_tokens: Option<usize>,
) -> (String, Vec<usize>) {
    let all = || references.iter().map(Referable::index).collect::<Vec<_>>();

    let Some(max_tokens) = max_tokens else {
        return (assemble(body, &render_block(references)), all());
    };

    let mut budget = max_tokens;
    for pass in 0..FIT_PASSES {
        let body = truncate_to_tokens(body, budget);
        let cited = cited_indices(&body);
        let kept: Vec<&T> = references
            .iter()
            .filter(|r| cited.contains(&r.index()))
            .collect();
        let block = render_block(&kept);
        let content = assemble(&body, &block);
        let total = estimate_tokens(&content);

        if total <= max_tokens {
            return (content, kept.iter().map(|r| r.index()).collect());
        }
        if pass + 1 == FIT_PASSES || budget <= MIN_BODY_TOKENS {
            // The block alone is over budget (very long URLs, very small cap).
            // Drop references from the tail until the whole thing fits.
            return drop_until_fits(&body, &kept, max_tokens);
        }
        // Scale the body budget by how far over we landed, rather than
        // subtracting the overshoot. Subtracting punishes the body for the
        // reference block's size — one long block drove the budget straight to
        // the floor and answered a 1000-token cap with under 300 tokens of
        // output. Scaling converges on the cap from below in two or three
        // passes instead.
        let scaled = (budget as u128 * max_tokens as u128 / total as u128) as usize;
        let next = scaled.max(MIN_BODY_TOKENS);
        if next >= budget {
            // Not converging (already at the floor): stop shrinking the body
            // and take references off the tail instead.
            return drop_until_fits(&body, &kept, max_tokens);
        }
        budget = next;
    }
    unreachable!("the loop returns on its final pass")
}

/// Last resort: shrink the reference block itself, tail first. Markers for
/// dropped references stop resolving, which is a worse output than a complete
/// block — but a silently ignored token cap is worse still.
fn drop_until_fits<T: Referable>(
    body: &str,
    kept: &[&T],
    max_tokens: usize,
) -> (String, Vec<usize>) {
    let mut kept = kept.to_vec();
    while !kept.is_empty() {
        let content = assemble(body, &render_block(&kept));
        if estimate_tokens(&content) <= max_tokens {
            return (content, kept.iter().map(|r| r.index()).collect());
        }
        kept.pop();
    }
    (body.to_string(), Vec::new())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn refs(n: usize) -> Vec<Reference> {
        (1..=n)
            .map(|i| Reference {
                index: i,
                url: format!("https://example.com/very/long/path/segment/{i}?query=value#frag"),
            })
            .collect()
    }

    fn body_citing(n: usize) -> String {
        (1..=n)
            .map(|i| format!("Item {i} with filler prose describing the thing [{i}]."))
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn cited_indices_finds_markers() {
        let got = cited_indices("a [1] b [12] c [x] d [3]");
        assert_eq!(got.into_iter().collect::<Vec<_>>(), vec![1, 3, 12]);
    }

    #[test]
    fn no_budget_keeps_everything() {
        let (content, kept) = fit_to_budget(&body_citing(3), &refs(3), None);
        assert_eq!(kept, vec![1, 2, 3]);
        assert!(content.contains("References:"));
        assert!(content.contains("[3] https://example.com"));
    }

    /// The regression this function exists for: a link-dense page must not
    /// answer a small budget with a full reference block.
    #[test]
    fn link_dense_page_respects_the_cap() {
        let refs = refs(120);
        let (content, kept) = fit_to_budget(&body_citing(120), &refs, Some(200));
        assert!(
            estimate_tokens(&content) <= 200,
            "estimate {} content: {content}",
            estimate_tokens(&content)
        );
        assert!(kept.len() < 120, "kept {} of 120", kept.len());
    }

    #[test]
    fn every_kept_reference_is_still_cited() {
        let refs = refs(120);
        let (content, kept) = fit_to_budget(&body_citing(120), &refs, Some(300));
        let body = content.split("References:").next().unwrap();
        let cited = cited_indices(body);
        for index in &kept {
            assert!(cited.contains(index), "kept [{index}] is not cited");
        }
    }

    /// Fitting the cap is necessary but not sufficient: answering a 1000-token
    /// budget with 280 tokens wastes most of the caller's allowance.
    #[test]
    fn a_generous_budget_is_actually_used() {
        let refs = refs(120);
        let (content, _) = fit_to_budget(&body_citing(120), &refs, Some(1000));
        let used = estimate_tokens(&content);
        assert!(used <= 1000, "over budget: {used}");
        assert!(used >= 800, "left {} of 1000 tokens unused", 1000 - used);
    }

    #[test]
    fn tiny_budget_still_fits() {
        let refs = refs(40);
        let (content, _) = fit_to_budget(&body_citing(40), &refs, Some(80));
        assert!(
            estimate_tokens(&content) <= 80,
            "estimate {}",
            estimate_tokens(&content)
        );
    }

    #[test]
    fn generous_budget_is_a_noop() {
        let refs = refs(3);
        let body = body_citing(3);
        let (content, kept) = fit_to_budget(&body, &refs, Some(100_000));
        assert_eq!(kept, vec![1, 2, 3]);
        assert_eq!(content, assemble(&body, &render_block(&refs)));
    }

    #[test]
    fn body_without_references_is_plain_truncation() {
        let empty: [Reference; 0] = [];
        let (content, kept) = fit_to_budget(&"word ".repeat(500), &empty, Some(50));
        assert!(kept.is_empty());
        assert!(content.contains("…[truncated]"));
        assert!(estimate_tokens(&content) <= 50);
    }
}
