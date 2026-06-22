# webtools-search

Zero-infrastructure web search (DuckDuckGo Lite) with **reference-style URLs**,
part of [`webtools`](https://github.com/kolisachint/webtools). No API key, no
backend. Each hit's title carries an inline `[N]` marker while the full URLs are
collected into a recoverable reference block, keeping the context window tight.

> Published on crates.io as `webtools-search`; the library import name is
> `websearch`.

```toml
[dependencies]
websearch = { package = "webtools-search", version = "0.1" }
```

```rust
use websearch::types::SearchOptions;

let output = websearch::run_search(SearchOptions {
    query: "rust async runtime".into(),
    max_results: Some(5),
    ..Default::default()
}).await?;

for hit in &output.results {
    println!("{} [{}]", hit.title, hit.ref_index);
}
```

DDG Lite's `//duckduckgo.com/l/?uddg=…` redirect wrappers are decoded back to
the real destination URLs.

## License

[MIT](https://github.com/kolisachint/webtools/blob/main/LICENSE)
