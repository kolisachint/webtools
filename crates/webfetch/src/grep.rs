//! Find where a page mentions something, without reading the page.
//!
//! Paging reads a document in order and [`crate::outline`] maps it by heading,
//! but neither answers "where does this mention rate limiting" on a document
//! whose headings do not say so — or that has no headings at all. Matching
//! against the extracted text does, for the cost of the matches themselves.
//!
//! Match offsets are the offsets [`crate::types::FetchOptions::offset`] reads,
//! like outline offsets, so a hit is followed by fetching at it. The snippet is
//! a locator, not the content: it exists to let a caller judge which hit is
//! worth reading, and the reading is done by the offset.

use regex::{Regex, RegexBuilder};
use serde::{Deserialize, Serialize};

use crate::compress::estimate_tokens;
use crate::outline::Section;

/// Characters of surrounding text carried with a hit, enough to judge it by.
const SNIPPET_CHARS: usize = 160;

/// Bound on the compiled pattern, so a pathological one is refused rather than
/// allocating without limit. The matching itself cannot blow up: this crate's
/// regex engine has no backtracking and runs in time linear in the input.
const PATTERN_SIZE_LIMIT: usize = 1 << 20;

/// One place the page mentions the pattern.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Match {
    /// Byte offset of the match in the extracted text. Read here.
    pub offset: usize,
    /// Text around the match, for judging which hit is worth reading.
    pub snippet: String,
    /// The heading whose section contains the hit, when the page has one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub section: Option<String>,
    /// Further occurrences close enough that this snippet already covers them.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub nearby: usize,
}

fn is_zero(n: &usize) -> bool {
    *n == 0
}

/// Compile a search pattern, case-insensitively unless it carries an uppercase
/// letter.
///
/// Smart case, as every search tool a caller has used already behaves: a
/// lowercase pattern is a loose search, and typing a capital is how you say you
/// meant it. Making case-sensitivity a separate flag would be a knob for
/// something the pattern already expresses.
pub fn compile(pattern: &str) -> Result<Regex, regex::Error> {
    let has_uppercase = pattern.chars().any(char::is_uppercase);
    RegexBuilder::new(pattern)
        .case_insensitive(!has_uppercase)
        .size_limit(PATTERN_SIZE_LIMIT)
        .build()
}

/// Where `pattern` occurs, one hit per neighbourhood, with the section each
/// falls in.
///
/// Occurrences closer together than a snippet are collapsed into the first,
/// carrying a count of the rest. A term repeated through one paragraph would
/// otherwise return a row per occurrence, each snippet largely quoting the
/// last — many tokens spent restating one location. The count keeps that
/// honest: the neighbourhood is reported as busy rather than as a single
/// mention.
pub fn grep(body_text: &str, pattern: &Regex, sections: &[Section]) -> Vec<Match> {
    let mut hits: Vec<Match> = Vec::new();
    for m in pattern.find_iter(body_text) {
        let section = section_for(sections, m.start());
        if let Some(last) = hits.last_mut() {
            // A section boundary is the document's own claim that this is
            // somewhere else, so it ends a neighbourhood however close the two
            // occurrences happen to sit.
            let same_place =
                last.section == section && m.start().saturating_sub(last.offset) < SNIPPET_CHARS;
            if same_place {
                last.nearby += 1;
                continue;
            }
        }
        hits.push(Match {
            offset: m.start(),
            snippet: snippet_around(body_text, m.start(), m.end()),
            section,
            nearby: 0,
        });
    }
    hits
}

/// The heading whose section contains `offset`: the last one starting at or
/// before it, since sections run from their heading to the next.
fn section_for(sections: &[Section], offset: usize) -> Option<String> {
    sections
        .iter()
        .rev()
        .find(|s| s.offset <= offset)
        .map(|s| s.title.clone())
}

/// Text around a match, widened to whole words and flattened to one line.
///
/// Snapping to whitespace keeps a snippet from opening or closing mid-word,
/// which reads as corruption; flattening keeps one hit to one line, so a list
/// of them stays scannable and its token cost stays predictable.
fn snippet_around(text: &str, start: usize, end: usize) -> String {
    let before = SNIPPET_CHARS / 2;
    let from = floor_char_boundary(text, start.saturating_sub(before));
    let to = ceil_char_boundary(text, (end + SNIPPET_CHARS / 2).min(text.len()));

    let mut slice = &text[from..to];
    if from > 0 {
        if let Some(space) = slice.find(char::is_whitespace) {
            slice = &slice[space + 1..];
        }
    }
    if to < text.len() {
        if let Some(space) = slice.rfind(char::is_whitespace) {
            slice = &slice[..space];
        }
    }

    let flattened = slice.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut out = String::new();
    if from > 0 {
        out.push('…');
    }
    out.push_str(&flattened);
    if to < text.len() {
        out.push('…');
    }
    out
}

