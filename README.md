# webtools

A unified, **token-efficient** web `fetch` + `search` CLI for LLM agents,
built around **reference-style URL preservation**. One small, blazing-fast
binary; no API keys, no backend.

```bash
webtools fetch  --url https://docs.example.com/api   # page → compact text + refs
webtools search --query "rust async runtime"          # web search → results + refs
```

## What an LLM gets

Every command returns exactly what an agent needs and nothing it doesn't:

- **Compact content** — anchor text + `[N]` markers instead of inline URLs.
- **Recoverable references** — full URLs in a trailing block, so the agent can
  still cite sources or follow a specific link.
- **A token budget signal** — `token_estimate` on every result, plus a
  `--max-tokens` cap on `fetch`.
- **Provenance** — `final_url` (post-redirect) and `source` on fetches.
- **Machine-readable mode** — `--json` for structured `FetchResult` /
  `SearchOutput`; `--output structured` for a typed block tree.

## Performance

The conversion path is pure-CPU and allocation-light. Offline latency on the
sample fixtures (release build, `cargo run --release --example latency`):

| Path                       | Latency   | Throughput     |
|----------------------------|-----------|----------------|
| `fetch`  html → text+refs  | ~47 µs/op | ~21k ops/sec   |
| `fetch`  html → markdown   | ~45 µs/op | ~22k ops/sec   |
| `fetch`  html → structured | ~47 µs/op | ~21k ops/sec   |
| `search` ddg-lite → results| ~63 µs/op | ~16k ops/sec   |

Real calls are dominated by the remote server's network latency, not our
code. The release binary is ~6.6 MB (LTO + stripped) and starts in single-digit
milliseconds.

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
webtools search --query "react 19 release notes"
webtools search --query "rust async" --max-results 8 --json
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

A single binary, `webtools`, exposes both tools as subcommands:

```bash
# Plain text with a reference block
webtools fetch --url https://docs.example.com/api

# Markdown
webtools fetch --url https://example.com/post --output markdown

# Full structured result as JSON
webtools fetch --url https://example.com --output structured --json

# Cap output size (estimated tokens)
webtools fetch --url https://example.com --max-tokens 2000
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

A Cargo workspace: shared primitives in a core crate, one library crate per
tool, and a thin root binary that wires them into subcommands.

```
Cargo.toml              Workspace + the webfetch-tools binary package
src/main.rs             Unified CLI: `webfetch` / `websearch` subcommands
crates/
├── core/               webfetch-core: primitives shared by both tools
│   └── src/
│       ├── compress.rs   Whitespace/decorative reduction + token budgeting
│       └── refs.rs       Referable trait + canonical reference-block renderer
├── webfetch/           webfetch: fetch + convert library
│   └── src/
│       ├── lib.rs        Public API (convert_html, fetch_and_convert)
│       ├── fetch.rs      HTTP fetch + redirect policy (reqwest)
│       ├── extract.rs    Content-root + title heuristics
│       ├── types.rs      Output structs
│       └── convert/      Format dispatcher: text | markdown | structured
└── websearch/          websearch: DuckDuckGo Lite search library
    └── src/
        ├── lib.rs        DDG Lite fetch + reference-style output
        ├── extract.rs    DOM → SearchResult parser (uddg decoding)
        └── types.rs      Search output structs
```

Each leaf crate re-exports `webfetch_core::{compress, refs}`, so the shared
reference-style logic has a single home but stays reachable as
`webfetch::refs` / `websearch::refs`.

## Install

Grab a prebuilt binary from the [Releases](../../releases) page, or build from
source:

```bash
cargo build --release --bin webtools
# binary at target/release/webtools
```

Tagging a `v*` release (e.g. `git tag v0.1.0 && git push origin v0.1.0`)
triggers the release workflow, which builds and attaches Linux and macOS
binaries.

## Development

```bash
cargo build --workspace
cargo test --workspace
cargo clippy --workspace --all-targets
cargo run --release --example latency   # offline latency benchmark
```
