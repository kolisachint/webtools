//! Offline latency harness for the hot paths (HTML parse → reference-style
//! conversion / DDG result parsing). Network is excluded on purpose: it is
//! dominated by the remote server, whereas this measures our own code.
//!
//! Run with: `cargo run --release --example latency`

use std::time::Instant;

use webfetch::types::{ContentType, FetchOptions};

const DOCS: &str = include_str!("../crates/webfetch/tests/fixtures/docs.html");
const DDG: &str = include_str!("../crates/websearch/tests/fixtures/ddg_lite.html");

fn bench<F: FnMut()>(name: &str, iters: u32, mut f: F) {
    // Warm up.
    for _ in 0..100 {
        f();
    }
    let start = Instant::now();
    for _ in 0..iters {
        f();
    }
    let elapsed = start.elapsed();
    let per = elapsed.as_secs_f64() / iters as f64;
    let per_us = per * 1e6;
    let per_ms = per * 1e3;
    let ops = 1.0 / per;
    println!("{name:<32} {per_us:>9.2} µs/op   {per_ms:>7.3} ms/op   {ops:>12.0} ops/sec");
}

fn main() {
    let iters = 20_000;
    println!("== webtools offline latency ({iters} iters, release build) ==\n");

    let opts_text = FetchOptions {
        content_type: ContentType::Text,
        ..Default::default()
    };
    let opts_md = FetchOptions {
        content_type: ContentType::Markdown,
        ..Default::default()
    };
    let opts_struct = FetchOptions {
        content_type: ContentType::Structured,
        ..Default::default()
    };

    bench("fetch: html → text+refs", iters, || {
        let _ = webfetch::convert_html(DOCS, "https://docs.example.com/page", &opts_text);
    });
    bench("fetch: html → markdown", iters, || {
        let _ = webfetch::convert_html(DOCS, "https://docs.example.com/page", &opts_md);
    });
    bench("fetch: html → structured", iters, || {
        let _ = webfetch::convert_html(DOCS, "https://docs.example.com/page", &opts_struct);
    });
    bench("search: ddg-lite → results", iters, || {
        let _ = websearch::build_output("react 19", DDG, 10);
    });

    // Token-saver evidence: how much smaller is the reference-style output?
    let result = webfetch::convert_html(DOCS, "https://docs.example.com/page", &opts_text);
    println!(
        "\nfetch sample: input {} B HTML → {} B content (~{} tokens), {} references preserved",
        DOCS.len(),
        result.content.len(),
        result.token_estimate,
        result.references.len(),
    );

    let search = websearch::build_output("react 19", DDG, 10);
    println!(
        "search sample: input {} B HTML → ~{} tokens, {} results, {} references preserved",
        DDG.len(),
        search.token_estimate,
        search.result_count,
        search.references.len(),
    );
}
