//! HTTP primitives shared by the fetch and search paths: the user agent, the
//! response-body cap, and the retry classification. Both paths previously
//! carried their own copy of the last two and had drifted apart — search read
//! bodies without any cap at all.

use reqwest::{Response, StatusCode};

/// The user agent both paths send.
///
/// A tool-shaped agent (`webfetch/0.1.x`) is refused or challenged by a large
/// share of CDNs, which turns an ordinary fetch into an empty page. The search
/// path already sent a browser agent for exactly this reason; the fetch path
/// did not, and the mismatch was the single most common cause of a 403.
pub const USER_AGENT: &str = "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0 Safari/537.36";

/// Hard cap on the response body we will read (5 MiB). The HTML extractor turns
/// a page into a few KB of text, so a multi-megabyte body is almost never worth
/// the bandwidth, memory, and parse time — and an unbounded read is a DoS lever.
/// Bodies over the cap are *truncated* (not errored): partial content is still
/// useful and the extractor copes with truncated HTML.
///
/// The cap counts bytes *after* transparent gzip/brotli decoding, so it also
/// bounds a decompression bomb.
pub const MAX_BODY_BYTES: usize = 5 * 1024 * 1024;

/// Append as much of `chunk` to `buf` as fits under `max`. Returns `true` once
/// the cap is reached (the body is truncated and the caller should stop).
fn push_capped(buf: &mut Vec<u8>, chunk: &[u8], max: usize) -> bool {
    let remaining = max.saturating_sub(buf.len());
    if chunk.len() >= remaining {
        buf.extend_from_slice(&chunk[..remaining]);
        true
    } else {
        buf.extend_from_slice(chunk);
        false
    }
}

/// Read a response body, streaming chunks with a running byte cap so an
/// oversized body is bounded before it is ever parsed. The `bool` in the error
/// reports whether the read failure is transient (worth retrying).
pub async fn read_body_capped(mut resp: Response) -> Result<String, (anyhow::Error, bool)> {
    let mut buf: Vec<u8> = Vec::new();
    // Honour Content-Length to pre-size, but never trust it past the cap.
    if let Some(len) = resp.content_length() {
        buf.reserve(len.min(MAX_BODY_BYTES as u64) as usize);
    }
    loop {
        match resp.chunk().await {
            Ok(Some(chunk)) => {
                if push_capped(&mut buf, &chunk, MAX_BODY_BYTES) {
                    break;
                }
            }
            Ok(None) => break,
            Err(e) => {
                let transient = e.is_timeout();
                return Err((e.into(), transient));
            }
        }
    }
    Ok(String::from_utf8_lossy(&buf).into_owned())
}

/// Is a send/connect failure worth retrying?
pub fn transient_send_error(e: &reqwest::Error) -> bool {
    e.is_timeout() || e.is_connect() || e.is_request()
}

/// Is a response status worth retrying? Server errors and explicit throttling.
pub fn transient_status(status: StatusCode) -> bool {
    status.is_server_error() || status.as_u16() == 429
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_capped_truncates_oversized_chunk() {
        let mut buf = Vec::new();
        // A single chunk larger than the cap is clipped to the cap.
        let stopped = push_capped(&mut buf, &[b'x'; 10], 4);
        assert!(stopped);
        assert_eq!(buf.len(), 4);
    }

    #[test]
    fn push_capped_accumulates_until_cap() {
        let mut buf = Vec::new();
        assert!(!push_capped(&mut buf, b"abc", 8));
        assert!(!push_capped(&mut buf, b"de", 8));
        assert_eq!(buf, b"abcde");
        // Next chunk crosses the cap: only the remaining 3 bytes are kept.
        let stopped = push_capped(&mut buf, b"fghij", 8);
        assert!(stopped);
        assert_eq!(buf.len(), 8);
        assert_eq!(buf, b"abcdefgh");
    }

    #[test]
    fn push_capped_small_body_unaffected() {
        let mut buf = Vec::new();
        let stopped = push_capped(&mut buf, b"hello", 1024);
        assert!(!stopped);
        assert_eq!(buf, b"hello");
    }

    #[test]
    fn transient_status_covers_5xx_and_429() {
        assert!(transient_status(StatusCode::INTERNAL_SERVER_ERROR));
        assert!(transient_status(StatusCode::TOO_MANY_REQUESTS));
        assert!(!transient_status(StatusCode::NOT_FOUND));
        assert!(!transient_status(StatusCode::FORBIDDEN));
    }
}
