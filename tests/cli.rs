//! End-to-end tests for the `webtools` binary: the offline `fetch` path, the
//! token budget, output formats, exit codes, and the config file.
//!
//! Only `tests/mcp.rs` drove the binary before, and only for `initialize` and
//! `tools/list` — nothing covered `--from-file`, `--max-tokens`, `--json`, or
//! what any of it exits with.

use std::io::Write;
use std::process::{Command, Output, Stdio};

/// Run the binary and capture its output. `env` entries are applied on top of
/// a cleared HOME so a developer's real config never leaks into a test.
fn run(args: &[&str], env: &[(&str, &str)], stdin: Option<&str>) -> Output {
    let dir = tempdir();
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_webtools"));
    cmd.args(args)
        .env("HOME", &dir)
        .env_remove("HOOCODE_CONFIG")
        .env_remove("WEBTOOLS_SEARCH_PROVIDER")
        .env_remove("BRAVE_API_KEY")
        .env_remove("TAVILY_API_KEY")
        .stdin(if stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (k, v) in env {
        cmd.env(k, v);
    }

    let mut child = cmd.spawn().expect("spawn webtools");
    if let Some(text) = stdin {
        child
            .stdin
            .take()
            .expect("stdin")
            .write_all(text.as_bytes())
            .expect("write stdin");
    }
    child.wait_with_output().expect("wait")
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

/// A unique scratch directory for a test's fake HOME.
fn tempdir() -> std::path::PathBuf {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static N: AtomicUsize = AtomicUsize::new(0);
    let dir = std::env::temp_dir().join(format!(
        "webtools-cli-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    ));
    std::fs::create_dir_all(&dir).expect("create temp home");
    dir
}

fn write_file(name: &str, contents: &str) -> std::path::PathBuf {
    let dir = tempdir();
    let path = dir.join(name);
    std::fs::write(&path, contents).expect("write fixture");
    path
}

const DOC: &str = r#"<html><head><title>Widgets</title></head><body><article>
<h2>Reference</h2>
<table><tr><th>Name</th><th>Type</th></tr><tr><td>alpha</td><td>string</td></tr></table>
<p>See the <a href="/api/v2/users">users endpoint</a> for details.</p>
</article></body></html>"#;

/// A document too long for one window, for the paging tests.
fn long_doc() -> String {
    let paragraphs = (1..=60)
        .map(|i| format!("<p>Paragraph {i} explains one more part of the widget system in enough words to cost tokens.</p>"))
        .collect::<Vec<_>>()
        .join("\n");
    format!("<html><head><title>Widgets</title></head><body><article>{paragraphs}</article></body></html>")
}

// --- offline fetch ----------------------------------------------------------

#[test]
fn from_file_renders_a_citation_header_and_references() {
    let path = write_file("doc.html", DOC);
    let out = run(
        &[
            "fetch",
            "--from-file",
            path.to_str().unwrap(),
            "--url",
            "https://docs.test/page",
        ],
        &[],
        None,
    );
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let text = stdout(&out);
    assert!(
        text.starts_with("Widgets\nhttps://docs.test/page"),
        "{text}"
    );
    assert!(text.contains("users endpoint [1]"), "{text}");
    assert!(
        text.contains("[1] https://docs.test/api/v2/users"),
        "{text}"
    );
    // Table cells are separated rather than run together.
    assert!(text.contains("Name | Type"), "{text}");
}

#[test]
fn from_file_reads_stdin() {
    let out = run(
        &["fetch", "--from-file", "-", "--url", "https://docs.test/"],
        &[],
        Some(DOC),
    );
    assert!(out.status.success());
    assert!(stdout(&out).contains("users endpoint [1]"));
}

/// A non-UTF-8 file used to abort the command with "stream did not contain
/// valid UTF-8" and no indication of which file.
#[test]
fn a_non_utf8_file_is_read_lossily() {
    let dir = tempdir();
    let path = dir.join("latin1.html");
    std::fs::write(
        &path,
        b"<html><body><article><p>Caf\xe9 na\xefve</p></article></body></html>".as_slice(),
    )
    .expect("write");
    let out = run(
        &[
            "fetch",
            "--from-file",
            path.to_str().unwrap(),
            "--url",
            "https://x.test/",
        ],
        &[],
        None,
    );
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert!(stdout(&out).contains("Caf"), "{}", stdout(&out));
}

/// `--from-file` has no Content-Type to read, so it has to honour
/// `<meta charset>` — otherwise the offline path mangles pages the network
/// path handles fine.
#[test]
fn from_file_decodes_a_declared_multibyte_charset() {
    let cases: [(&str, &[u8], &str); 3] = [
        // "こんにちは" in Shift_JIS.
        (
            "sjis.html",
            b"<html><head><meta charset=\"shift_jis\"></head><body><article><p>\x82\xb1\x82\xf1\x82\xc9\x82\xbf\x82\xcd</p></article></body></html>",
            "こんにちは",
        ),
        // "中文" in GBK.
        (
            "gbk.html",
            b"<html><head><meta charset=\"gbk\"></head><body><article><p>\xd6\xd0\xce\xc4</p></article></body></html>",
            "中文",
        ),
        // "한국" in EUC-KR.
        (
            "euckr.html",
            b"<html><head><meta charset=\"euc-kr\"></head><body><article><p>\xc7\xd1\xb1\xb9</p></article></body></html>",
            "한국",
        ),
    ];

    for (name, bytes, expected) in cases {
        let dir = tempdir();
        let path = dir.join(name);
        std::fs::write(&path, bytes).expect("write");
        let out = run(
            &[
                "fetch",
                "--from-file",
                path.to_str().unwrap(),
                "--url",
                "https://x.test/",
                "--json",
            ],
            &[],
            None,
        );
        assert!(out.status.success(), "{name}: {}", stderr(&out));
        let v: serde_json::Value = serde_json::from_str(&stdout(&out)).expect("valid json");
        assert!(
            v["content"].as_str().unwrap().contains(expected),
            "{name}: expected {expected:?}, got {:?}",
            v["content"]
        );
        assert!(
            v["metadata"]["charset"].is_null(),
            "{name}: an exact decode must not warn"
        );
    }
}

/// A label no encoding matches must still be surfaced, or a garbled page looks
/// like a correct one.
#[test]
fn an_unrecognized_charset_label_warns() {
    let path = write_file(
        "bogus.html",
        "<html><head><meta charset=\"x-made-up\"></head><body><article><p>hi</p></article></body></html>",
    );
    let out = run(
        &[
            "fetch",
            "--from-file",
            path.to_str().unwrap(),
            "--url",
            "https://x.test/",
        ],
        &[],
        None,
    );
    assert!(out.status.success());
    assert!(stderr(&out).contains("x-made-up"), "{}", stderr(&out));
}

#[test]
fn a_missing_file_reports_the_path() {
    let out = run(&["fetch", "--from-file", "/no/such/file.html"], &[], None);
    assert!(!out.status.success());
    assert!(
        stderr(&out).contains("/no/such/file.html"),
        "{}",
        stderr(&out)
    );
}

#[test]
fn fetch_without_a_url_or_file_fails() {
    let out = run(&["fetch"], &[], None);
    assert!(!out.status.success());
    assert!(stderr(&out).contains("--url"), "{}", stderr(&out));
}

#[test]
fn from_file_without_a_url_warns_that_links_are_dropped() {
    let path = write_file("doc.html", DOC);
    let out = run(&["fetch", "--from-file", path.to_str().unwrap()], &[], None);
    assert!(out.status.success());
    assert!(stderr(&out).contains("relative links"), "{}", stderr(&out));
}

// --- budget and formats -----------------------------------------------------

/// A budgeted fetch has to say what it is a slice of and where to resume;
/// without it the CLI ends mid-sentence with nothing to act on.
#[test]
fn a_budgeted_fetch_prints_where_it_stopped_and_how_to_continue() {
    let path = write_file("long.html", &long_doc());
    let out = run(
        &[
            "fetch",
            "--from-file",
            path.to_str().unwrap(),
            "--url",
            "https://docs.test/page",
            "--max-tokens",
            "40",
        ],
        &[],
        None,
    );

    let text = stdout(&out);
    assert!(
        text.contains("continue with --offset"),
        "no continuation footer: {text}"
    );
    assert!(text.contains("tokens);"), "no token accounting: {text}");
}

/// Following the printed offsets reads the document once through, and the run
/// that finishes it prints no footer — the signal that there is nothing left.
#[test]
fn offset_pages_through_a_document_and_stops_at_the_end() {
    let path = write_file("long.html", &long_doc());
    let mut offset = 0usize;
    let mut runs = 0;

    loop {
        let offset_arg = offset.to_string();
        let out = run(
            &[
                "fetch",
                "--from-file",
                path.to_str().unwrap(),
                "--url",
                "https://docs.test/page",
                "--max-tokens",
                "40",
                "--offset",
                &offset_arg,
            ],
            &[],
            None,
        );
        assert!(out.status.success(), "fetch failed: {}", stderr(&out));

        let text = stdout(&out);
        runs += 1;
        assert!(runs < 100, "paging failed to terminate");

        let Some(next) = text
            .rsplit("continue with --offset ")
            .next()
            .and_then(|tail| tail.trim_end().trim_end_matches(']').parse::<usize>().ok())
        else {
            break;
        };
        assert!(next > offset, "offset must advance: {offset} -> {next}");
        offset = next;
    }

    assert!(runs > 2, "expected several pages, got {runs}");
}

/// A whole document inside the budget is complete, so it must not invite a
/// pointless second call.
#[test]
fn a_complete_fetch_prints_no_continuation() {
    let path = write_file("doc.html", DOC);
    let out = run(
        &[
            "fetch",
            "--from-file",
            path.to_str().unwrap(),
            "--url",
            "https://docs.test/page",
        ],
        &[],
        None,
    );

    assert!(!stdout(&out).contains("continue with --offset"));
}

#[test]
fn max_tokens_bounds_the_whole_output_references_included() {
    let mut html = String::from("<html><head><title>Links</title></head><body><article>");
    for i in 0..120 {
        html.push_str(&format!(
            "<p>Item {i}: see <a href=\"https://example.com/very/long/path/{i}\
             ?query=value#frag\">link {i}</a>.</p>"
        ));
    }
    html.push_str("</article></body></html>");
    let path = write_file("links.html", &html);

    for budget in ["50", "200", "1000"] {
        let out = run(
            &[
                "fetch",
                "--from-file",
                path.to_str().unwrap(),
                "--url",
                "https://x.test/",
                "--max-tokens",
                budget,
                "--json",
            ],
            &[],
            None,
        );
        assert!(out.status.success(), "stderr: {}", stderr(&out));
        let v: serde_json::Value = serde_json::from_str(&stdout(&out)).expect("valid json");
        let estimate = v["token_estimate"].as_u64().expect("token_estimate");
        let cap: u64 = budget.parse().unwrap();
        assert!(
            estimate <= cap,
            "budget {budget} produced {estimate} tokens"
        );
    }
}

#[test]
fn json_output_carries_status_and_provenance() {
    let path = write_file("doc.html", DOC);
    let out = run(
        &[
            "fetch",
            "--from-file",
            path.to_str().unwrap(),
            "--url",
            "https://docs.test/page",
            "--json",
        ],
        &[],
        None,
    );
    let v: serde_json::Value = serde_json::from_str(&stdout(&out)).expect("valid json");
    assert_eq!(v["status"], "ok");
    assert_eq!(v["media"], "html");
    assert_eq!(v["source"], "https://docs.test/page");
    assert_eq!(v["references"][0]["url"], "https://docs.test/api/v2/users");
}

#[test]
fn markdown_output_keeps_links_inline_and_lists_them() {
    let path = write_file("doc.html", DOC);
    let out = run(
        &[
            "fetch",
            "--from-file",
            path.to_str().unwrap(),
            "--url",
            "https://docs.test/page",
            "--output",
            "markdown",
            "--json",
        ],
        &[],
        None,
    );
    let v: serde_json::Value = serde_json::from_str(&stdout(&out)).expect("valid json");
    let content = v["content"].as_str().unwrap();
    assert!(
        content.contains("[users endpoint](https://docs.test/api/v2/users)"),
        "{content}"
    );
    assert!(!v["references"].as_array().unwrap().is_empty());
}

#[test]
fn structured_output_is_parseable_and_typed() {
    let path = write_file("doc.html", DOC);
    let out = run(
        &[
            "fetch",
            "--from-file",
            path.to_str().unwrap(),
            "--url",
            "https://docs.test/page",
            "--output",
            "structured",
        ],
        &[],
        None,
    );
    let v: serde_json::Value = serde_json::from_str(
        stdout(&out)
            .split_once("\n\n")
            .map(|(_, rest)| rest)
            .unwrap_or(&stdout(&out)),
    )
    .expect("structured output is JSON");
    let kinds: Vec<&str> = v["blocks"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|b| b["kind"].as_str())
        .collect();
    assert!(kinds.contains(&"heading"), "kinds: {kinds:?}");
    assert!(kinds.contains(&"tablerow"), "kinds: {kinds:?}");
}

// --- status reporting -------------------------------------------------------

#[test]
fn a_javascript_rendered_shell_is_reported_as_needing_js() {
    let path = write_file(
        "spa.html",
        "<html><head><title>App</title></head><body><div id=\"root\"></div>\
         <script src=\"/app.js\"></script></body></html>",
    );
    let out = run(
        &[
            "fetch",
            "--from-file",
            path.to_str().unwrap(),
            "--url",
            "https://spa.test/",
            "--json",
        ],
        &[],
        None,
    );
    let v: serde_json::Value = serde_json::from_str(&stdout(&out)).expect("valid json");
    assert_eq!(v["status"], "needs_js");
    assert!(stderr(&out).contains("JavaScript"), "{}", stderr(&out));
}

#[test]
fn a_pathologically_nested_document_is_refused() {
    let depth = 20_000;
    let html = format!(
        "<html><body>{}<p>x</p>{}</body></html>",
        "<div>".repeat(depth),
        "</div>".repeat(depth)
    );
    let path = write_file("deep.html", &html);

    let started = std::time::Instant::now();
    let out = run(
        &[
            "fetch",
            "--from-file",
            path.to_str().unwrap(),
            "--url",
            "https://x.test/",
            "--json",
        ],
        &[],
        None,
    );
    let elapsed = started.elapsed();

    let v: serde_json::Value = serde_json::from_str(&stdout(&out)).expect("valid json");
    assert_eq!(v["status"], "too_complex");
    assert!(
        elapsed < std::time::Duration::from_secs(10),
        "took {elapsed:?}; the guard did not short-circuit"
    );
}

// --- configuration ----------------------------------------------------------

#[test]
fn a_provider_without_a_key_fails_loudly_rather_than_searching_elsewhere() {
    let out = run(
        &["search", "--query", "x", "--provider", "brave"],
        &[],
        None,
    );
    assert!(!out.status.success());
    let err = stderr(&out);
    assert!(err.contains("BRAVE_API_KEY"), "{err}");
    assert!(err.contains("api_key"), "{err}");
}

#[test]
fn an_unknown_provider_is_rejected() {
    let out = run(
        &["search", "--query", "x", "--provider", "altavista"],
        &[],
        None,
    );
    assert!(!out.status.success());
    assert!(stderr(&out).contains("altavista"), "{}", stderr(&out));
}

#[test]
fn fetch_defaults_come_from_the_config_file() {
    let dir = tempdir();
    let config = dir.join("settings.json");
    std::fs::write(
        &config,
        r#"{ "unrelatedTool": { "x": 1 },
             "webtools": { "fetch": { "max_tokens": 40 } } }"#,
    )
    .expect("write config");

    let mut html = String::from("<html><head><title>T</title></head><body><article>");
    for i in 0..80 {
        html.push_str(&format!(
            "<p>Paragraph {i} with a good deal of filler text.</p>"
        ));
    }
    html.push_str("</article></body></html>");
    let path = write_file("long.html", &html);

    let out = run(
        &[
            "fetch",
            "--from-file",
            path.to_str().unwrap(),
            "--url",
            "https://x.test/",
            "--json",
        ],
        &[("HOOCODE_CONFIG", config.to_str().unwrap())],
        None,
    );
    let v: serde_json::Value = serde_json::from_str(&stdout(&out)).expect("valid json");
    let estimate = v["token_estimate"].as_u64().unwrap();
    assert!(estimate <= 40, "config max_tokens ignored: {estimate}");
}

/// A flag has to win over the file, or the file silently overrides intent.
#[test]
fn a_flag_beats_the_config_file() {
    let dir = tempdir();
    let config = dir.join("settings.json");
    std::fs::write(
        &config,
        r#"{ "webtools": { "fetch": { "max_tokens": 40 } } }"#,
    )
    .expect("write config");

    let mut html = String::from("<html><head><title>T</title></head><body><article>");
    for i in 0..80 {
        html.push_str(&format!(
            "<p>Paragraph {i} with a good deal of filler text.</p>"
        ));
    }
    html.push_str("</article></body></html>");
    let path = write_file("long.html", &html);

    let out = run(
        &[
            "fetch",
            "--from-file",
            path.to_str().unwrap(),
            "--url",
            "https://x.test/",
            "--max-tokens",
            "400",
            "--json",
        ],
        &[("HOOCODE_CONFIG", config.to_str().unwrap())],
        None,
    );
    let v: serde_json::Value = serde_json::from_str(&stdout(&out)).expect("valid json");
    let estimate = v["token_estimate"].as_u64().unwrap();
    assert!(estimate > 40, "the flag was ignored: {estimate}");
    assert!(estimate <= 400, "over the flag's budget: {estimate}");
}

#[test]
fn a_malformed_config_warns_and_the_command_still_runs() {
    let dir = tempdir();
    let config = dir.join("settings.json");
    std::fs::write(&config, "{ this is not json").expect("write config");
    let path = write_file("doc.html", DOC);

    let out = run(
        &[
            "fetch",
            "--from-file",
            path.to_str().unwrap(),
            "--url",
            "https://x.test/",
        ],
        &[("HOOCODE_CONFIG", config.to_str().unwrap())],
        None,
    );
    assert!(
        out.status.success(),
        "a broken config must not break the tool"
    );
    let err = stderr(&out);
    assert!(err.contains("invalid JSON"), "{err}");
    // The warning locates the problem without echoing the file, which holds keys.
    assert!(!err.contains("this is not json"), "{err}");
}

#[test]
fn a_missing_config_is_silent() {
    let path = write_file("doc.html", DOC);
    let out = run(
        &[
            "fetch",
            "--from-file",
            path.to_str().unwrap(),
            "--url",
            "https://x.test/",
        ],
        &[("HOOCODE_CONFIG", "/no/such/settings.json")],
        None,
    );
    assert!(out.status.success());
    assert!(!stderr(&out).contains("settings.json"), "{}", stderr(&out));
}

// --- page cache -------------------------------------------------------------

/// Serve one canned HTTP response per connection on loopback, counting
/// requests. A plain `std` listener on a thread: the point is to observe how
/// many times the binary hits the network, not to be an HTTP server.
fn serve_counting(body: String) -> (String, std::sync::Arc<std::sync::atomic::AtomicUsize>) {
    use std::io::{Read, Write};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().expect("addr");
    let hits = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&hits);

    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { break };
            counter.fetch_add(1, Ordering::SeqCst);
            let mut buf = [0u8; 2048];
            let _ = stream.read(&mut buf);
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = stream.write_all(response.as_bytes());
            let _ = stream.flush();
        }
    });

    (format!("http://{addr}/doc"), hits)
}

