//! Structured conversion: emit the page as an ordered list of typed blocks,
//! serialized to JSON. Links are preserved as reference indices (same scheme
//! as the text path), so structured output is both machine-parseable and
//! token-frugal inline.
//!
//! Blocks carry the kind they came from — heading (with its level), list item,
//! code, quote, table row, paragraph — so a consumer can reconstruct document
//! shape. Reading it back off flat text could not tell a heading from a
//! sentence, which made "structured" no more structured than `text`.

use ego_tree::NodeRef;
use scraper::node::Node;
use scraper::{ElementRef, Html};
use serde::{Deserialize, Serialize};

use super::text::RefCollector;
use crate::compress::compress_text;
use crate::extract;
use crate::types::UrlReference;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StructuredDoc {
    pub blocks: Vec<Block>,
    pub references: Vec<UrlReference>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Block {
    pub kind: BlockKind,
    /// Heading depth (1-6). Only present on [`BlockKind::Heading`].
    #[serde(skip_serializing_if = "Option::is_none")]
    pub level: Option<u8>,
    pub text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BlockKind {
    Heading,
    Paragraph,
    ListItem,
    Code,
    Quote,
    TableRow,
}

/// Elements that open a block, and the kind they produce.
fn block_kind(name: &str) -> Option<(BlockKind, Option<u8>)> {
    let heading = |n: u8| Some((BlockKind::Heading, Some(n)));
    match name {
        "h1" => heading(1),
        "h2" => heading(2),
        "h3" => heading(3),
        "h4" => heading(4),
        "h5" => heading(5),
        "h6" => heading(6),
        "li" => Some((BlockKind::ListItem, None)),
        "pre" => Some((BlockKind::Code, None)),
        "blockquote" => Some((BlockKind::Quote, None)),
        "tr" => Some((BlockKind::TableRow, None)),
        "p" => Some((BlockKind::Paragraph, None)),
        _ => None,
    }
}

/// Walk the tree, emitting one block per block-level element.
///
/// Text that is not inside any block-level element still has to go somewhere;
/// it accumulates in `loose` and is flushed as a paragraph when a block starts
/// or the walk ends, so nothing is silently dropped.
fn walk(node: NodeRef<Node>, blocks: &mut Vec<Block>, loose: &mut String, refs: &mut RefCollector) {
    match node.value() {
        Node::Text(t) => loose.push_str(&t[..]),
        Node::Element(el) => {
            let name = el.name();
            if super::is_skippable(name) {
                return;
            }

            if name == "a" {
                let inner = ElementRef::wrap(node)
                    .map(|e| e.text().collect::<String>())
                    .unwrap_or_default();
                let inner = compress_text(&inner);
                loose.push_str(&inner);
                if let Some(url) = el.attr("href").and_then(|h| refs.resolve(h)) {
                    let idx = refs.index_for(url, &inner);
                    loose.push_str(&format!(" [{idx}]"));
                }
                return;
            }

            match block_kind(name) {
                Some((kind, level)) => {
                    flush(blocks, loose);
                    let mut inner = String::new();
                    for child in node.children() {
                        walk(child, blocks, &mut inner, refs);
                    }
                    let text = compress_text(&inner);
                    if !text.is_empty() {
                        blocks.push(Block { kind, level, text });
                    }
                }
                None => {
                    if matches!(name, "br" | "td" | "th") {
                        loose.push(' ');
                    }
                    for child in node.children() {
                        walk(child, blocks, loose, refs);
                    }
                }
            }
        }
        _ => {}
    }
}

/// Emit whatever loose text has accumulated as a paragraph block.
fn flush(blocks: &mut Vec<Block>, loose: &mut String) {
    let text = compress_text(loose);
    loose.clear();
    if !text.is_empty() {
        blocks.push(Block {
            kind: BlockKind::Paragraph,
            level: None,
            text,
        });
    }
}

/// Build a structured document from a parsed page.
pub fn structured(doc: &Html, base_url: &str) -> StructuredDoc {
    let root = match extract::content_root(doc) {
        Some(el) => el,
        None => {
            return StructuredDoc {
                blocks: Vec::new(),
                references: Vec::new(),
            }
        }
    };

    let mut refs = RefCollector::new(base_url);
    let mut blocks = Vec::new();
    let mut loose = String::new();
    for child in root.children() {
        walk(child, &mut blocks, &mut loose, &mut refs);
    }
    flush(&mut blocks, &mut loose);

    StructuredDoc {
        blocks,
        references: refs.references,
    }
}

/// [`structured`] for callers holding raw HTML.
pub fn html_to_structured(html: &str, base_url: &str) -> StructuredDoc {
    structured(&Html::parse_document(html), base_url)
}

pub fn to_json(doc: &StructuredDoc) -> String {
    serde_json::to_string_pretty(doc).unwrap_or_else(|_| "{}".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocks_carry_their_kind() {
        let html = "<article><h2>Setup</h2><p>Install it.</p>\
                    <ul><li>first</li><li>second</li></ul>\
                    <pre>cargo build</pre><blockquote>note</blockquote>\
                    <table><tr><td>a</td><td>b</td></tr></table></article>";
        let doc = html_to_structured(html, "https://x.test/");
        let kinds: Vec<_> = doc.blocks.iter().map(|b| b.kind).collect();
        assert_eq!(
            kinds,
            vec![
                BlockKind::Heading,
                BlockKind::Paragraph,
                BlockKind::ListItem,
                BlockKind::ListItem,
                BlockKind::Code,
                BlockKind::Quote,
                BlockKind::TableRow,
            ],
            "blocks: {:?}",
            doc.blocks
        );
        assert_eq!(doc.blocks[0].level, Some(2));
        assert_eq!(doc.blocks[0].text, "Setup");
        assert_eq!(doc.blocks[6].text, "a b");
    }

    #[test]
    fn links_become_reference_markers() {
        let html = r#"<article><p>See the <a href="/guide">guide</a>.</p></article>"#;
        let doc = html_to_structured(html, "https://x.test/");
        assert_eq!(doc.references.len(), 1);
        assert_eq!(doc.references[0].url, "https://x.test/guide");
        assert!(doc.blocks[0].text.contains("[1]"), "{:?}", doc.blocks);
    }

    #[test]
    fn text_outside_any_block_is_still_captured() {
        let html = "<article>bare text with no wrapper</article>";
        let doc = html_to_structured(html, "https://x.test/");
        assert_eq!(doc.blocks.len(), 1);
        assert_eq!(doc.blocks[0].text, "bare text with no wrapper");
    }
}
