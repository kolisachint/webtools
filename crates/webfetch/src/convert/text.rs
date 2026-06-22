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

struct RefCollector {
    /// Maps a resolved URL to its assigned reference index (for de-duplication).
    seen: HashMap<String, usize>,
    references: Vec<UrlReference>,
    base: Option<Url>,
}

impl RefCollector {
    fn new(base_url: &str) -> Self {
        Self {
            seen: HashMap::new(),
            references: Vec::new(),
            base: Url::parse(base_url).ok(),
        }
    }

    /// Resolve a possibly-relative href against the page's base URL.
    fn resolve(&self, href: &str) -> Option<String> {
        let href = href.trim();
        if href.is_empty() || href.starts_with('#') {
            return None;
        }
        if href.starts_with("javascript:") || href.starts_with("mailto:") {
            return None;
        }
        match &self.base {
            Some(base) => base.join(href).ok().map(|u| u.to_string()),
            None => Url::parse(href).ok().map(|u| u.to_string()),
        }
    }

    /// Return the reference index for a URL, assigning a new one if unseen.
    fn index_for(&mut self, url: String, text: &str) -> usize {
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

/// Convert an HTML document to reference-style plain text.
///
/// Returns the body text (with inline `[N]` markers) and the ordered list of
/// references. The returned text does **not** include the rendered
/// "References:" block — see [`render_references`] to append it.
pub fn html_to_text_with_refs(html: &str, base_url: &str) -> (String, Vec<UrlReference>) {
    let doc = Html::parse_document(html);
    let root: ElementRef = match extract::content_root(&doc) {
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

/// Render a reference list into the canonical block appended to text output.
/// Thin wrapper over [`crate::refs::render_block`].
pub fn render_references(references: &[UrlReference]) -> String {
    crate::refs::render_block(references)
}