/// Paging exists to keep a long page out of a context window; refetching it per
/// window would trade that for a download per window, and let the page change
/// underneath a read whose offsets came from an earlier snapshot.
#[test]
fn paging_a_document_downloads_it_once() {
    let cache_dir = tempdir().join("cache");
    let (url, hits) = serve_counting(long_doc());
    let env = [
        // The SSRF guard rejects loopback, which is where the test server is.
        ("WEBFETCH_ALLOW_PRIVATE", "1"),
        ("WEBTOOLS_CACHE_DIR", cache_dir.to_str().unwrap()),
    ];

    let first = run(&["fetch", "--url", &url, "--max-tokens", "40"], &env, None);
    assert!(
        first.status.success(),
        "first fetch failed: {}",
        stderr(&first)
    );
    let text = stdout(&first);
    let next = text
        .rsplit("continue with --offset ")
        .next()
        .and_then(|tail| tail.trim_end().trim_end_matches(']').parse::<usize>().ok())
        .unwrap_or_else(|| panic!("no continuation footer: {text}"));

    let offset = next.to_string();
    let second = run(
        &[
            "fetch",
            "--url",
            &url,
            "--max-tokens",
            "40",
            "--offset",
            &offset,
        ],
        &env,
        None,
    );
    assert!(
        second.status.success(),
        "second fetch failed: {}",
        stderr(&second)
    );
    assert!(
        stdout(&second).contains("showing bytes"),
        "second window not rendered"
    );

    assert_eq!(
        hits.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "the second window refetched the page"
    );
}