fn floor_char_boundary(text: &str, mut index: usize) -> usize {
    index = index.min(text.len());
    while index > 0 && !text.is_char_boundary(index) {
        index -= 1;
    }
    index
}

fn ceil_char_boundary(text: &str, mut index: usize) -> usize {
    index = index.min(text.len());
    while index < text.len() && !text.is_char_boundary(index) {
        index += 1;
    }
    index
}

/// Render matches as the text a caller reads, inside `max_tokens`.
///
/// Whole hits are dropped from the tail when the budget cannot hold them all,
/// and the count of what was dropped survives even a budget too small for it —
/// a search that silently reports three of forty hits is worse than one that
/// says so, because the missing ones look like absence of evidence.
pub fn render(matches: &[Match], pattern: &str, max_tokens: Option<usize>) -> String {
    if matches.is_empty() {
        return format!("[no matches for /{pattern}/ in this page]");
    }

    let rows: Vec<String> = matches
        .iter()
        .map(|m| {
            let nearby = match m.nearby {
                0 => String::new(),
                n => format!(" (+{n} nearby)"),
            };
            match &m.section {
                Some(section) => {
                    format!(
                        "offset {} in \"{}\"{nearby} — {}",
                        m.offset, section, m.snippet
                    )
                }
                None => format!("offset {}{nearby} — {}", m.offset, m.snippet),
            }
        })
        .collect();

    let footer = "\nRead a match by fetching it at the offset shown.";
    let Some(max) = max_tokens else {
        return format!("{}{footer}", rows.join("\n"));
    };

    let (body, shown) = fit_rows(&rows, max, footer);
    if shown == rows.len() {
        return format!("{body}{footer}");
    }

    // Reserve room for the count before refitting, so the budget is not spent
    // on hits and then overrun by the line saying hits are missing.
    let note =
        |dropped: usize| format!("[{dropped} more match(es) not shown; raise the token budget]");
    let reserved = max.saturating_sub(estimate_tokens(&note(rows.len())));
    let (body, shown) = fit_rows(&rows, reserved, footer);
    let note = note(rows.len() - shown);

    if shown == 0 {
        return format!("{note}{footer}");
    }
    format!("{body}\n{note}{footer}")
}

