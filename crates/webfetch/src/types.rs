use serde::{Deserialize, Serialize};

pub use crate::tls::TlsConfig;

/// Whether a fetch actually produced content.
///
/// An extraction that yields nothing used to be reported exactly like a
/// successful one — empty `content`, exit 0 — so a caller could not tell a
/// blank page from a page whose text never arrives without a browser. Agents
/// read that as "this page has nothing to say" and moved on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContentStatus {
    /// Content was extracted.
    Ok,
    /// The document parsed but genuinely holds no text.
    Empty,
    /// An HTML shell with scripts and no text: the content is rendered by
    /// JavaScript, which this fetcher does not run.
    NeedsJs,
    /// The document was too deeply nested to parse within a sane time budget
    /// and was refused before parsing. See `webfetch::limits`.
    TooComplex,
}

impl ContentStatus {
    /// Did extraction fail to produce usable content?
    pub fn is_failure(self) -> bool {
        !matches!(self, ContentStatus::Ok)
    }

    /// A one-line explanation, or `None` when content came back normally.
    pub fn note(self) -> Option<&'static str> {
        match self {
            ContentStatus::Ok => None,
            ContentStatus::Empty => Some("the page parsed but contains no text"),
            ContentStatus::NeedsJs => Some(
                "no text content: the page renders its body with JavaScript, \
                 which webtools does not execute",
            ),
            ContentStatus::TooComplex => {
                Some("the document is too deeply nested to parse safely and was refused")
            }
        }
    }
}

/// Result of fetching and converting a web page.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FetchResult {
    pub title: String,
    /// The URL the content actually came from, after redirects.
    pub final_url: String,
    pub content: String,
    pub content_type: ContentType,
    /// The detected source media kind: "html", "json", "text", or a raw
    /// content-type for anything not rendered.
    pub media: String,
    pub token_estimate: usize,
    /// Whether content was extracted — see [`ContentStatus`].
    pub status: ContentStatus,
    /// References cited by `content`. When `max_tokens` truncates the body,
    /// references the surviving text no longer cites are dropped from both, so
    /// this list and the inline `[N]` markers always agree.
    pub references: Vec<UrlReference>,
    #[serde(default)]
    pub metadata: Metadata,
    /// The URL that was requested, before any redirect.
    pub source: String,
}

/// Citation-oriented page metadata, all best-effort.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Metadata {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub published: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lang: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub site_name: Option<String>,
    /// The document's declared character set, when it is not UTF-8. Bodies are
    /// decoded as UTF-8, so a value here means the text may be garbled.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub charset: Option<String>,
}

/// A single preserved URL, recoverable by its `index`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct UrlReference {
    pub index: usize,
    pub url: String,
    /// The anchor text the link was attached to (best-effort).
    pub text: String,
}

impl crate::refs::Referable for UrlReference {
    fn index(&self) -> usize {
        self.index
    }
    fn url(&self) -> &str {
        &self.url
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ContentType {
    Text,
    Markdown,
    Structured,
}

impl ContentType {
    pub fn parse(s: &str) -> Self {
        match s.to_ascii_lowercase().as_str() {
            "markdown" | "md" => ContentType::Markdown,
            "structured" | "json" => ContentType::Structured,
            _ => ContentType::Text,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct FetchOptions {
    pub url: String,
    pub content_type: ContentType,
    pub max_tokens: Option<usize>,
    pub timeout_secs: u64,
    /// TLS trust configuration (OS store is honoured by default; this carries
    /// the explicit `--ca-cert` / `--insecure` overrides).
    #[serde(default)]
    pub tls: TlsConfig,
}

impl Default for FetchOptions {
    fn default() -> Self {
        Self {
            url: String::new(),
            content_type: ContentType::Text,
            max_tokens: None,
            timeout_secs: 10,
            tls: TlsConfig::default(),
        }
    }
}
