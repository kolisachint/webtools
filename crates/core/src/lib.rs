//! Shared primitives for the webfetch/websearch tools: text compression and
//! token budgeting ([`compress`]), reference-style URL preservation
//! ([`refs`]), and shared HTTP-client TLS trust configuration ([`tls`]). Both
//! leaf crates re-export these so their internal modules can keep using
//! `crate::compress` / `crate::refs` / `crate::tls`.

pub mod compress;
pub mod refs;
pub mod tls;
