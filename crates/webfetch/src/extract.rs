use std::collections::HashMap;

use ego_tree::{NodeId, NodeRef};
use once_cell::sync::Lazy;
use scraper::node::Node;
use scraper::{ElementRef, Html, Selector};

use crate::types::Metadata;

/// Selectors are compiled once per process rather than on every call — this
/// runs on the hot conversion path, several times per page.
fn selector(sel: &str) -> Selector {
    Selector::parse(sel).expect("static selector")
}

static CONTENT_ROOTS: Lazy<[Selector; 3]> = Lazy::new(|| {
    [
        selector("article"),
        selector("main"),
        selector("[role=main]"),
    ]
});
static DIV: Lazy<Selector> = Lazy::new(|| selector("div"));
static BODY: Lazy<Selector> = Lazy::new(|| selector("body"));
static TITLE: Lazy<[Selector; 2]> = Lazy::new(|| [selector("title"), selector("h1")]);
static HTML_EL: Lazy<Selector> = Lazy::new(|| selector("html"));

static META_DESCRIPTION: Lazy<[Selector; 2]> = Lazy::new(|| {
    [
        selector("meta[name=description]"),
        selector("meta[property='og:description']"),
    ]
});
static META_AUTHOR: Lazy<[Selector; 2]> = Lazy::new(|| {
    [
        selector("meta[name=author]"),
        selector("meta[property='article:author']"),
    ]
});
static META_PUBLISHED: Lazy<[Selector; 2]> = Lazy::new(|| {
    [
        selector("meta[property='article:published_time']"),
        selector("meta[name='date']"),
    ]
});
static META_SITE_NAME: Lazy<[Selector; 1]> =
    Lazy::new(|| [selector("meta[property='og:site_name']")]);

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
    for selector in CONTENT_ROOTS.iter() {
        if let Some(el) = doc.select(selector).next() {
            return Some(el);
        }
    }

    // Fall back to the largest text-bearing <div>, using one bottom-up pass to
    // compute every node's subtree text length up front.
    let mut lengths: HashMap<NodeId, usize> = HashMap::new();
    subtree_text_lengths(doc.tree.root(), &mut lengths);

    let mut best: Option<(usize, ElementRef)> = None;
    for el in doc.select(&DIV) {
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

    doc.select(&BODY).next()
}

/// Extract the page title from `<title>` or the first `<h1>`.
pub fn extract_title(doc: &Html) -> String {
    for selector in TITLE.iter() {
        if let Some(el) = doc.select(selector).next() {
            let t = el.text().collect::<String>().trim().to_string();
            if !t.is_empty() {
                return t;
            }
        }
    }
    String::new()
}

/// Read the `content` attribute of the first matching `<meta>` selector.
fn meta(doc: &Html, selectors: &[Selector]) -> Option<String> {
    for selector in selectors {
        if let Some(el) = doc.select(selector).next() {
            if let Some(c) = el.value().attr("content") {
                let c = c.trim();
                if !c.is_empty() {
                    return Some(c.to_string());
                }
            }
        }
    }
    None
}

/// Extract citation-oriented metadata: description, author, publish date,
/// language, and site name (from standard `<meta>`/OpenGraph tags).
pub fn extract_metadata(doc: &Html) -> Metadata {
    let lang = doc
        .select(&HTML_EL)
        .next()
        .and_then(|el| el.value().attr("lang"))
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    Metadata {
        description: meta(doc, &*META_DESCRIPTION),
        author: meta(doc, &*META_AUTHOR),
        published: meta(doc, &*META_PUBLISHED),
        site_name: meta(doc, &*META_SITE_NAME),
        lang,
        charset: None,
    }
}
