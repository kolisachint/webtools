//! A map of a long page: its headings, where each section starts, and what
//! each one costs to read.
//!
//! Paging makes a long document readable in sequence, which is the wrong shape
//! for the common case — the answer is in one section and the rest is
//! overhead. An outline is the cheap map that turns a sequential read into a
//! targeted one: a few hundred tokens naming the sections, each with the offset
//! that reads it.
//!
//! Offsets are into the extracted text, the same space `FetchOptions::offset`
//! addresses, so a section is read by feeding its offset straight back — no
//! second addressing scheme, and no way for the two to disagree.

use scraper::{Html, Selector};
use serde::{Deserialize, Serialize};

use crate::compress::{compress_text, estimate_tokens};
use crate::extract;

/// One heading and the span of text it opens.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Section {
    /// Heading depth, 1-6.
    pub level: u8,
    pub title: String,
    /// Byte offset in the extracted text. Pass it as `offset` to read here.
    pub offset: usize,
    /// Size of the section: from this heading to the next, or to the end.
    pub bytes: usize,
    /// What reading this section costs, so a caller can choose before paying.
    pub token_estimate: usize,
}

/// Build the outline of a parsed document against its extracted text.
///
/// Headings are located *in the extracted text* rather than recorded during
/// conversion, because conversion is not the last step: whitespace compression
/// and duplicate-title stripping both run afterwards and would shift any offset
/// captured mid-walk. Searching the finished text is what keeps an outline
/// offset and a paging offset the same number.
///
/// A heading whose text cannot be found — dropped as a duplicate title, or
/// rewritten past recognition — is skipped rather than guessed at: a wrong
/// offset reads as a section boundary in the wrong place, which is worse than
/// an absent row.
pub fn outline(doc: &Html, body_text: &str) -> Vec<Section> {
    let Some(root) = extract::content_root(doc) else {
        return Vec::new();
    };
    let Ok(selector) = Selector::parse("h1, h2, h3, h4, h5, h6") else {
        return Vec::new();
    };

    let mut sections: Vec<Section> = Vec::new();
    // Headings are searched for in document order from where the last one was
    // found, so a title that recurs ("Parameters" under every endpoint) lands on
    // its own occurrence instead of the first one every time.
    let mut cursor = 0usize;

    for element in root.select(&selector) {
        let level = element
            .value()
            .name()
            .strip_prefix('h')
            .and_then(|n| n.parse::<u8>().ok())
            .unwrap_or(1);
        let title = compress_text(&element.text().collect::<String>());
        if title.is_empty() {
            continue;
        }
        let Some(found) = body_text.get(cursor..).and_then(|rest| rest.find(&title)) else {
            continue;
        };
        let offset = cursor + found;
        cursor = offset + title.len();
        sections.push(Section {
            level,
            title,
            offset,
            bytes: 0,
            token_estimate: 0,
        });
    }

    // A section runs to the next heading, so its extent is only known once the
    // next one is placed.
    for i in 0..sections.len() {
        let start = sections[i].offset;
        let end = sections
            .get(i + 1)
            .map(|next| next.offset)
            .unwrap_or(body_text.len());
        sections[i].bytes = end.saturating_sub(start);
        sections[i].token_estimate = body_text
            .get(start..end)
            .map(estimate_tokens)
            .unwrap_or_default();
    }

    sections
}

/// Render an outline as the text a caller reads, inside `max_tokens`.
///
/// Indented by heading depth, with the offset that reads each section. Rows are
/// dropped from the tail when the budget cannot hold them all — an outline that
/// silently overran its cap would be the same failure paging exists to fix.
pub fn render(sections: &[Section], max_tokens: Option<usize>) -> String {
    if sections.is_empty() {
        return "[no headings: this document has no outline to show]".to_string();
    }

    let rows: Vec<String> = sections
        .iter()
        .map(|s| {
            let indent = "  ".repeat(usize::from(s.level).saturating_sub(1));
            format!(
                "{indent}{} — offset {}, ~{} tokens",
                s.title, s.offset, s.token_estimate
            )
        })
        .collect();

    let footer = "\nRead a section by fetching it at the offset shown.";
    let Some(max) = max_tokens else {
        return format!("{}{footer}", rows.join("\n"));
    };

    // Two passes: fit against the whole budget first, and only if rows have to
    // be dropped, refit against a budget that reserves room for saying so. One
    // pass would spend the budget on rows and then overrun on the note — which
    // is the note that must not be cut.
    let (body, shown) = fit_rows(&rows, max, footer);
    if shown == rows.len() {
        return format!("{body}{footer}");
    }

    let note = format!(
        "[{} more heading(s) not shown; raise the token budget]",
        rows.len() - shown
    );
    let reserved = max.saturating_sub(estimate_tokens(&note));
    let (body, shown) = fit_rows(&rows, reserved, footer);
    // Recomputed against the refitted count, so the number is the truth after
    // the reserve, not before it.
    let note = format!(
        "[{} more heading(s) not shown; raise the token budget]",
        rows.len() - shown
    );

    // The count of what was dropped is the one line that must survive: an
    // outline that quietly omits half a document is worse than one that admits
    // it, so a budget too small even for the note still gets the note.
    if shown == 0 {
        return format!("{note}{footer}");
    }
    format!("{body}\n{note}{footer}")
}