/// The escape hatch has to actually bypass the cache: a stale page must be
/// re-readable on demand.
#[test]
fn no_cache_refetches_every_time() {
    let cache_dir = tempdir().join("cache-off");
    let (url, hits) = serve_counting(long_doc());
    let env = [
        ("WEBFETCH_ALLOW_PRIVATE", "1"),
        ("WEBTOOLS_CACHE_DIR", cache_dir.to_str().unwrap()),
    ];

    for _ in 0..2 {
        let out = run(&["fetch", "--url", &url, "--no-cache"], &env, None);
        assert!(out.status.success(), "fetch failed: {}", stderr(&out));
    }

    assert_eq!(hits.load(std::sync::atomic::Ordering::SeqCst), 2);
}

/// A cache that cannot be written must not break fetching.
#[test]
fn an_unwritable_cache_directory_does_not_fail_the_fetch() {
    let (url, _hits) = serve_counting(long_doc());
    let out = run(
        &["fetch", "--url", &url],
        &[
            ("WEBFETCH_ALLOW_PRIVATE", "1"),
            // A path under a regular file can never be created.
            ("WEBTOOLS_CACHE_DIR", "/etc/hosts/nope"),
        ],
        None,
    );

    assert!(out.status.success(), "fetch failed: {}", stderr(&out));
    assert!(stdout(&out).contains("Paragraph 1"));
}

