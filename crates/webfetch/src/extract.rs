use std::collections::HashMap;

use ego_tree::{NodeId, NodeRef};
use scraper::node::Node;
use scraper::{ElementRef, Html, Selector};

use crate::types::Metadata;

/// Sum the trimmed length of every descendant text node, for every node in the
/// tree, in a single bottom-up pass.
///
/// The previous "largest `<div>`" heuristic called `el.text()` (a full subtree
/// walk) once per `<div>`; on nested DOMs the same text was re-summed at every
/// ancestor, making it ~O(n²). Computing each node's subtree text length once
/// and reading it back from the map keeps the identical "largest text-bearing
/// container" semantics in O(n).
fn subtree_text_lengths(root: NodeRef<Node>, out: &mut HashMap<NodeId, usize>) -> usize {
    let mut total = match root.value() {
        Node::Text(t) => t.trim().len(),
        _ => 0,
    };
    for child in root.children() {
        total += subtree_text_lengths(child, out);
    }
    out.insert(root.id(), total);
    total
}

/// Pick the element most likely to contain the primary article content.
///
/// Heuristic, in priority order: `<article>`, `<main>`, `[role=main]`,
/// then the largest `<div>` by text length, falling back to `<body>`.
pub fn content_root(doc: &Html) -> Option<ElementRef<'_>> {
    for sel in ["article", "main", "[role=main]"] {
        if let Ok(selector) = Selector::parse(sel) {
            if let Some(el) = doc.select(&selector).next() {
                return Some(el);
            }
        }
    }

    // Fall back to the largest text-bearing <div>, using one bottom-up pass to
    // compute every node's subtree text length up front.
    if let Ok(div_sel) = Selector::parse("div") {
        let mut lengths: HashMap<NodeId, usize> = HashMap::new();
        subtree_text_lengths(doc.tree.root(), &mut lengths);

        let mut best: Option<(usize, ElementRef)> = None;
        for el in doc.select(&div_sel) {
            let len = lengths.get(&el.id()).copied().unwrap_or(0);
            if best.as_ref().is_none_or(|(b, _)| len > *b) {
                best = Some((len, el));
            }
        }
        if let Some((len, el)) = best {
            if len > 0 {
                return Some(el);
            }
        }
    }

    Selector::parse("body")
        .ok()
        .and_then(|sel| doc.select(&sel).next())
}

/// Extract the page title from `<title>` or the first `<h1>`.
pub fn extract_title(doc: &Html) -> String {
    for sel in ["title", "h1"] {
        if let Ok(selector) = Selector::parse(sel) {
            if let Some(el) = doc.select(&selector).next() {
                let t = el.text().collect::<String>().trim().to_string();
                if !t.is_empty() {
                    return t;
                }
            }
        }
    }
    String::new()
}

/// Read the `content` attribute of the first matching `<meta>` selector.
fn meta(doc: &Html, selectors: &[&str]) -> Option<String> {
    for sel in selectors {
        if let Ok(selector) = Selector::parse(sel) {
            if let Some(el) = doc.select(&selector).next() {
                if let Some(c) = el.value().attr("content") {
                    let c = c.trim();
                    if !c.is_empty() {
                        return Some(c.to_string());
                    }
                }
            }
        }
    }
    None
}

/// Extract citation-oriented metadata: description, author, publish date,
/// language, and site name (from standard `<meta>`/OpenGraph tags).
pub fn extract_metadata(doc: &Html) -> Metadata {
    let lang = Selector::parse("html")
        .ok()
        .and_then(|sel| doc.select(&sel).next())
        .and_then(|el| el.value().attr("lang"))
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    Metadata {
        description: meta(
            doc,
            &["meta[name=description]", "meta[property='og:description']"],
        ),
        author: meta(
            doc,
            &["meta[name=author]", "meta[property='article:author']"],
        ),
        published: meta(
            doc,
            &[
                "meta[property='article:published_time']",
                "meta[name='date']",
            ],
        ),
        site_name: meta(doc, &["meta[property='og:site_name']"]),
        lang,
    }
}
