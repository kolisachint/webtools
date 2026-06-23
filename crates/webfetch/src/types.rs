use serde::{Deserialize, Serialize};

pub use crate::tls::TlsConfig;

/// Result of fetching and converting a web page.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FetchResult {
    pub title: String,
    pub final_url: String,
    pub content: String,
    pub content_type: ContentType,
    /// The detected source media kind: "html", "json", "text", or a raw
    /// content-type for anything not rendered.
    pub media: String,
    pub token_estimate: usize,
    pub references: Vec<UrlReference>,
    #[serde(default)]
    pub metadata: Metadata,
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