// --- outline ----------------------------------------------------------------

/// A document with headings, for the outline tests.
fn sectioned_doc() -> String {
    let sections = ["Installation", "Configuration", "Troubleshooting"]
        .iter()
        .map(|name| {
            let body = (1..=6)
                .map(|i| {
                    format!("Sentence {i} of the {name} section, with enough words to cost tokens.")
                })
                .collect::<Vec<_>>()
                .join(" ");
            format!("<h2>{name}</h2><p>{body}</p>")
        })
        .collect::<Vec<_>>()
        .join("");
    format!("<html><head><title>Handbook</title></head><body><article>{sections}</article></body></html>")
}

#[test]
fn outline_lists_the_headings_instead_of_the_body() {
    let path = write_file("handbook.html", &sectioned_doc());
    let out = run(
        &[
            "fetch",
            "--from-file",
            path.to_str().unwrap(),
            "--url",
            "https://docs.test/handbook",
            "--outline",
        ],
        &[],
        None,
    );

    let text = stdout(&out);
    assert!(text.contains("Installation — offset 0"), "{text}");
    assert!(text.contains("Configuration — offset"), "{text}");
    assert!(text.contains("Troubleshooting — offset"), "{text}");
    // The map, not the territory: the body itself must not come with it.
    assert!(
        !text.contains("Sentence 1 of the"),
        "outline included the body: {text}"
    );
}

