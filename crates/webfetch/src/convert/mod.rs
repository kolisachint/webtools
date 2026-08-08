//! Output dispatcher: routes an HTML document to the requested format.

pub mod markdown;
pub mod structured;
pub mod text;

use scraper::Html;

use crate::compress::compress_block;
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

/// A converted document: the rendered `content` plus the references it cites.
///
/// `content` never carries the trailing `References:` block — assembling that
/// is [`crate::refs::fit_to_budget`]'s job, because whether a reference survives
/// depends on whether the (possibly truncated) body still cites it.
pub struct Converted {
    pub content: String,
    pub references: Vec<UrlReference>,
}

/// Convert a parsed document to the requested content type.
pub fn convert_parsed(doc: &Html, base_url: &str, content_type: ContentType) -> Converted {
    match content_type {
        ContentType::Text => {
            let (body, references) = text::text_with_refs(doc, base_url);
            Converted {
                content: compress_block(&body),
                references,
            }
        }
        ContentType::Markdown => {
            let (md, references) = markdown::markdown_with_refs(doc, base_url);
            Converted {
                content: compress_block(&md),
                references,
            }
        }
        ContentType::Structured => {
            let parsed = structured::structured(doc, base_url);
            Converted {
                content: structured::to_json(&parsed),
                references: parsed.references,
            }
        }
    }
}

/// [`convert_parsed`] for callers holding raw HTML.
///
/// Prefer the parsed form: the full pipeline used to parse the same document
/// twice — once for the title and metadata, once here — which was roughly a
/// third of its total cost.
pub fn convert(html: &str, base_url: &str, content_type: ContentType) -> Converted {
    convert_parsed(&Html::parse_document(html), base_url, content_type)
}
