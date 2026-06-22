use serde::{Deserialize, Serialize};

/// Result of fetching and converting a web page.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FetchResult {
    pub title: String,
    pub final_url: String,
    pub content: String,
    pub content_type: ContentType,
    pub token_estimate: usize,
    pub references: Vec<UrlReference>,
    pub source: String,
}

/// A single preserved URL, recoverable by its `index`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct UrlReference {
    pub index: usize,
    pub url: String,
    /// The anchor text the link was attached to (best-effort).
    pub text: String,
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
}

impl Default for FetchOptions {
    fn default() -> Self {
        Self {
            url: String::new(),
            content_type: ContentType::Text,
            max_tokens: None,
            timeout_secs: 10,
        }
    }
}
