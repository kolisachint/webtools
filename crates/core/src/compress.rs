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
    let base = text.len() / 4;
    let url_punct = text
        .bytes()
        .filter(|b| {
            matches!(
                b,
                b'/' | b':' | b'.' | b'?' | b'#' | b'&' | b'=' | b'%' | b'~'
            )
        })
        .count();
    base + url_punct / 2
}

/// The smallest body budget we will ever leave after reserving room for a
/// reference block, so that a page dominated by links still shows *some* body.
const MIN_BODY_TOKENS: usize = 64;

/// Truncate text to roughly `max_tokens`, on a character boundary, appending
/// an elision marker when content is dropped.
pub fn truncate_to_tokens(text: &str, max_tokens: usize) -> String {
    let max_chars = max_tokens.saturating_mul(4);
    if text.len() <= max_chars {
        return text.to_string();
    }
    let mut end = max_chars;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}\n…[truncated]", &text[..end])
}

/// Truncate `content` to `max_tokens` while keeping a trailing `refs_block`
/// (a rendered `References:` list) intact.
///
/// Reference-style output appends the block to the end of the content; a plain
/// `truncate_to_tokens` over the whole string would cut the references off,
/// leaving inline `[N]` markers that resolve to nothing. Instead we strip the
/// block, truncate only the body to the budget left after reserving room for
/// the block (floored at [`MIN_BODY_TOKENS`] so a link-heavy page keeps some
/// body), then re-append the block whole.
pub fn truncate_preserving_refs(content: &str, refs_block: &str, max_tokens: usize) -> String {
    if refs_block.is_empty() {
        return truncate_to_tokens(content, max_tokens);
    }
    // The block sits at the very end, joined to the body by a blank line.
    let body = content
        .strip_suffix(refs_block)
        .map(|b| b.trim_end_matches('\n'))
        .unwrap_or(content);

    let refs_tokens = estimate_tokens(refs_block);
    let body_budget = max_tokens.saturating_sub(refs_tokens).max(MIN_BODY_TOKENS);
    let body = truncate_to_tokens(body, body_budget);
    format!("{body}\n\n{refs_block}")
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
    fn preserving_refs_keeps_block_intact_when_body_truncated() {
        let refs_block = "References:\n[1] https://example.com/a\n[2] https://example.com/b";
        let body = "word ".repeat(500); // far over budget
        let content = format!("{}\n\n{}", body.trim_end(), refs_block);
        let out = truncate_preserving_refs(&content, refs_block, 80);
        assert!(out.contains("…[truncated]"), "out: {out}");
        // The full reference block survives at the very end.
        assert!(out.ends_with(refs_block), "out tail: {:?}", out);
    }

    #[test]
    fn preserving_refs_is_noop_when_within_budget() {
        let refs_block = "References:\n[1] https://example.com/a";
        let content = format!("short body\n\n{refs_block}");
        let out = truncate_preserving_refs(&content, refs_block, 10_000);
        assert_eq!(out, content);
    }

    #[test]
    fn preserving_refs_without_block_falls_back_to_plain_truncate() {
        let content = "z".repeat(1000);
        let out = truncate_preserving_refs(&content, "", 10);
        assert_eq!(out, truncate_to_tokens(&content, 10));
    }
}
