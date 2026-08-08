//! Shared primitives for the webfetch/websearch tools: text compression and
//! token budgeting ([`compress`]), reference-style URL preservation and the
//! reference-aware token budget ([`refs`]), HTTP fetch primitives — user agent,
//! body cap, retry classification ([`http`]) — and shared HTTP-client TLS trust
//! configuration ([`tls`]). Both leaf crates re-export these so their internal
//! modules can keep using `crate::compress` / `crate::refs` / `crate::tls`.

pub mod compress;
pub mod http;
pub mod refs;
pub mod tls;