/// Take rows while they fit, so the budget cuts whole hits rather than
/// truncating one into a half-offset nobody can use.
fn fit_rows(rows: &[String], max_tokens: usize, footer: &str) -> (String, usize) {
    let mut body = String::new();
    let mut shown = 0usize;
    for row in rows {
        let candidate = if body.is_empty() {
            row.clone()
        } else {
            format!("{body}\n{row}")
        };
        if estimate_tokens(&format!("{candidate}{footer}")) > max_tokens {
            break;
        }
        body = candidate;
        shown += 1;
    }
    (body, shown)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::convert::convert_parsed;
    use crate::outline;
    use crate::types::ContentType;
    use scraper::Html;

    const DOC: &str = "<article>\
        <h2>Installation</h2><p>Install it, then set the rate limit you want.</p>\
        <h2>Reference</h2><p>The rate limit defaults to sixty requests a minute.</p>\
        </article>";

    fn parse(html: &str) -> (String, Vec<Section>) {
        let doc = Html::parse_document(html);
        let text = convert_parsed(&doc, "https://example.com", ContentType::Text).content;
        let sections = outline::outline(&doc, &text);
        (text, sections)
    }

    #[test]
    fn a_hit_reports_where_to_read_it_and_which_section_it_is_in() {
        let (text, sections) = parse(DOC);
        let hits = grep(&text, &compile("rate limit").unwrap(), &sections);

        assert_eq!(hits.len(), 2, "{hits:?}");
        assert_eq!(hits[0].section.as_deref(), Some("Installation"));
        assert_eq!(hits[1].section.as_deref(), Some("Reference"));
        // The offset is the one a fetch reads, so it lands on the match itself.
        assert!(text[hits[1].offset..].starts_with("rate limit"));
    }

    /// Hits collapse by neighbourhood, so what a caller counts is occurrences.
    fn occurrences(hits: &[Match]) -> usize {
        hits.iter().map(|h| h.nearby + 1).sum()
    }

    #[test]
    fn a_lowercase_pattern_is_loose_and_an_uppercase_one_is_exact() {
        let (text, sections) = parse(DOC);
        // The heading "Installation" and the sentence "Install it" both match
        // either way; only an all-caps pattern, which nothing matches, differs.
        assert_eq!(
            occurrences(&grep(&text, &compile("install").unwrap(), &sections)),
            2
        );
        assert_eq!(
            occurrences(&grep(&text, &compile("Install").unwrap(), &sections)),
            2
        );
        assert_eq!(
            grep(&text, &compile("INSTALL").unwrap(), &sections).len(),
            0
        );
    }

    #[test]
    fn a_snippet_carries_context_without_splitting_words() {
        let (text, sections) = parse(DOC);
        let hits = grep(&text, &compile("sixty").unwrap(), &sections);

        let snippet = &hits[0].snippet;
        assert!(snippet.contains("sixty requests"), "{snippet}");
        // Elision marks a widened snippet, and neither end lands mid-word.
        let trimmed = snippet.trim_matches('…');
        assert!(
            !trimmed.starts_with(' ') && !trimmed.ends_with(' '),
            "{snippet}"
        );
    }

    /// A term repeated inside one snippet's worth of text is one location, and
    /// paying for four overlapping snippets to say so is the cost this avoids.
    #[test]
    fn occurrences_in_one_neighbourhood_collapse_into_one_hit() {
        let dense = format!(
            "<article><p>{}</p></article>",
            "The rate limit applies. ".repeat(4)
        );
        let (text, sections) = parse(&dense);
        let hits = grep(&text, &compile("rate limit").unwrap(), &sections);

        assert_eq!(hits.len(), 1, "{hits:?}");
        assert_eq!(hits[0].nearby, 3);
        assert!(render(&hits, "rate limit", None).contains("(+3 nearby)"));
    }

    /// A neighbourhood is only as wide as the snippet that shows it, so a long
    /// run becomes several hits rather than one claiming to cover it all — and
    /// no occurrence is lost in the process.
    #[test]
    fn a_run_longer_than_a_snippet_becomes_several_hits() {
        let dense = format!(
            "<article><p>{}</p></article>",
            "The rate limit applies. ".repeat(16)
        );
        let (text, sections) = parse(&dense);
        let hits = grep(&text, &compile("rate limit").unwrap(), &sections);

        assert!(hits.len() > 1, "{hits:?}");
        assert_eq!(occurrences(&hits), 16);
    }

    /// Collapsing must not swallow a genuinely separate mention further down.
    #[test]
    fn occurrences_far_apart_stay_separate_hits() {
        let filler = "Words of filler in between the two mentions. ".repeat(8);
        let (text, sections) = parse(&format!(
            "<article><p>The rate limit applies.</p><p>{filler}</p><p>The rate limit again.</p></article>"
        ));
        let hits = grep(&text, &compile("rate limit").unwrap(), &sections);

        assert_eq!(hits.len(), 2, "{hits:?}");
        assert_eq!(hits[0].nearby, 0);
        assert_eq!(occurrences(&hits), 2);
    }

    #[test]
    fn a_page_without_the_pattern_says_so() {
        let (text, sections) = parse(DOC);
        let hits = grep(&text, &compile("websocket").unwrap(), &sections);

        assert!(hits.is_empty());
        assert!(render(&hits, "websocket", None).contains("no matches"));
    }

    #[test]
    fn a_page_without_headings_still_reports_offsets() {
        let (text, sections) = parse("<article><p>The rate limit is fixed.</p></article>");
        let hits = grep(&text, &compile("rate limit").unwrap(), &sections);

        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].section, None);
        assert!(render(&hits, "rate limit", None).starts_with("offset 4 —"));
    }

    /// Reporting three of forty hits without saying so reads as absence of
    /// evidence, so the count outlives the budget.
    #[test]
    fn a_budget_drops_whole_hits_and_says_how_many() {
        let (text, sections) = parse(DOC);
        let hits = grep(&text, &compile("rate limit").unwrap(), &sections);
        let rendered = render(&hits, "rate limit", Some(40));

        assert!(estimate_tokens(&rendered) <= 40, "over budget: {rendered}");
        assert!(rendered.contains("more match(es) not shown"), "{rendered}");
    }

    #[test]
    fn an_invalid_pattern_is_an_error_rather_than_a_panic() {
        assert!(compile("(unclosed").is_err());
    }
}
