# webfetch

A token-efficient web content fetcher with **reference-style URL preservation**.

## The problem

Most "clean text" extractors either strip links down to their domain
(`example.com`) — losing the ability to cite a source or follow a specific
link — or leave full URLs inline, where each one burns 10+ tokens.

`webfetch` uses a third strategy: it keeps the anchor text and appends a
compact `[N]` marker, then collects the full URLs into a reference list. The
agent sees `[1]` inline (≈1 token) but can still recover the exact URL.

| Approach        | Inline cost          | URL access  |
|-----------------|----------------------|-------------|
| Strip to domain | `example.com`        | Lost        |
| Full URL inline | `https://…` (10+ tok)| Immediate   |
| **Reference**   | `[1]` (~1 tok)       | Recoverable |

### Example

Input HTML linking to an API endpoint and an auth flow produces:

```
See the users endpoint [1] for details. Authentication uses OAuth2 [2].

References:
[1] https://docs.example.com/api/v2/users
[2] https://auth.example.com/oauth2
```

Repeated links collapse to a single reference — the same URL always reuses
its first index.

## Web search

The same reference-style preservation powers a zero-infrastructure search
layer (`websearch` binary / `webfetch::search` module) that scrapes
DuckDuckGo Lite — no API key, no backend.

```bash
websearch --query "react 19 release notes"
websearch --query "rust async" --max-results 8 --json
```

Output keeps titles + snippets inline with `[N]` markers and collects the
URLs into a reference block:

```
React 19 – React [1]
React 19 introduces the new use hook for data fetching and more APIs.

Partial Prerendering – Next.js [2]
The Next.js App Router now supports partial prerendering.

References:
[1] https://react.dev/blog/2024/12/01/react-19
[2] https://nextjs.org/blog/partial-prerendering
```

DDG Lite's `//duckduckgo.com/l/?uddg=…` redirect wrappers are decoded back to
the real destination URLs.

## Output formats

- **text** (default) — reference-style plain text. Most token-efficient.
- **markdown** — keeps links inline as `[text](url)` for faithful rendering.
- **structured** — JSON blocks plus a `references` array, for machine parsing.

## Usage

```bash
# Plain text with a reference block
webfetch --url https://docs.example.com/api

# Markdown
webfetch --url https://example.com/post --output markdown

# Full structured result as JSON
webfetch --url https://example.com --output structured --json

# Cap output size (estimated tokens)
webfetch --url https://example.com --max-tokens 2000
```

## Library

```rust
use webfetch::types::{ContentType, FetchOptions};

let opts = FetchOptions { content_type: ContentType::Text, ..Default::default() };

// Offline conversion (no network):
let result = webfetch::convert_html(html, "https://example.com/page", &opts);
for r in &result.references {
    println!("[{}] {}", r.index, r.url);
}

// Or fetch + convert:
// let result = webfetch::fetch_and_convert(FetchOptions { url: "...".into(), ..opts }).await?;
```

## Architecture

```
src/
├── main.rs        webfetch CLI entry
├── bin/
│   └── websearch.rs  websearch CLI entry
├── lib.rs         Public API (convert_html, fetch_and_convert)
├── fetch.rs       HTTP fetch + redirect policy (reqwest)
├── extract.rs     Content-root + title heuristics
├── convert/
│   ├── mod.rs     Format dispatcher
│   ├── text.rs    Reference-style URL collection
│   ├── markdown.rs Inline-link markdown
│   └── structured.rs JSON blocks
├── search/
│   ├── mod.rs     DDG Lite fetch + reference-style output
│   ├── extract.rs DOM → SearchResult parser (uddg decoding)
│   └── types.rs   Search output structs
├── compress.rs    Whitespace/decorative reduction + token budgeting
└── types.rs       Output structs
```

## Development

```bash
cargo build
cargo test
cargo clippy --all-targets
```