/// Take rows while they fit, so the budget cuts whole rows rather than
/// truncating one mid-offset into something unusable.
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
    use crate::types::ContentType;

    const DOC: &str = "<article>\
        <h2>Getting started</h2><p>First you install it and then you run it.</p>\
        <h2>Reference</h2><p>Every flag, described at length for token cost.</p>\
        <h3>Flags</h3><p>The flags themselves, one after another.</p>\
        </article>";

    fn parse(html: &str) -> (Html, String) {
        let doc = Html::parse_document(html);
        let text = convert_parsed(&doc, "https://example.com", ContentType::Text).content;
        (doc, text)
    }

    #[test]
    fn every_heading_becomes_a_section_at_its_own_offset() {
        let (doc, text) = parse(DOC);
        let sections = outline(&doc, &text);

        let titles: Vec<&str> = sections.iter().map(|s| s.title.as_str()).collect();
        assert_eq!(titles, vec!["Getting started", "Reference", "Flags"]);
        assert_eq!(sections[0].level, 2);
        assert_eq!(sections[2].level, 3);
        // Offsets are ascending and each one lands on its own heading.
        for section in &sections {
            assert!(text[section.offset..].starts_with(&section.title));
        }
        assert!(sections[0].offset < sections[1].offset);
    }

    /// The offsets have to be the ones paging uses, or the outline points at
    /// text the reader never sees.
    #[test]
    fn a_section_offset_reads_that_section() {
        let (doc, text) = parse(DOC);
        let sections = outline(&doc, &text);
        let reference = &sections[1];

        let window = &text[reference.offset..reference.offset + reference.bytes];
        assert!(window.starts_with("Reference"));
        assert!(window.contains("Every flag"));
        assert!(!window.contains("First you install"));
    }

    #[test]
    fn the_last_section_runs_to_the_end() {
        let (doc, text) = parse(DOC);
        let sections = outline(&doc, &text);
        let last = sections.last().expect("a section");

        assert_eq!(last.offset + last.bytes, text.len());
    }

    /// A title that recurs must resolve to its own occurrence, not the first.
    #[test]
    fn a_repeated_title_lands_on_each_occurrence() {
        let (doc, text) = parse(
            "<article><h2>Parameters</h2><p>For the first endpoint here.</p>\
             <h2>Parameters</h2><p>For the second endpoint here.</p></article>",
        );
        let sections = outline(&doc, &text);

        assert_eq!(sections.len(), 2);
        assert_ne!(sections[0].offset, sections[1].offset);
        assert!(text[sections[1].offset..].starts_with("Parameters"));
    }

    #[test]
    fn a_document_without_headings_has_no_outline() {
        let (doc, text) = parse("<article><p>Just prose, no headings at all.</p></article>");
        assert!(outline(&doc, &text).is_empty());
        assert!(render(&[], None).contains("no headings"));
    }

    #[test]
    fn rendering_names_the_offset_that_reads_each_section() {
        let (doc, text) = parse(DOC);
        let rendered = render(&outline(&doc, &text), None);

        assert!(rendered.contains("Getting started — offset 0"));
        assert!(rendered.contains("at the offset shown"));
    }

    /// An outline that overran its own budget would be the failure paging
    /// exists to prevent — and one that quietly dropped half a document would
    /// be worse, so the count of what is missing comes with it.
    #[test]
    fn a_budget_drops_whole_rows_and_says_how_many() {
        let (doc, text) = parse(DOC);
        let sections = outline(&doc, &text);
        let rendered = render(&sections, Some(40));

        assert!(estimate_tokens(&rendered) <= 40, "over budget: {rendered}");
        assert!(
            rendered.contains("Getting started"),
            "no rows kept: {rendered}"
        );
        assert!(rendered.contains("more heading(s) not shown"), "{rendered}");
    }

    /// A budget too small for even one row still has to say what is there,
    /// rather than answering with an empty-looking outline.
    #[test]
    fn a_budget_too_small_for_any_row_still_reports_the_count() {
        let (doc, text) = parse(DOC);
        let rendered = render(&outline(&doc, &text), Some(1));

        assert!(
            rendered.contains("3 more heading(s) not shown"),
            "{rendered}"
        );
    }
}