/// The point of the outline: its offsets are the ones `--offset` reads, so a
/// caller can map a page cheaply and then fetch only the section it needs.
#[test]
fn an_outline_offset_reads_that_section() {
    let path = write_file("handbook.html", &sectioned_doc());
    let args = [
        "fetch",
        "--from-file",
        path.to_str().unwrap(),
        "--url",
        "https://docs.test/handbook",
    ];

    let outline = stdout(&run(&[args.as_slice(), &["--outline"]].concat(), &[], None));
    let offset = outline
        .lines()
        .find(|line| line.contains("Configuration — offset"))
        .and_then(|line| line.split("offset ").nth(1))
        .and_then(|rest| rest.split(',').next())
        .and_then(|n| n.trim().parse::<usize>().ok())
        .unwrap_or_else(|| panic!("no Configuration row: {outline}"));

    let offset = offset.to_string();
    let section = stdout(&run(
        &[
            args.as_slice(),
            &["--offset", &offset, "--max-tokens", "60"],
        ]
        .concat(),
        &[],
        None,
    ));

    assert!(section.contains("Configuration"), "{section}");
    assert!(
        section.contains("Sentence 1 of the Configuration"),
        "{section}"
    );
    assert!(
        !section.contains("Sentence 1 of the Installation"),
        "read started before the section: {section}"
    );
}

