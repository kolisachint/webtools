# Product

`webtools` is a unified, token-efficient web `fetch` + `search` CLI for LLM
agents, built around **reference-style URL preservation**. One small, fast binary
that works with no API keys and no backend — and takes them when you want better
search.

## What an LLM gets

Every command returns exactly what an agent needs and nothing it doesn't:

- **Compact content** — anchor text + `[N]` markers instead of inline URLs.
- **Recoverable references** — full URLs in a trailing block, so the agent can
  still cite sources or follow a specific link.
- **An enforced token budget** — `token_estimate` on every result, and a
  `--max-tokens` cap on `fetch` that the whole output honours, references
  included (see [Token budget](#token-budget)).
- **An honest failure signal** — every result carries a `status`, so "no answer"
  and "I was blocked" are never the same outcome (see
  [Knowing when it did not work](#knowing-when-it-did-not-work)).
- **Provenance & metadata** — `source` (what you asked for), `final_url` (where
  it came from after redirects), plus best-effort `title`, `description`,
  `author`, `published`, `lang`, and `site_name` for citations.
- **Right handling per content type** — HTML is extracted; JSON is
  pretty-printed; plain text / Markdown pass through verbatim; binary is
  summarized, never mangled (detected from `Content-Type`, sniffed otherwise
  and surfaced as `media`).
- **Machine-readable mode** — `--json` for structured `FetchResult` /
  `SearchOutput`; `--output structured` for a typed block tree.
- **Native tool-calling** — `webtools mcp` runs an MCP stdio server exposing
  `fetch` and `search` so MCP-aware models can call them directly.
- **Resilience** — transient failures (timeouts, 5xx, 429) retry with backoff.
- **A choice of search backend** — keyless DuckDuckGo by default, or a keyed
  provider for callers who need a real API contract (see
  [Search providers](#search-providers)).

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

### Output formats

- **text** (default) — reference-style plain text. Most token-efficient.
- **markdown** — keeps links inline as `[text](url)` for faithful rendering.
- **structured** — JSON blocks plus a `references` array, for machine parsing.

### Token budget

`--max-tokens` is a cap on the *whole* output, not just the body. The body is
truncated first, and then only the references the surviving text still cites are
kept — dropping a reference nobody mentions costs nothing, and it is usually
enough on its own. If the block still does not fit, references come off the tail
until it does.

This matters most on the pages this tool is pointed at. A documentation index
with 120 links answers `--max-tokens 200` with 200 estimated tokens; reserving
room for the complete reference block and appending it regardless produced
roughly 3300.

Structured output is JSON, so it is never cut mid-string: blocks are dropped
from the end and the document is re-serialized, and the result always parses.

### Knowing when it did not work

An empty answer and a refused one are different facts, and every result says
which it is.

`fetch` reports `status`:

| `status` | Meaning |
|----------|---------|
| `ok` | Content was extracted. |
| `empty` | The document parsed and genuinely holds no text. |
| `needs_js` | An HTML shell with scripts and no text — the page renders its body with JavaScript, which `webtools` does not execute. |
| `too_complex` | Refused before parsing: nesting depth beyond the limit (see [Limits](#limits)). |

`search` reports its own:

| `status` | Meaning |
|----------|---------|
| `ok` | Results were parsed. |
| `empty` | A real results page that reported no hits. |
| `blocked` | A challenge, rate-limit, or unrecognized page. The query was **not** answered. |

A blocked search exits non-zero and is marked `isError` over MCP. Anything less
and a rate-limited search looks exactly like a subject the web has nothing on —
which is how an agent ends up confidently reporting that nothing exists.

### Offline / piped input

`fetch` can convert a local or piped body instead of hitting the network —
handy for testing or post-processing:

```bash
webtools fetch --from-file page.html --url https://site/page   # base for links
curl -s https://api.example.com/data | webtools fetch --from-file - --json
```

## Web search

The same reference-style preservation powers a zero-infrastructure search
layer (`websearch` library / `webtools search` subcommand) that scrapes
DuckDuckGo Lite — no API key, no backend.

```bash
webtools search --query "react 19 release notes"
webtools search --query "rust async" --max-results 8 --json
webtools search --query "open data" --safe-search off
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

### Search providers

DuckDuckGo Lite is the default and needs no key, so `webtools` works with no
configuration at all. It is also scraped HTML: DuckDuckGo rate-limits it hard,
and the markup can change without notice. When search reliability matters, point
`webtools` at a backend with an actual API contract:

| Provider | `--provider` | Credential | Notes |
|----------|--------------|------------|-------|
| DuckDuckGo Lite | `duckduckgo` (default) | none | Scraped HTML; free, and the least reliable. |
| Brave Search | `brave` | `BRAVE_API_KEY` | Independent index, plain JSON. |
| Tavily | `tavily` | `TAVILY_API_KEY` | Returns cleaned page content, so a search often answers without a follow-up `fetch`. |
| SearXNG | `searxng` | `WEBTOOLS_SEARXNG_URL` (+ optional key) | Self-hosted; for networks where the public APIs are unreachable. |

```bash
webtools search --query "rust async runtime" --provider brave
```

Keys are read from the environment or from
[`~/.hoocode/settings.json`](#configuration), never from the command line —
a key in `argv` is visible to every process on the machine. They travel in
request headers, never in a URL, so they cannot leak through error messages,
proxy logs, or server access logs.

Every result records the backend that answered it in `provider`. A configured
fallback is therefore never silent: you can always tell a Brave answer from a
scraped one.

### Configuration

`webtools` reads optional settings from `~/.hoocode/settings.json`
(`$HOOCODE_CONFIG` overrides the path). There is no requirement to have one —
a missing file is not an error.

The file is shared with other tooling, so `webtools` reads only its own
`webtools` key and ignores everything else. Unknown keys inside that section are
ignored too, so a newer config never breaks an older binary.

```json
{
  "webtools": {
    "search": {
      "provider": "brave",
      "fallback": "duckduckgo",
      "providers": {
        "brave":   { "api_key": "..." },
        "tavily":  { "api_key": "..." },
        "searxng": { "base_url": "https://searx.internal" }
      }
    },
    "fetch": {
      "timeout_secs": 20,
      "max_tokens": 4000,
      "ca_certs": ["/etc/ssl/corp-root.pem"]
    }
  }
}
```

Precedence, highest first: **command-line flag → environment variable → config
file → built-in default.** Environment variables are first-class, not an
afterthought: MCP clients launch servers with an `env` block, and containers
often have no home directory to read.

| Setting | Environment variable |
|---------|----------------------|
| Search provider | `WEBTOOLS_SEARCH_PROVIDER` |
| Search fallback | `WEBTOOLS_SEARCH_FALLBACK` (`none` disables) |
| Brave key | `WEBTOOLS_BRAVE_API_KEY`, `BRAVE_API_KEY` |
| Tavily key | `WEBTOOLS_TAVILY_API_KEY`, `TAVILY_API_KEY` |
| SearXNG endpoint | `WEBTOOLS_SEARXNG_URL`, `WEBTOOLS_SEARXNG_API_KEY` |

The file holds credentials, so `webtools` warns when it is readable by other
users — `chmod 600 ~/.hoocode/settings.json`. Keys are never printed: the
`Debug` implementations of every type that holds one redact it.

The libraries do not read this file. `webtools-fetch` and `webtools-search` are
published crates, and a library that reaches into a user's home directory behind
its caller's back is surprising and hard to test — so the binary resolves
everything and passes fully populated option structs down.

## As an MCP server

`webtools mcp` runs a hand-rolled MCP (Model Context Protocol) stdio server,
speaking line-delimited JSON-RPC 2.0. It negotiates protocol versions
`2024-11-05` through `2025-06-18` and exposes two tools — `fetch` (`url`,
`output?`, `max_tokens?`, `timeout?`, `json?`) and `search` (`query`,
`max_results?`, `safe_search?`, `timeout?`, `json?`).

```jsonc
// e.g. in an MCP client config
{ "command": "webtools", "args": ["mcp"] }
```

Three things are worth knowing about the MCP surface specifically:

- **Results are rendered text by default.** Passing `json: true` returns the
  full `FetchResult` / `SearchOutput` instead. The JSON form escapes every
  newline in `content` and repeats the reference list that `content` already
  carries — a real cost when the destination is a context window.
- **`fetch` caps output at 6000 tokens** unless `max_tokens` says otherwise. The
  CLI can afford an unbounded page because a terminal scrolls; an MCP result
  goes straight into a model's context.
- **Requests are served concurrently**, so one slow fetch does not stall the
  others on the connection.

The server takes no command-line flags, so extra CA certificates and a keyed
search provider both come from [the config file](#configuration) or the
environment.

## Security (SSRF guard)

`fetch` is reachable from the CLI and the MCP server, so a crafted or
prompt-injected URL could try to reach internal services. Before connecting,
the guard rejects non-`http(s)` schemes and any host that resolves to a
non-public IP (loopback, private ranges, link-local incl. the cloud metadata
endpoint `169.254.169.254`, CGNAT, ULA, …). The resolved public addresses are
**pinned** for the connection, closing the DNS-rebinding window between
validation and connect. Redirects are followed manually so **every hop is
re-validated and re-pinned**, not just the initial host. The response body is
capped (5 MiB) and read with a running byte limit, so an oversized or malicious
page is bounded before it is ever parsed.

Set `WEBFETCH_ALLOW_PRIVATE=1` to reach internal hosts for trusted internal use
or tests. While it is active the process prints a one-line warning to stderr on
first use — **do not enable it for untrusted input**, as it re-opens SSRF to
loopback, private, and metadata addresses. It relaxes which *hosts* are allowed
and nothing else: only `http` and `https` are ever fetchable, with or without it.

**Behind an HTTP proxy the pinning does not apply.** When `HTTP_PROXY` /
`HTTPS_PROXY` is set, the connection goes to the proxy and the proxy resolves the
host itself, so the addresses this guard pinned are never used. Host validation
still runs before the request, but the DNS-rebinding protection specifically
depends on connecting directly.

## Limits

Two bounds keep a hostile or merely enormous page from becoming a denial of
service:

- **Body size** — 5 MiB, counted after gzip/brotli decoding, so a decompression
  bomb is bounded too. Larger bodies are truncated, not rejected; partial
  content is still useful.
- **Nesting depth** — 10 000 elements. html5ever's tree builder rescans its stack
  of open elements as it inserts, so parse time grows quadratically with depth:
  16 000 nested `<div>`s take 1.7 s, and 200 000 — a 2.2 MB file, comfortably
  inside the body cap — took over four minutes. Neither the body cap nor
  `--timeout` helps, since the cap counts bytes and the timeout covers the HTTP
  request rather than the parse after it. Depth is measured in one linear scan
  before parsing, and a document past the limit comes back as `too_complex`.

`--timeout` bounds each request; the whole fetch, across redirects and retries,
is bounded at three times that.

## TLS, proxies, and custom CAs

`fetch` and `search` build their HTTPS clients to trust, in order:

1. **The OS / system trust store** (`rustls-native-certs`). This is what makes
   requests work behind a **TLS-intercepting proxy** (common on corporate
   networks): the proxy presents a certificate signed by an organization root
   CA that lives in the OS store. Install that root CA in the OS store and
   `webtools` will trust it — no flags needed.
2. **The bundled webpki roots**, used only as a fallback when the OS store
   yields no usable certificates.
3. **`SSL_CERT_FILE`**, if set and readable: its PEM certificates are loaded as
   additional trust anchors (an unreadable value prints a warning and is
   skipped).
4. **`--ca-cert <PATH>`** (repeatable): extra PEM bundles to trust as roots —
   the explicit way to add a proxy's root CA without touching the OS store.

```bash
# Trust a corporate proxy's root CA just for this call
webtools fetch  --url https://docs.internal/api --ca-cert /etc/ssl/corp-root.pem
webtools search --query "internal wiki" --ca-cert /etc/ssl/corp-root.pem

# Or point SSL_CERT_FILE at a bundle
SSL_CERT_FILE=/etc/ssl/corp-root.pem webtools fetch --url https://docs.internal/api
```

If you hit `invalid peer certificate: UnknownIssuer`, the server (or a proxy in
front of it) is presenting a certificate from a CA your trust stores don't know.
Add that CA via the OS store, `SSL_CERT_FILE`, or `--ca-cert` — in that order of
preference.

### `--insecure` (last resort)

`--insecure` disables TLS certificate verification entirely. It is **never the
default**, is strictly opt-in, and prints a loud warning to stderr. Use it only
as a last resort for debugging or a known-trusted endpoint you cannot otherwise
validate — it leaves the connection open to interception. Prefer `--ca-cert`.

```bash
webtools fetch --url https://self-signed.internal --insecure   # warns; unverified
```

## Performance

The conversion path is pure-CPU and allocation-light. Offline latency on the
sample fixtures (release build, `cargo run --release --example latency`):

| Path                       | Latency   | Throughput     |
|----------------------------|-----------|----------------|
| `fetch`  html → text+refs  | ~37 µs/op | ~27k ops/sec   |
| `fetch`  html → markdown   | ~36 µs/op | ~28k ops/sec   |
| `fetch`  html → structured | ~61 µs/op | ~16k ops/sec   |
| `search` ddg-lite → results| ~63 µs/op | ~16k ops/sec   |

The text and markdown paths are about 20% faster than they were, because a
document is now parsed once rather than twice — the title and metadata pass used
to parse it, and then the converter parsed it again. Structured output is slower
than it was, and deliberately: it walks the DOM to type its blocks instead of
splitting flat text into paragraphs.

Real calls are dominated by the remote server's network latency, not our
code. The release binary is ~6.4 MB (LTO + stripped) and starts in single-digit
milliseconds.

## Architecture

A Cargo workspace: shared primitives in a core crate, one library crate per
tool, and a thin root binary that wires them into subcommands.

```
Cargo.toml              Workspace + the webtools binary package
src/
├── main.rs             Unified CLI: fetch / search / mcp subcommands
├── config.rs           ~/.hoocode/settings.json loader (binary only)
└── mcp.rs              MCP stdio server (JSON-RPC over stdin/stdout)
crates/
├── core/               webfetch-core: primitives shared by both tools
│   └── src/
│       ├── compress.rs   Whitespace/decorative reduction + token estimation
│       ├── refs.rs       Referable trait, reference block, budget fitting
│       ├── http.rs       Shared user agent, body cap, retry classification
│       └── tls.rs        Trust anchors: OS store, SSL_CERT_FILE, --ca-cert
├── webfetch/           webfetch: fetch + convert library
│   └── src/
│       ├── lib.rs        Public API (convert_html, convert_body, fetch_and_convert)
│       ├── fetch.rs      HTTP fetch: redirects, retry/backoff, total deadline
│       ├── guard.rs      SSRF guard: scheme, IP classification, DNS pinning
│       ├── limits.rs     Pre-parse nesting-depth bound
│       ├── media.rs      Content-type classification (html/json/text/other)
│       ├── extract.rs    Content-root, title, and citation metadata
│       ├── types.rs      Output structs (FetchResult, ContentStatus, …)
│       └── convert/      Format dispatcher: text | markdown | structured
└── websearch/          websearch: web search library
    └── src/
        ├── lib.rs        Provider dispatch, fallback, reference-style output
        ├── providers/    duckduckgo (default) | brave | tavily | searxng
        ├── extract.rs    DDG parser (uddg decoding) + page classification
        └── types.rs      Search output structs (SearchOutput, SearchStatus, …)
```

Each leaf crate re-exports `webfetch_core::{compress, refs, http, tls}`, so the
shared reference-style logic has a single home but stays reachable as
`webfetch::refs` / `websearch::refs`.

Configuration deliberately lives only in `src/`. The library crates take
fully-populated option structs and never read the environment or the filesystem
on their caller's behalf.
