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
    truncate_to_tokens_at(text, max_tokens).0
}

/// [`truncate_to_tokens`], also reporting how many bytes of `text` the returned
/// prefix consumed.
///
/// Callers that page through a document need the resume point to be exact. An
/// estimate of what came back cannot supply it: [`estimate_tokens`] is lossy in
/// both directions, so re-deriving a byte position from a token count drifts,
/// and successive windows then overlap or skip text outright. The cut position
/// is known here and nowhere else, so it is returned rather than reconstructed.
///
/// The count covers only `text`; the elision marker is not part of it.
pub fn truncate_to_tokens_at(text: &str, max_tokens: usize) -> (String, usize) {
    if estimate_tokens(text) <= max_tokens {
        return (text.to_string(), text.len());
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
    end = snap_to_boundary(text, end);
    // A budget smaller than the marker leaves no room for content at all, and a
    // window that consumes nothing is worse than a small one: a caller paging by
    // the reported consumption would ask for the same offset forever. Always
    // make one character of progress.
    if end == 0 {
        end = text
            .char_indices()
            .nth(1)
            .map(|(i, _)| i)
            .unwrap_or(text.len());
    }
    (format!("{}{}", &text[..end], TRUNCATION_MARKER), end)
}

/// How far back a cut may be pulled to land on a natural boundary, as a
/// fraction of the window. Beyond this the text has no usable break (a long
/// unbroken token, CJK without spaces) and the hard cut stands.
const SNAP_BACK_FRACTION: usize = 5;

/// Pull a cut back to the nearest paragraph, line, or word boundary.
///
/// A window that ends mid-word reads as corrupted rather than continued, and
/// the next window opens on the tail of a word nobody can parse. Snapping
/// costs a few tokens of the budget and keeps both halves readable. It cannot
/// desynchronize paging: the resume point is the consumption actually returned,
/// so a shorter window simply means the next one starts earlier.
fn snap_to_boundary(text: &str, end: usize) -> usize {
    if end == 0 || end >= text.len() {
        return end;
    }
    let floor = end - end / SNAP_BACK_FRACTION;
    let head = &text[..end];
    // Paragraph break first, then any newline, then a space: prefer the break
    // that leaves the most self-contained window.
    for pattern in ["\n\n", "\n", " "] {
        if let Some(at) = head.rfind(pattern) {
            let cut = at + pattern.len();
            if cut >= floor {
                return cut;
            }
        }
    }
    end
}

/// The remainder of `text` from a byte `offset`, for reading a long document a
/// window at a time.
///
/// The offset is clamped to the document and snapped back to a character
/// boundary, so an offset from any source is safe to pass: a caller that
/// resumes from a stale or hand-written position gets a shorter window, never a
/// panic. Offsets address the *extracted* text, not the source bytes, so they
/// stay stable across output formats of the same document.
pub fn window_from(text: &str, offset: usize) -> &str {
    let mut start = offset.min(text.len());
    while start > 0 && !text.is_char_boundary(start) {
        start -= 1;
    }
    &text[start..]
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

    /// The property paging rests on: walking the reported consumption from one
    /// window to the next reproduces the document exactly — no repeated
    /// sentence, no silently skipped paragraph.
    #[test]
    fn windows_tile_the_document_exactly() {
        let text = "Sentence number one is here. ".repeat(400);
        let mut offset = 0usize;
        let mut seen = String::new();
        let mut windows = 0;

        while offset < text.len() {
            let remainder = window_from(&text, offset);
            let (_, consumed) = truncate_to_tokens_at(remainder, 60);
            assert!(consumed > 0, "a window must make progress");
            seen.push_str(&remainder[..consumed]);
            offset += consumed;
            windows += 1;
            assert!(windows < 1000, "paging failed to terminate");
        }

        assert!(windows > 5, "expected several windows, got {windows}");
        assert_eq!(seen, text);
    }

    /// A budget too small to hold even the elision marker must still advance,
    /// or a caller following the consumption never terminates.
    #[test]
    fn a_pathologically_small_budget_still_makes_progress() {
        let text = "ünicode text that will not fit";
        let (out, consumed) = truncate_to_tokens_at(text, 1);
        assert!(consumed > 0);
        assert!(text.is_char_boundary(consumed));
        assert!(out.starts_with('ü'));
    }

    /// A window that ends mid-word reads as corrupted, and hands the next
    /// window a fragment nobody can parse.
    #[test]
    fn a_window_ends_on_a_word_boundary_when_one_is_near() {
        let text = "alpha beta gamma delta epsilon zeta eta theta iota kappa lambda mu ".repeat(20);
        let (_, consumed) = truncate_to_tokens_at(&text, 20);
        assert!(
            text[..consumed].ends_with(' '),
            "cut mid-word: {:?}",
            &text[consumed.saturating_sub(12)..consumed]
        );
        // The next window opens on a whole word, not a suffix of one.
        assert!(window_from(&text, consumed).starts_with(char::is_alphabetic));
    }

    /// Text with no break inside reach still gets a window: a hard cut beats
    /// returning nothing.
    #[test]
    fn text_without_a_boundary_still_yields_a_window() {
        let text = "x".repeat(500);
        let (_, consumed) = truncate_to_tokens_at(&text, 20);
        assert!(consumed > 0);
    }

    #[test]
    fn consumption_covers_the_whole_text_when_nothing_is_cut() {
        let text = "well within budget";
        let (out, consumed) = truncate_to_tokens_at(text, 1000);
        assert_eq!(out, text);
        assert_eq!(consumed, text.len());
    }

    /// A stale or hand-written offset is a shorter window, never a panic.
    #[test]
    fn window_clamps_past_the_end_and_snaps_to_a_char_boundary() {
        let text = "héllo wörld";
        assert_eq!(window_from(text, text.len() + 500), "");
        // Byte 2 is inside "é" (bytes 1..3): snapping back keeps the slice legal.
        assert_eq!(window_from(text, 2), &text[1..]);
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
