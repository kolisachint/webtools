# webtools-fetch

Token-efficient web content fetcher with **reference-style URL preservation**,
part of [`webtools`](https://github.com/kolisachint/webtools). Instead of
stripping links to their domain (losing the source) or expanding full URLs
inline (wasting tokens), links become compact `[N]` markers collected into a
recoverable reference block.

> Published on crates.io as `webtools-fetch`; the library import name is
> `webfetch`.

```toml
[dependencies]
webfetch = { package = "webtools-fetch", version = "0.1" }
```

```rust
use webfetch::types::{ContentType, FetchOptions};

// Offline: convert HTML without network I/O.
let opts = FetchOptions { content_type: ContentType::Text, ..Default::default() };
let result = webfetch::convert_html(html, "https://example.com/page", &opts);
println!("{}", result.content);          // text with [N] markers
for r in &result.references {
    println!("[{}] {}", r.index, r.url); // recover full URLs
}

// Network: fetch + convert with retry/backoff.
let result = webfetch::fetch_and_convert(FetchOptions {
    url: "https://docs.example.com/api".into(),
    ..opts
}).await?;
```

Output formats: `text` (reference-style, default), `markdown` (inline links),
`structured` (JSON blocks + `references` array).

## License

[MIT](https://github.com/kolisachint/webtools/blob/main/LICENSE)