#[test]
fn a_document_without_headings_says_it_has_no_outline() {
    let path = write_file(
        "flat.html",
        "<html><body><article><p>Prose only.</p></article></body></html>",
    );
    let out = run(
        &[
            "fetch",
            "--from-file",
            path.to_str().unwrap(),
            "--url",
            "https://docs.test/flat",
            "--outline",
        ],
        &[],
        None,
    );

    assert!(stdout(&out).contains("no outline"), "{}", stdout(&out));
}

#[test]
fn outline_entries_travel_in_json_too() {
    let path = write_file("handbook.html", &sectioned_doc());
    let out = run(
        &[
            "fetch",
            "--from-file",
            path.to_str().unwrap(),
            "--url",
            "https://docs.test/handbook",
            "--outline",
            "--json",
        ],
        &[],
        None,
    );

    let text = stdout(&out);
    assert!(text.contains("\"outline\""), "{text}");
    assert!(text.contains("\"token_estimate\""), "{text}");
    assert!(text.contains("\"Configuration\""), "{text}");
}

/// A plain fetch must not start paying for a field it did not ask for.
#[test]
fn a_fetch_without_outline_carries_no_outline_field() {
    let path = write_file("handbook.html", &sectioned_doc());
    let out = run(
        &[
            "fetch",
            "--from-file",
            path.to_str().unwrap(),
            "--url",
            "https://docs.test/handbook",
            "--json",
        ],
        &[],
        None,
    );

    assert!(!stdout(&out).contains("\"outline\""));
}

