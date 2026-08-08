//! Integration tests for the HTTP path: retries, redirect re-validation, the
//! body cap, charset decoding, and the overall deadline.
//!
//! None of this had coverage before — every existing test converted HTML that
//! was already in hand. The tests here drive a real socket, using a hand-rolled
//! HTTP/1.1 responder rather than a mock-server dependency: the responses are a
//! few lines each, and the point is to exercise `fetch_page`, not a library.
//!
//! The server binds loopback, so the SSRF guard has to be relaxed for the test
//! process — that is what `WEBFETCH_ALLOW_PRIVATE` is for.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Once};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

use webfetch::tls::TlsConfig;

/// The guard rejects loopback, and the variable is process-global, so set it
/// once for the whole test binary.
fn allow_loopback() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| std::env::set_var("WEBFETCH_ALLOW_PRIVATE", "1"));
}

/// A canned reply, or a closure over the request count for stateful scenarios.
type Responder = Arc<dyn Fn(usize) -> Vec<u8> + Send + Sync>;

/// Serve `responder` on loopback until the test drops the returned handle.
/// Returns the base URL and a counter of requests actually received.
async fn serve(responder: Responder) -> (String, Arc<AtomicUsize>) {
    allow_loopback();
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("addr");
    let hits = Arc::new(AtomicUsize::new(0));

    let counter = Arc::clone(&hits);
    tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                return;
            };
            let responder = Arc::clone(&responder);
            let counter = Arc::clone(&counter);
            tokio::spawn(async move {
                // Read just enough to consume the request line and headers.
                let mut buf = [0u8; 4096];
                let _ = stream.read(&mut buf).await;
                let n = counter.fetch_add(1, Ordering::SeqCst);
                let _ = stream.write_all(&responder(n)).await;
                let _ = stream.flush().await;
            });
        }
    });

    (format!("http://{addr}"), hits)
}

fn response(status: &str, headers: &str, body: &[u8]) -> Vec<u8> {
    let mut out = format!(
        "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n{headers}\r\n",
        body.len()
    )
    .into_bytes();
    out.extend_from_slice(body);
    out
}

fn ok_html(body: &str) -> Vec<u8> {
    response("200 OK", "Content-Type: text/html\r\n", body.as_bytes())
}

async fn fetch(url: &str) -> anyhow::Result<webfetch::fetch::FetchedPage> {
    webfetch::fetch_page(url, 5, &TlsConfig::default()).await
}

// --- retries ----------------------------------------------------------------

#[tokio::test]
async fn a_transient_5xx_is_retried_and_then_succeeds() {
    let (base, hits) = serve(Arc::new(|n| {
        if n == 0 {
            response("503 Service Unavailable", "", b"nope")
        } else {
            ok_html("<html><body><article><p>recovered</p></article></body></html>")
        }
    }))
    .await;

    let page = fetch(&base).await.expect("should retry past the 503");
    assert!(page.body.contains("recovered"));
    assert_eq!(hits.load(Ordering::SeqCst), 2, "expected exactly one retry");
}

#[tokio::test]
async fn a_429_is_retried() {
    let (base, hits) = serve(Arc::new(|n| {
        if n < 2 {
            response("429 Too Many Requests", "", b"slow down")
        } else {
            ok_html("<html><body><p>ok</p></body></html>")
        }
    }))
    .await;

    fetch(&base).await.expect("429 is transient");
    assert_eq!(hits.load(Ordering::SeqCst), 3);
}

#[tokio::test]
async fn a_404_is_not_retried() {
    let (base, hits) = serve(Arc::new(|_| response("404 Not Found", "", b"gone"))).await;

    assert!(fetch(&base).await.is_err());
    assert_eq!(
        hits.load(Ordering::SeqCst),
        1,
        "a client error is final; retrying it just wastes the budget"
    );
}

// --- redirects --------------------------------------------------------------

#[tokio::test]
async fn redirects_are_followed_to_the_final_page() {
    let (base, hits) = serve(Arc::new(|n| match n {
        0 => response("302 Found", "Location: /second\r\n", b""),
        1 => response("302 Found", "Location: /third\r\n", b""),
        _ => ok_html("<html><body><article><p>arrived</p></article></body></html>"),
    }))
    .await;

    let page = fetch(&base).await.expect("follows redirects");
    assert!(page.body.contains("arrived"));
    assert!(page.final_url.ends_with("/third"), "{}", page.final_url);
    assert_eq!(hits.load(Ordering::SeqCst), 3);
}

#[tokio::test]
async fn a_redirect_loop_is_cut_off() {
    let (base, hits) = serve(Arc::new(|n| {
        response("302 Found", &format!("Location: /hop{n}\r\n"), b"")
    }))
    .await;

    let err = fetch(&base).await.expect_err("a loop must not run forever");
    assert!(format!("{err:#}").contains("too many redirects"), "{err:#}");
    // Initial request plus the redirect cap, and no more.
    assert!(hits.load(Ordering::SeqCst) <= 7, "{:?}", hits);
}

