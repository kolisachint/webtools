//! Shared reference-style URL preservation.
//!
//! Both the fetch path ([`crate::convert`]) and the search path
//! ([`crate::search`]) cite URLs with inline `[N]` markers and collect the
//! full URLs into a trailing block. This module owns the one canonical
//! rendering of that block so the two paths cannot drift apart.

use serde::{Deserialize, Serialize};

/// Anything that can be listed in a reference block: an index and a URL.
pub trait Referable {
    fn index(&self) -> usize;
    fn url(&self) -> &str;
}

/// A slim reference entry (index → URL) for an output's reference block.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Reference {
    pub index: usize,
    pub url: String,
}

impl Referable for Reference {
    fn index(&self) -> usize {
        self.index
    }
    fn url(&self) -> &str {
        &self.url
    }
}

/// Render references into the canonical block:
///
/// ```text
/// References:
/// [1] https://example.com/a
/// [2] https://example.com/b
/// ```
///
/// Returns an empty string when there are no references.
pub fn render_block<T: Referable>(references: &[T]) -> String {
    if references.is_empty() {
        return String::new();
    }
    let mut s = String::from("References:\n");
    for r in references {
        s.push_str(&format!("[{}] {}\n", r.index(), r.url()));
    }
    s.truncate(s.trim_end().len());
    s
}