// --- grep -------------------------------------------------------------------

#[test]
fn grep_reports_where_a_page_mentions_something() {
    let path = write_file("handbook.html", &sectioned_doc());
    let out = run(
        &[
            "fetch",
            "--from-file",
            path.to_str().unwrap(),
            "--url",
            "https://docs.test/handbook",
            "--grep",
            "Configuration",
        ],
        &[],
        None,
    );

    let text = stdout(&out);
    assert!(text.contains("offset "), "{text}");
    assert!(text.contains("at the offset shown"), "{text}");
    // A search returns locations, not the page: a snippet carries the text
    // around its own hit, and nothing from the far end of the document.
    assert!(
        !text.contains("Sentence 6 of the Troubleshooting"),
        "{text}"
    );
}

/// The point of a search: its offsets are the ones `--offset` reads, so a hit
/// is followed by fetching it.
#[test]
fn a_grep_offset_reads_that_part_of_the_page() {
    let path = write_file("handbook.html", &sectioned_doc());
    let args = [
        "fetch",
        "--from-file",
        path.to_str().unwrap(),
        "--url",
        "https://docs.test/handbook",
    ];

    let hits = stdout(&run(
        &[args.as_slice(), &["--grep", "Troubleshooting"]].concat(),
        &[],
        None,
    ));
    let offset = hits
        .lines()
        .find(|line| line.starts_with("offset "))
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|n| n.parse::<usize>().ok())
        .unwrap_or_else(|| panic!("no hit: {hits}"));

    let offset = offset.to_string();
    let read = stdout(&run(
        &[
            args.as_slice(),
            &["--offset", &offset, "--max-tokens", "60"],
        ]
        .concat(),
        &[],
        None,
    ));

    assert!(read.contains("Troubleshooting"), "{read}");
}

