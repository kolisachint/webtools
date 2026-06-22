//! Types for the web-search layer.

use serde::{Deserialize, Serialize};

/// A single search hit, carrying its reference index so the inline body can
/// cite `[N]` while the full URL lives in the reference block.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SearchResult {
    pub title: String,
    pub snippet: String,
    pub url: String,
    pub ref_index: usize,
}

/// A slim reference entry (index → URL) for the output's reference block.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Reference {
    pub index: usize,
    pub url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchOutput {
    pub query: String,
    pub results: Vec<SearchResult>,
    pub references: Vec<Reference>,
    pub token_estimate: usize,
    pub result_count: usize,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SearchOptions {
    pub query: String,
    pub max_results: Option<usize>,
    pub safe_search: Option<bool>,
    pub timeout_secs: u64,
}

impl Default for SearchOptions {
    fn default() -> Self {
        Self {
            query: String::new(),
            max_results: Some(5),
            safe_search: None,
            timeout_secs: 10,
        }
    }
}
