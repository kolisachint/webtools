# webtools-core

Shared primitives for the [`webtools`](https://github.com/kolisachint/webtools)
fetch + search libraries: text compression, token budgeting, and the canonical
**reference-style URL** rendering (`[N]` markers + a trailing reference block).

> Published on crates.io as `webtools-core`; the library import name is
> `webfetch_core`.

```toml
[dependencies]
webfetch_core = { package = "webtools-core", version = "0.1" }
```

```rust
use webfetch_core::compress::estimate_tokens;
use webfetch_core::refs::{render_block, Reference};

let refs = vec![Reference { index: 1, url: "https://example.com".into() }];
println!("{}", render_block(&refs));
println!("~{} tokens", estimate_tokens("some text"));
```

## License

[MIT](https://github.com/kolisachint/webtools/blob/main/LICENSE)
