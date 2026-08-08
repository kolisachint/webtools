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