/// Every hop goes back through the guard, so a redirect cannot be used to reach
/// somewhere the initial URL was not allowed to.
#[tokio::test]
async fn a_redirect_to_a_blocked_scheme_is_refused() {
    let (base, _) = serve(Arc::new(|_| {
        response("302 Found", "Location: file:///etc/passwd\r\n", b"")
    }))
    .await;

    let err = fetch(&base).await.expect_err("file:// is not fetchable");
    assert!(format!("{err:#}").contains("scheme"), "{err:#}");
}

// --- body handling ----------------------------------------------------------

#[tokio::test]
async fn an_oversized_body_is_truncated_not_rejected() {
    let big = "x".repeat(6 * 1024 * 1024); // over the 5 MiB cap
    let body = Arc::new(format!(
        "<html><body><article><p>{big}</p></article></body></html>"
    ));
    let (base, _) = serve(Arc::new(move |_| ok_html(&body))).await;

    let page = fetch(&base).await.expect("partial content is still useful");
    assert!(
        page.body.len() <= 5 * 1024 * 1024,
        "body was {} bytes",
        page.body.len()
    );
    assert!(page.body.starts_with("<html>"));
}

#[tokio::test]
async fn a_latin1_page_is_decoded_with_its_declared_charset() {
    // "Café" in windows-1252 is not valid UTF-8; decoded as UTF-8 it is mojibake.
    let mut body = b"<html><body><article><p>Caf\xe9 na\xefve</p></article></body></html>".to_vec();
    let payload = Arc::new(std::mem::take(&mut body));
    let (base, _) = serve(Arc::new(move |_| {
        response(
            "200 OK",
            "Content-Type: text/html; charset=ISO-8859-1\r\n",
            &payload,
        )
    }))
    .await;

    let page = fetch(&base).await.expect("fetch");
    assert!(page.body.contains("Café naïve"), "body: {}", page.body);
    assert!(page.undecodable_charset.is_none());
}

#[tokio::test]
async fn an_undecodable_charset_is_reported() {
    let (base, _) = serve(Arc::new(|_| {
        response(
            "200 OK",
            "Content-Type: text/html; charset=Shift_JIS\r\n",
            b"<html><body><p>\x82\xa0</p></body></html>",
        )
    }))
    .await;

    let page = fetch(&base).await.expect("fetch");
    assert_eq!(page.undecodable_charset.as_deref(), Some("Shift_JIS"));
}

#[tokio::test]
async fn the_content_type_reaches_the_converter() {
    let (base, _) = serve(Arc::new(|_| {
        response(
            "200 OK",
            "Content-Type: application/json\r\n",
            br#"{"b":2,"a":1}"#,
        )
    }))
    .await;

    let options = webfetch::types::FetchOptions {
        url: base,
        ..Default::default()
    };
    let result = webfetch::fetch_and_convert(options).await.expect("fetch");
    assert_eq!(result.media, "json");
    // Pretty-printed rather than run through the HTML extractor.
    assert!(result.content.contains("\"a\": 1"), "{}", result.content);
}

// --- provenance and budget --------------------------------------------------

#[tokio::test]
async fn source_keeps_the_requested_url_across_a_redirect() {
    let (base, _) = serve(Arc::new(|n| {
        if n == 0 {
            response("302 Found", "Location: /moved\r\n", b"")
        } else {
            ok_html("<html><body><article><p>here</p></article></body></html>")
        }
    }))
    .await;

    let requested = format!("{base}/start");
    let options = webfetch::types::FetchOptions {
        url: requested.clone(),
        ..Default::default()
    };
    let result = webfetch::fetch_and_convert(options).await.expect("fetch");
    assert_eq!(result.source, requested, "source is what was asked for");
    assert!(
        result.final_url.ends_with("/moved"),
        "final_url is where it landed: {}",
        result.final_url
    );
    assert_ne!(result.source, result.final_url);
}

/// `--timeout` bounds one request; the whole fetch is bounded at a multiple of
/// it, so a chain of slow hops cannot run for minutes.
#[tokio::test]
async fn the_total_budget_stops_a_slow_redirect_chain() {
    allow_loopback();
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("addr");
    tokio::spawn(async move {
        let mut hop = 0usize;
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                return;
            };
            hop += 1;
            tokio::spawn(async move {
                let mut buf = [0u8; 4096];
                let _ = stream.read(&mut buf).await;
                // Slower than the per-request timeout below.
                tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
                let _ = stream
                    .write_all(&response(
                        "302 Found",
                        &format!("Location: /hop{hop}\r\n"),
                        b"",
                    ))
                    .await;
            });
        }
    });

    let started = std::time::Instant::now();
    let err = webfetch::fetch_page(&format!("http://{addr}"), 1, &TlsConfig::default())
        .await
        .expect_err("the chain never terminates");
    let elapsed = started.elapsed();

    assert!(
        elapsed < std::time::Duration::from_secs(10),
        "took {elapsed:?}; the total budget did not apply"
    );
    let message = format!("{err:#}");
    assert!(
        message.contains("budget") || message.contains("timed out") || message.contains("timeout"),
        "unexpected error: {message}"
    );
}
