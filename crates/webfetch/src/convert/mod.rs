//! Output dispatcher: routes an HTML document to the requested format.

pub mod markdown;
pub mod structured;
pub mod text;

use crate::compress::{compress_block, compress_text};
use crate::types::{ContentType, UrlReference};

/// Elements whose contents never belong in extracted output (scripts,
/// styling, embedded documents). Shared by every walker so the formats
/// agree on what to drop.
pub(crate) fn is_skippable(name: &str) -> bool {
    matches!(
        name,
        "script" | "style" | "noscript" | "svg" | "head" | "template" | "iframe"
    )
}

/// A converted document: the rendered `content` plus any preserved references.
pub struct Converted {
    pub content: String,
    pub references: Vec<UrlReference>,
}

/// Convert HTML to the requested content type.
///
/// For [`ContentType::Text`], the reference list is rendered into a trailing
/// `References:` block appended to the content (and also returned separately).
pub fn convert(html: &str, base_url: &str, content_type: ContentType) -> Converted {
    match content_type {
        ContentType::Text => {
            let (body, references) = text::html_to_text_with_refs(html, base_url);
            let body = compress_block(&body);
            let refs_block = text::render_references(&references);
            let content = if refs_block.is_empty() {
                body
            } else {
                format!("{}\n\n{}", body, refs_block)
            };
            Converted {
                content,
                references,
            }
        }
        ContentType::Markdown => {
            let md = markdown::html_to_markdown(html, base_url);
            Converted {
                content: compress_block(&md),
                references: Vec::new(),
            }
        }
        ContentType::Structured => {
            let doc = structured::html_to_structured(html, base_url);
            let _ = compress_text; // available for callers extending block kinds
            Converted {
                content: structured::to_json(&doc),
                references: doc.references,
            }
        }
    }
}
