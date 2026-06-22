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

/// Fast token approximation: ~4 characters per token, matching common
/// BPE tokenizers closely enough for budgeting.
pub fn estimate_tokens(text: &str) -> usize {
    text.len() / 4
}

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
