//! Shared primitives for the webfetch/websearch tools: text compression and
//! token budgeting ([`compress`]) and reference-style URL preservation
//! ([`refs`]). Both leaf crates re-export these so their internal modules can
//! keep using `crate::compress` / `crate::refs`.

pub mod compress;
pub mod refs;
