//! Plain-text conversion with **reference-style URL preservation**.
//!
//! Links are not stripped to their domain, nor expanded inline. Instead each
//! distinct URL is assigned a stable index and the anchor text is followed by
//! a compact `[N]` marker. The full URLs are collected into a reference list
//! that callers can append to the output or expose separately, so the agent
//! sees `[1]` inline (≈1 token) but can still recover the exact link.

use std::collections::HashMap;

use ego_tree::NodeRef;
use scraper::node::Node;
use scraper::{ElementRef, Html};
use url::Url;

use crate::extract;
use crate::types::UrlReference;

/// Collects and de-duplicates the links a walk encounters. Shared by the text
/// and markdown walkers so both resolve and filter hrefs by the same rules.
pub(crate) struct RefCollector {
    /// Maps a resolved URL to its assigned reference index (for de-duplication).
    seen: HashMap<String, usize>,
    pub(crate) references: Vec<UrlReference>,
    base: Option<Url>,
}

impl RefCollector {
    pub(crate) fn new(base_url: &str) -> Self {
        Self {
            seen: HashMap::new(),
            references: Vec::new(),
            base: Url::parse(base_url).ok(),
        }
    }

    /// Resolve a possibly-relative href against the page's base URL.
    ///
    /// In-page anchors carry no destination, and `javascript:` / `mailto:` are
    /// not fetchable: none of them earn a reference slot.
    pub(crate) fn resolve(&self, href: &str) -> Option<String> {
        let href = href.trim();
        if href.is_empty() || href.starts_with('#') {
            return None;
        }
        let scheme = href.split(':').next().unwrap_or("").to_ascii_lowercase();
        if matches!(scheme.as_str(), "javascript" | "mailto" | "data" | "tel") {
            return None;
        }
        match &self.base {
            Some(base) => base.join(href).ok().map(|u| u.to_string()),
            None => Url::parse(href).ok().map(|u| u.to_string()),
        }
    }

    /// Return the reference index for a URL, assigning a new one if unseen.
    pub(crate) fn index_for(&mut self, url: String, text: &str) -> usize {
        if let Some(idx) = self.seen.get(&url) {
            return *idx;
        }
        let idx = self.references.len() + 1;
        self.seen.insert(url.clone(), idx);
        self.references.push(UrlReference {
            index: idx,
            url,
            text: text.trim().to_string(),
        });
        idx
    }
}

fn is_block(name: &str) -> bool {
    matches!(
        name,
        "p" | "div"
            | "section"
            | "article"
            | "header"
            | "footer"
            | "h1"
            | "h2"
            | "h3"
            | "h4"
            | "h5"
            | "h6"
            | "li"
            | "ul"
            | "ol"
            | "table"
            | "tr"
            | "blockquote"
            | "pre"
            | "figure"
            | "aside"
            | "nav"
            | "main"
    )
}

/// Separator written between cells of the same table row.
///
/// Without it adjacent cells ran together — `<th>Name</th><th>Type</th>` came
/// out as `NameType` — which mangles exactly the reference tables this tool is
/// most often pointed at. A newline per cell would be unambiguous but costs a
/// line each; a pipe keeps the row on one line and reads like a table.
const CELL_SEPARATOR: &str = " | ";

fn walk(node: NodeRef<Node>, out: &mut String, refs: &mut RefCollector) {
    match node.value() {
        Node::Text(t) => out.push_str(&t[..]),
        Node::Element(el) => {
            let name = el.name();
            if super::is_skippable(name) {
                return;
            }

            if name == "br" {
                out.push('\n');
                return;
            }

            if name == "a" {
                // Collect the anchor's inner text first.
                let mut inner = String::new();
                for child in node.children() {
                    walk(child, &mut inner, refs);
                }
                let inner = inner.trim().to_string();
                out.push_str(&inner);
                if let Some(href) = el.attr("href") {
                    if let Some(resolved) = refs.resolve(href) {
                        let idx = refs.index_for(resolved, &inner);
                        out.push_str(&format!(" [{}]", idx));
                    }
                }
                return;
            }

            if matches!(name, "td" | "th") {
                // The row opened a fresh line, so the first cell needs no
                // separator; every later cell in the row does.
                if !out.is_empty() && !out.ends_with('\n') {
                    out.push_str(CELL_SEPARATOR);
                }
                for child in node.children() {
                    walk(child, out, refs);
                }
                return;
            }

            let block = is_block(name);
            if block && !out.ends_with('\n') && !out.is_empty() {
                out.push('\n');
            }
            for child in node.children() {
                walk(child, out, refs);
            }
            if block && !out.ends_with('\n') {
                out.push('\n');
            }
        }
        _ => {}
    }
}

/// Convert a parsed HTML document to reference-style plain text.
///
/// Returns the body text (with inline `[N]` markers) and the ordered list of
/// references. The returned text does **not** include the rendered
/// "References:" block — see [`render_references`] to append it.
pub fn text_with_refs(doc: &Html, base_url: &str) -> (String, Vec<UrlReference>) {
    let root: ElementRef = match extract::content_root(doc) {
        Some(el) => el,
        None => return (String::new(), Vec::new()),
    };

    let mut refs = RefCollector::new(base_url);
    let mut out = String::new();
    for child in root.children() {
        walk(child, &mut out, &mut refs);
    }
    (out, refs.references)
}

/// [`text_with_refs`] for callers holding raw HTML. Parses the document; prefer
/// the parsed form when the caller already has one.
pub fn html_to_text_with_refs(html: &str, base_url: &str) -> (String, Vec<UrlReference>) {
    text_with_refs(&Html::parse_document(html), base_url)
}

/// Render a reference list into the canonical block appended to text output.
/// Thin wrapper over [`crate::refs::render_block`].
pub fn render_references(references: &[UrlReference]) -> String {
    crate::refs::render_block(references)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_cells_are_separated() {
        let html = "<article><table>\
                    <tr><th>Name</th><th>Type</th></tr>\
                    <tr><td>alpha</td><td>string</td></tr>\
                    </table></article>";
        let (text, _) = html_to_text_with_refs(html, "https://x.test/");
        assert!(text.contains("Name | Type"), "text: {text:?}");
        assert!(text.contains("alpha | string"), "text: {text:?}");
    }

    #[test]
    fn unfetchable_schemes_get_no_reference() {
        let html = r##"<article><p>
            <a href="javascript:alert(1)">js</a>
            <a href="mailto:a@b.c">mail</a>
            <a href="#top">anchor</a>
            <a href="/ok">ok</a></p></article>"##;
        let (text, refs) = html_to_text_with_refs(html, "https://x.test/");
        assert_eq!(refs.len(), 1, "refs: {refs:?}");
        assert_eq!(refs[0].url, "https://x.test/ok");
        assert!(text.contains("ok [1]"));
    }
}
