use once_cell::sync::Lazy;
use regex::Regex;

static WHITESPACE_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"\s+").unwrap());
static DECORATIVE_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"[▶→←▼▲•·◆◇◊✓✗✔✘‣⁃◦]").unwrap());

/// Semantic text reduction: strip decorative glyphs, then collapse runs of
/// whitespace, then trim.
///
/// Order matters — decorative characters are removed *before* collapsing
/// whitespace so that a glyph surrounded by spaces (e.g. `"Click ▶ to play"`)
/// does not leave a double space behind.
pub fn compress_text(text: &str) -> String {
    let clean = DECORATIVE_RE.replace_all(text, "");
    let collapsed = WHITESPACE_RE.replace_all(&clean, " ");
    collapsed.trim().to_string()
}

/// Collapse repeated blank lines while preserving paragraph breaks, and
/// compress whitespace within each line.
pub fn compress_block(text: &str) -> String {
    let mut lines: Vec<String> = Vec::new();
    let mut prev_blank = false;
    for raw in text.lines() {
        let line = compress_text(raw);
        let blank = line.is_empty();
        if blank && prev_blank {
            continue;
        }
        lines.push(line);
        prev_blank = blank;
    }
    lines.join("\n").trim().to_string()
}

/// Is this byte one of the punctuation characters a BPE tokenizer almost
/// always splits on? See [`estimate_tokens`].
fn is_url_punct(b: u8) -> bool {
    matches!(
        b,
        b'/' | b':' | b'.' | b'?' | b'#' | b'&' | b'=' | b'%' | b'~'
    )
}

/// Token cost of `text` in *quarter-tokens*, the unit both [`estimate_tokens`]
/// and [`truncate_to_tokens`] work in so the two can never disagree.
///
/// One byte costs one quarter-token (the ~4-chars-per-token rule); each URL
/// punctuation byte costs two extra (the half-token surcharge).
fn cost_quarters(text: &str) -> usize {
    text.len() + 2 * text.bytes().filter(|b| is_url_punct(*b)).count()
}

/// Fast token approximation.
///
/// Prose is ~4 characters per token, which matches common BPE tokenizers
/// closely enough for budgeting. URLs and reference blocks, however, are
/// punctuation-dense — BPE breaks on `/ : . ? # & = % ~`, so a URL yields far
/// more tokens per character than prose and a naive `len/4` badly
/// *under*-budgets them. We therefore add a surcharge of half a token per such
/// punctuation byte, which pushes URL-heavy text (the trailing reference block
/// especially) toward its true token count while leaving prose essentially
/// unchanged. The heuristic is deterministic and a single linear scan.
pub fn estimate_tokens(text: &str) -> usize {
    cost_quarters(text) / 4
}

/// The elision marker appended when [`truncate_to_tokens`] drops content.
pub const TRUNCATION_MARKER: &str = "\n…[truncated]";

/// Truncate text to roughly `max_tokens`, on a character boundary, appending
/// an elision marker when content is dropped.
///
/// The prefix is chosen with the *same* cost model [`estimate_tokens`] uses, so
/// `estimate_tokens(truncate_to_tokens(t, n)) <= n` holds for the returned body
/// even on punctuation-dense text. (A naive `max_tokens * 4` character cut
/// ignores the URL surcharge and overshoots badly on link-heavy pages.)
pub fn truncate_to_tokens(text: &str, max_tokens: usize) -> String {
    if estimate_tokens(text) <= max_tokens {
        return text.to_string();
    }
    // Reserve room for the marker so the returned string still fits the budget.
    let budget = (max_tokens * 4).saturating_sub(cost_quarters(TRUNCATION_MARKER));

    let mut spent = 0usize;
    let mut end = 0usize;
    for (i, b) in text.bytes().enumerate() {
        let next = spent + if is_url_punct(b) { 3 } else { 1 };
        if next > budget {
            break;
        }
        spent = next;
        end = i + 1;
    }
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}{}", &text[..end], TRUNCATION_MARKER)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn estimate_unchanged_for_plain_prose() {
        assert_eq!(estimate_tokens(&"a".repeat(100)), 25);
    }

    #[test]
    fn url_heavy_text_estimates_higher_than_prose_of_same_length() {
        // Four reference lines: punctuation-dense URLs.
        let urls = "[1] https://example.com/a/b?c=d#e\n\
                    [2] https://example.org/x/y/z?q=1\n\
                    [3] https://sub.example.net/path/to/thing\n\
                    [4] https://example.io/foo/bar/baz?k=v";
        // Same byte length, but plain prose (no URL punctuation).
        let prose = "x".repeat(urls.len());
        assert_eq!(urls.len(), prose.len());
        assert!(
            estimate_tokens(urls) > estimate_tokens(&prose),
            "urls={} prose={}",
            estimate_tokens(urls),
            estimate_tokens(&prose)
        );
    }

    #[test]
    fn truncate_respects_the_budget_it_reports() {
        let text = "a".repeat(1000);
        let out = truncate_to_tokens(&text, 20);
        assert!(out.ends_with(TRUNCATION_MARKER));
        assert!(
            estimate_tokens(&out) <= 20,
            "estimate {}",
            estimate_tokens(&out)
        );
    }

    /// The old `max_tokens * 4` character cut ignored the URL surcharge, so
    /// punctuation-dense text came back well over budget.
    #[test]
    fn truncate_respects_budget_on_url_heavy_text() {
        let urls = "[1] https://example.com/a/b?c=d#e\n".repeat(200);
        let out = truncate_to_tokens(&urls, 50);
        assert!(
            estimate_tokens(&out) <= 50,
            "estimate {}",
            estimate_tokens(&out)
        );
    }

    #[test]
    fn truncate_is_a_noop_within_budget() {
        let text = "short enough";
        assert_eq!(truncate_to_tokens(text, 1000), text);
    }

    #[test]
    fn truncate_never_splits_a_utf8_char() {
        let text = "é".repeat(500);
        let out = truncate_to_tokens(&text, 10);
        // Round-trips as valid UTF-8 (the type system guarantees it, but the
        // boundary walk is what makes the slice legal in the first place).
        assert!(out.starts_with('é'));
        assert!(estimate_tokens(&out) <= 10);
    }
}
