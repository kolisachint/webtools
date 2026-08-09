//! A pre-parse bound on document complexity.
//!
//! html5ever's tree builder rescans its stack of open elements when it inserts
//! certain tags, so parse time grows quadratically with nesting depth. Measured
//! on this crate: 4 000 nested `<div>`s parse in 0.09 s, 16 000 in 1.7 s, and
//! 200 000 — a 2.2 MB file, comfortably inside the 5 MiB body cap — took over
//! four minutes. Neither the body cap nor `--timeout` helps: the cap counts
//! bytes, and the timeout covers the HTTP request, not the parse that follows.
//! On the MCP server that stall blocks every other request.
//!
//! So depth is measured up front, in one linear scan, and a document past the
//! limit is refused rather than parsed. Real pages sit around depth 20-50;
//! [`MAX_NESTING_DEPTH`] is far above anything a document written for humans
//! reaches, so the check only fires on pathological input.

/// Nesting depth past which a document is refused.
pub const MAX_NESTING_DEPTH: usize = 10_000;

/// HTML void elements: they never open a level.
const VOID_ELEMENTS: [&str; 14] = [
    "area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "param", "source",
    "track", "wbr",
];

/// Estimate a document's maximum element nesting depth.
///
/// A tag scan, not a parse — the whole point is to answer before paying for a
/// parse. It ignores the implicit closes a real parser performs (`<p>` inside
/// `<p>`, unclosed `<li>`), so it can *over*-estimate on sloppy markup; that is
/// the safe direction only because the limit sits so far above real documents.
pub fn max_nesting_depth(html: &str) -> usize {
    let bytes = html.as_bytes();
    let mut depth: isize = 0;
    let mut max: isize = 0;
    let mut i = 0;

    while i < bytes.len() {
        if bytes[i] != b'<' {
            i += 1;
            continue;
        }
        let Some(next) = bytes.get(i + 1) else { break };

        // Comments, doctypes and processing instructions open nothing. A
        // comment ends at `-->`, not at the first `>` — markup inside one must
        // not be counted.
        if bytes[i..].starts_with(b"<!--") {
            i = match find_slice(bytes, i + 4, b"-->") {
                Some(end) => end + 3,
                None => break,
            };
            continue;
        }
        if matches!(next, b'!' | b'?') {
            i = match find_byte(bytes, i, b'>') {
                Some(end) => end + 1,
                None => break,
            };
            continue;
        }

        let closing = *next == b'/';
        let name_start = if closing { i + 2 } else { i + 1 };
        let mut name_end = name_start;
        while name_end < bytes.len() && bytes[name_end].is_ascii_alphanumeric() {
            name_end += 1;
        }
        if name_end == name_start {
            i += 1;
            continue;
        }
        let Some(tag_end) = find_byte(bytes, name_end, b'>') else {
            break;
        };
        let name = html[name_start..name_end].to_ascii_lowercase();
        let self_closing = tag_end > 0 && bytes[tag_end - 1] == b'/';

        if closing {
            depth -= 1;
        } else if !self_closing && !VOID_ELEMENTS.contains(&name.as_str()) {
            depth += 1;
            max = max.max(depth);
        }
        i = tag_end + 1;
    }

    max.max(0) as usize
}

/// Is this document too deeply nested to parse within a sane time budget?
pub fn too_deeply_nested(html: &str) -> Option<usize> {
    let depth = max_nesting_depth(html);
    (depth > MAX_NESTING_DEPTH).then_some(depth)
}

fn find_byte(bytes: &[u8], from: usize, needle: u8) -> Option<usize> {
    bytes[from..]
        .iter()
        .position(|b| *b == needle)
        .map(|p| p + from)
}

fn find_slice(bytes: &[u8], from: usize, needle: &[u8]) -> Option<usize> {
    if from >= bytes.len() {
        return None;
    }
    bytes[from..]
        .windows(needle.len())
        .position(|w| w == needle)
        .map(|p| p + from)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ordinary_documents_are_shallow() {
        let html = "<html><body><div><section><article><p>Hi <b>there</b></p>\
                    </article></section></div></body></html>";
        assert!(max_nesting_depth(html) < 10, "{}", max_nesting_depth(html));
        assert_eq!(too_deeply_nested(html), None);
    }

    #[test]
    fn void_and_self_closing_tags_do_not_nest() {
        let html = "<div><br><img src=x><hr><input><meta charset=utf-8><span/></div>";
        assert_eq!(max_nesting_depth(html), 1);
    }

    #[test]
    fn comments_and_doctype_are_ignored() {
        let html = "<!DOCTYPE html><!-- <div><div><div> --><p>x</p>";
        assert_eq!(max_nesting_depth(html), 1);
    }

    #[test]
    fn depth_is_counted() {
        let n = 500;
        let html = format!("{}<p>x</p>{}", "<div>".repeat(n), "</div>".repeat(n));
        assert_eq!(max_nesting_depth(&html), n + 1);
    }

    #[test]
    fn pathological_nesting_is_refused() {
        let n = MAX_NESTING_DEPTH + 1;
        let html = format!("{}text{}", "<div>".repeat(n), "</div>".repeat(n));
        assert_eq!(too_deeply_nested(&html), Some(n));
    }
}
