//! Structured conversion: emit the page as an ordered list of typed blocks,
//! serialized to JSON. Links are preserved as reference indices (same scheme
//! as the text path), so structured output is both machine-parseable and
//! token-frugal inline.

use serde::{Deserialize, Serialize};

use super::text::html_to_text_with_refs;
use crate::compress::compress_text;
use crate::types::UrlReference;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StructuredDoc {
    pub blocks: Vec<Block>,
    pub references: Vec<UrlReference>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Block {
    pub kind: BlockKind,
    pub text: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BlockKind {
    Paragraph,
}

/// Build a structured document. Each non-empty line of the reference-style
/// text becomes a paragraph block; references are carried alongside.
pub fn html_to_structured(html: &str, base_url: &str) -> StructuredDoc {
    let (text, references) = html_to_text_with_refs(html, base_url);
    let blocks = text
        .lines()
        .map(compress_text)
        .filter(|l| !l.is_empty())
        .map(|text| Block {
            kind: BlockKind::Paragraph,
            text,
        })
        .collect();
    StructuredDoc { blocks, references }
}

pub fn to_json(doc: &StructuredDoc) -> String {
    serde_json::to_string_pretty(doc).unwrap_or_else(|_| "{}".to_string())
}