#[test]
fn a_pattern_that_matches_nothing_says_so() {
    let path = write_file("handbook.html", &sectioned_doc());
    let out = run(
        &[
            "fetch",
            "--from-file",
            path.to_str().unwrap(),
            "--url",
            "https://docs.test/handbook",
            "--grep",
            "websockets",
        ],
        &[],
        None,
    );

    assert!(stdout(&out).contains("no matches"), "{}", stdout(&out));
}

/// An unusable pattern is the caller's to fix, so say which part was rejected
/// rather than returning the page as though nothing was asked.
#[test]
fn an_invalid_pattern_is_reported_not_ignored() {
    let path = write_file("handbook.html", &sectioned_doc());
    let out = run(
        &[
            "fetch",
            "--from-file",
            path.to_str().unwrap(),
            "--url",
            "https://docs.test/handbook",
            "--grep",
            "(unclosed",
        ],
        &[],
        None,
    );

    let text = stdout(&out);
    assert!(text.contains("invalid search pattern"), "{text}");
    assert!(!text.contains("Sentence 1 of the Installation"), "{text}");
}

/// Two views of one page at once has no meaning, so the CLI refuses rather
/// than silently picking one.
#[test]
fn grep_and_outline_cannot_be_combined() {
    let path = write_file("handbook.html", &sectioned_doc());
    let out = run(
        &[
            "fetch",
            "--from-file",
            path.to_str().unwrap(),
            "--url",
            "https://docs.test/handbook",
            "--outline",
            "--grep",
            "Configuration",
        ],
        &[],
        None,
    );

    assert!(!out.status.success());
    assert!(
        stderr(&out).contains("cannot be used with"),
        "{}",
        stderr(&out)
    );
}

#[test]
fn a_fetch_without_grep_carries_no_matches_field() {
    let path = write_file("handbook.html", &sectioned_doc());
    let out = run(
        &[
            "fetch",
            "--from-file",
            path.to_str().unwrap(),
            "--url",
            "https://docs.test/handbook",
            "--json",
        ],
        &[],
        None,
    );

    assert!(!stdout(&out).contains("\"matches\""));
}
