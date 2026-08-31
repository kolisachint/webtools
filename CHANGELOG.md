# Changelog

All notable changes to this project are documented here. The format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project uses
lockstep semantic versioning across all crates.

> Version numbers are owned by the release workflow (it derives the next version
> from the latest `v*` tag plus the PR's `cargo:<bump>` label and rewrites every
> manifest on merge). Entries land under **Unreleased** and are stamped with the
> released version by that workflow — see `AGENTS.md` → Releasing.

## [Unreleased]

### Added

- **`fetch --grep <pattern>` finds where a page mentions something.** Paging
  reads a document in order and `--outline` maps it by heading, but neither
  answers "where does this mention rate limiting" on a page whose headings do
  not say so, or that has no headings at all. `--grep` (MCP: `grep`) reports
  each location with its offset, the text around it, and the section it falls
  in — and those offsets are the ones `--offset` reads, so a hit is followed by
  fetching it.

  The pattern is a regular expression, case-insensitive unless it carries an
  uppercase letter: smart case, so case-sensitivity needs no separate flag.
  Matching is linear in the input — this engine does not backtrack — and the
  compiled pattern is size-bounded, so neither a hostile pattern nor a hostile
  page can blow it up. An unusable pattern is reported with the part the engine
  rejected rather than silently returning the page.

  Occurrences closer together than a snippet collapse into the first with a
  count of the rest, since a term repeated through one paragraph is one
  location and a row per occurrence spends many tokens restating it; a section
  boundary ends a neighbourhood however close the occurrences sit. Hits honour
  the token budget as an outline does, dropping whole hits and saying how many,
  because a search that silently reports three of forty reads as absence of
  evidence.

  `--grep` and `--outline` are two views of one page and cannot be combined. In
  JSON the hits are a `matches` array, absent from a fetch that did not ask.

- **A long page can be read to the end.** Every fetch now reports the document
  it is a slice of and where to resume — `offset`, `next_offset`, `total_bytes`,
  `total_token_estimate` and `truncated` in JSON, and a footer in rendered
  output:

  ```
  [showing bytes 0-178 of 3499 (~49 of ~894 tokens); continue with --offset 178]
  ```

  `fetch --offset N` (MCP: `offset`) starts the next window there. Before this,
  a budget made a long document a dead end: output stopped at the cap with a
  bare elision marker, saying neither how much was missing nor how to reach it,
  and the only route to the rest was re-fetching the whole page at a larger cap.

  Windows tile the document exactly — every byte once, in order — because the
  resume point is the consumption the truncation actually made rather than a
  byte position re-derived from a token count, which drifts in both directions.
  The footer is absent on the window that reaches the end, so its absence is the
  signal that the read is complete.

  Cuts snap back to the nearest paragraph, line or word boundary within reach,
  so windows neither end nor begin mid-word; text with no break in reach is
  still cut hard, and a budget too small even for the elision marker still
  advances one character rather than returning a window that consumed nothing.

- **Fetched pages are cached, so paging a document downloads it once.** Windows
  are served from an on-disk cache keyed by requested URL, which also means
  every window of a read sees the same snapshot — without it, offsets from one
  response can address text a changed page no longer has.

  Raw pages are cached rather than converted output, so one entry serves every
  `--output` format and every offset of the same document. Entries live under
  `$XDG_CACHE_HOME/webtools/fetch` (`~/.cache/...`, or `~/Library/Caches/...` on
  macOS), owner-only, for 15 minutes; `WEBTOOLS_CACHE_DIR`,
  `webtools.fetch.cache_ttl_secs` / `WEBTOOLS_CACHE_TTL`, and `--no-cache` /
  `WEBTOOLS_NO_CACHE` control location, freshness and opt-out. Writes are
  atomic, expired and excess entries are pruned, and every operation is
  best-effort: a cache that cannot be read or written degrades to no cache, not
  to a failed fetch.

  The cache lives in the binary, like config: the published libraries still
  never touch a caller's filesystem. `webfetch::convert_page` is the new seam —
  it converts a page a caller already holds exactly as a live fetch would.

- **`fetch --outline` maps a long page instead of reading it.** Paging made a
  long document readable in sequence, which is the wrong shape when the answer
  is in one section and the rest is overhead. `--outline` (MCP: `outline`)
  returns every heading with the offset that reads its section and what that
  section costs, so a page is mapped for a few dozen tokens and then read one
  section at a time.

  Outline offsets *are* paging offsets — there is no second addressing scheme to
  drift out of step. Headings are located in the finished extracted text rather
  than recorded during conversion, since whitespace compression and
  duplicate-title stripping run afterwards and would shift an offset captured
  mid-walk; a heading that cannot be found there is skipped rather than guessed
  at, because a wrong offset puts a section boundary in the wrong place.

  The outline honours `--max-tokens`, dropping whole rows from the tail rather
  than cutting one mid-offset, and the count of what it dropped survives even a
  budget too small to hold it. In JSON it is an `outline` array of
  `{level, title, offset, bytes, token_estimate}`, absent from any fetch that
  did not ask for one.

### Fixed

- **CJK pages came back as mojibake.** Shift_JIS, GBK, GB18030, Big5, EUC-KR,
  EUC-JP and ISO-2022-JP are now decoded exactly, along with the rest of the
  WHATWG Encoding Standard, via `encoding_rs`. 0.2.0 decoded only UTF-8 and the
  single-byte Western family and reported everything else as undecodable, to
  avoid the conversion tables; measured, they cost 0.17 MB of binary against
  garbling most of the non-Latin web. Label lookup follows the WHATWG aliasing
  rules, so what pages actually declare (`latin1`, `sjis`, `x-gbk`,
  `windows-949`, …) resolves.
- `--from-file` ignored `<meta charset>` and read every local file as UTF-8, so
  the offline path mangled pages the network path handled correctly.

### Breaking

- `webfetch::types::FetchOptions` gains `grep`, and `FetchResult` gains a
  `matches` vector of `webfetch::grep::Match`. Exhaustive struct literals need
  them; `..Default::default()` is unaffected.
- `webfetch::types::FetchOptions` gains `outline`, and `FetchResult` gains an
  `outline` vector of `webfetch::outline::Section`. Exhaustive struct literals
  need them; `..Default::default()` is unaffected.
- `webfetch_core::refs::fit_to_budget` returns a `Fitted { content, kept,
  body_consumed }` instead of a `(String, Vec<usize>)`. The consumption is the
  new field: it is where the body was actually cut, and paging cannot be exact
  without it.
- `webfetch::fetch::FetchedPage` now derives `Serialize`/`Deserialize`, so a
  caller can persist a fetched page (the CLI cache does).
- `webfetch::types::FetchOptions` gains `offset`, and `FetchResult` gains
  `total_token_estimate`, `total_bytes`, `offset`, `next_offset` and
  `truncated`. Callers building either struct exhaustively need the new fields;
  `..Default::default()` is unaffected.
- `webfetch_core::charset::Charset` replaces its `Cp1252` and `Unsupported`
  variants with `Supported(String)` (carrying the encoding's canonical name)
  and `Unknown(String)`. It is now `#[non_exhaustive]`, so the recognized set
  can grow without breaking callers again.
- `webfetch_core::charset::decode_cp1252` is removed; `decode` handles it.

## [0.2.0] - 2026-08-09

Supersedes the yanked 0.1.17, which shipped this same work under a patch
version by mistake.

### Breaking

Every library consumer needs a look at this list. Structs gained public fields,
so struct-literal construction of them no longer compiles.

- `websearch::types::SearchOptions` gained `provider` and `fallback`.
- `websearch::types::SearchOutput` gained `status` and `provider`.
- `websearch::build_output` now takes parsed results, a status and a provider
  label. The old "parse a DuckDuckGo page" behaviour is `build_output_from_ddg`.
- `websearch::fetch_ddg_lite` moved to `websearch::providers::ddg::fetch_lite`.
- `webfetch::types::FetchResult` gained `status`; `Metadata` gained `charset`.
- `webfetch::fetch::FetchedPage` gained `undecodable_charset`.
- `webfetch::convert::convert` no longer appends the `References:` block to
  `content` — which references survive depends on the token budget, so the
  pipeline assembles it. Use `webfetch::convert_body` for the assembled form,
  or `refs::fit_to_budget` directly.
- `webfetch::convert::structured::Block` gained `level`, and `BlockKind` gained
  `Heading`, `ListItem`, `Code`, `Quote` and `TableRow` alongside `Paragraph`.
- `webfetch_core::compress::truncate_preserving_refs` is removed. Its job — fit
  a body and its references inside a budget — is `refs::fit_to_budget`, which
  actually holds the budget.
- `webfetch_core::compress::truncate_to_tokens` still has its signature but
  different behaviour: it now charges the same per-byte cost `estimate_tokens`
  does, so its result honours the budget on punctuation-dense text.

### Fixed

- **Search snippets could describe the wrong page.** Results and snippets were
  read from two independent iterators and zipped by position, so a result with
  no snippet row shifted every following snippet onto the wrong URL. Snippets
  now attach to the link they follow in document order.
- **A blocked search looked like a search with no results.** DuckDuckGo serves
  its bot challenge with HTTP 200 and no result rows, which came back as
  `result_count: 0` and exit 0 — indistinguishable from a query nothing matches.
  Search now reports `status` (`ok` / `empty` / `blocked`); a blocked search
  exits non-zero and is an `isError` over MCP.
- **`--max-tokens` did not bound the output.** Room was reserved for the whole
  reference block and the block appended regardless, so a 120-link page answered
  a 200-token cap with roughly 3300 estimated tokens. The body is now truncated
  first and only the references it still cites are kept. `--output structured`
  drops blocks and re-serializes instead of cutting JSON mid-string, so the
  result always parses.
- **Table cells ran together.** `<th>Name</th><th>Type</th>` rendered as
  `NameType`; cells in a row are now separated.
- **An empty extraction was reported as success.** `fetch` now reports `status`
  (`ok` / `empty` / `needs_js` / `too_complex`), so a JavaScript-rendered shell
  is distinguishable from a page that genuinely has no text.
- **`source` and `final_url` were always identical**, both holding the
  post-redirect URL. `source` is now the URL that was requested.
- **A deeply nested document could hang the process.** html5ever's tree builder
  is quadratic in nesting depth: a 2.2 MB file inside the 5 MiB body cap took
  over four minutes to parse, stalling the whole MCP server. Depth is now
  measured before parsing and a document past 10 000 levels is refused.
- `--timeout` bounded a single request while retries and redirects could keep a
  fetch running for minutes; the whole fetch is now bounded at three times it.
- The `javascript:` and `mailto:` links the text path drops were still emitted
  as live links by the markdown path.
- **Non-UTF-8 pages came back as replacement characters.** Bodies are now
  decoded with the charset the response declares (`Content-Type`, falling back
  to `<meta charset>`). UTF-8 and the single-byte Western family
  (`windows-1252`, `ISO-8859-1`) are decoded exactly. Multi-byte legacy
  encodings still are not — the conversion tables would add roughly a megabyte
  to the binary — but the charset is reported on `metadata.charset` and warned
  about, so the cause is visible.
- `--from-file` failed outright on a non-UTF-8 file; it now reads lossily.
- `WEBFETCH_ALLOW_PRIVATE` disabled the URL scheme check along with the address
  check. It now relaxes which hosts are reachable and nothing else.
- The search path read response bodies without any size cap, unlike fetch.

### Added

- **Pluggable search providers**: Brave, Tavily, and SearXNG alongside the
  keyless DuckDuckGo default. API keys travel in request headers only, never in
  a URL, and every type holding one redacts it from `Debug`.
- **Optional configuration** from `~/.hoocode/settings.json` (`$HOOCODE_CONFIG`
  overrides the path), covering the search provider and its credentials, fetch
  defaults, and extra CA certificates. Precedence is flag → environment →
  file → default. This is also the only way to give the MCP server extra trust
  anchors, since it takes no flags. Only the `webtools` section is read and
  unknown keys are ignored, so a file shared with other tooling still loads.
  The published libraries do not read it — the binary resolves everything and
  passes populated option structs down.
- `SearchOutput.provider` records which backend answered, so a fallback from a
  keyed provider to scraped DuckDuckGo is never silent.
- `--provider` on `webtools search`.
- Structured output carries real block kinds (heading with level, paragraph,
  list item, code, quote, table row) instead of labelling everything a
  paragraph, and `--output markdown` now populates `references`.
- CI compiles the workspace on macOS and Windows (both are release targets) and
  builds on the declared `rust-version`, now set to 1.82.

### Changed

- The MCP server handles requests concurrently, up to eight calls at a time;
  one slow fetch previously blocked every other call on the connection.
- MCP tool results are rendered text by default rather than a pretty-printed
  `FetchResult` with escaped newlines and a duplicated reference list. Pass
  `json: true` for the full structure.
- MCP `fetch` defaults to a 6000-token cap, since its output goes straight into
  a model's context.
- The MCP server negotiates protocol versions `2024-11-05` through `2025-06-18`
  instead of always answering `2024-11-05`.
- Both paths send the same browser user agent. `fetch` previously identified
  itself as `webfetch/<version>`, which CDNs refuse far more often.
- HTML documents are parsed once instead of twice, making the text and markdown
  paths about 20% faster. The OS trust store is cached rather than reloaded on
  every redirect hop, and selectors are compiled once.

### Security

- The bundled webpki roots are now always trusted alongside the OS store, not
  only when the OS store comes back empty. A *partial* system store — a slim
  container image with a handful of certificates — previously replaced a
  complete root set with an incomplete one and could not verify most of the
  public web. (v0.1.14 described the trust as additive; the code made webpki a
  fallback.)
- The SSRF guard refuses ports that never speak HTTP (ssh, smtp, mysql, redis
  and the rest of the list browsers refuse). HTTP on 8080, 3000 or 8443 is
  unaffected.
- IPv6 transition addresses that embed an IPv4 one — NAT64 `64:ff9b::/96` and
  6to4 `2002::/16` — are classified by the address they carry. They were a way
  to name `169.254.169.254` without writing it down.
- Documented that IP pinning does not apply behind an HTTP proxy: the proxy
  resolves the host itself, so the DNS-rebinding protection depends on
  connecting directly.

### Tests

- The HTTP path has coverage for the first time, against a local socket:
  transient retries, non-retry of client errors, redirect following and
  re-validation, the redirect cap, the body cap, charset decoding, provenance
  across a redirect, and the total deadline.
- CLI integration tests for the offline fetch path, the token budget, every
  output format, status reporting, the config file and its precedence, and exit
  codes.
- The MCP server's concurrency is verified by timing: three 900 ms fetches
  complete in about one, where the old sequential loop would take three.

## [0.1.17] - 2026-08-09 [YANKED]

Yanked from crates.io. It carried the breaking API changes now listed under
0.2.0, but was released as a patch: Cargo treats `0.1.16 -> 0.1.17` as
compatible, so anything depending on `webtools-fetch = "0.1"` would have
resolved to it and failed to compile. Use 0.2.0 — the contents are identical.

## [0.1.16] - 2026-06-26

### Changed

- Dropped the `aarch64-unknown-linux-gnu` release target; the musl aarch64
  build covers that platform.

## [0.1.15] - 2026-06-26

### Fixed

- Stale version references in `docs/install.md`.

## [0.1.14] - 2026-06-23

### Added

- TLS trust now uses the operating system certificate store (via
  `rustls-native-certs`) in addition to the bundled webpki roots, so requests
  succeed behind TLS-intercepting proxies whose organization root CA lives in
  the OS store. The bundled webpki roots remain trusted as well.
- `SSL_CERT_FILE` is honored: when set and readable, its PEM certificates are
  loaded as additional trust anchors (an unreadable value warns and is skipped).
- `fetch` and `search` gained `--ca-cert <PATH>` (repeatable) to add extra PEM
  trust anchors, e.g. a corporate proxy's root CA that is not in the OS store.
- `fetch` and `search` gained `--insecure` to disable TLS certificate
  verification. It is strictly opt-in, never the default, prints a loud warning,
  and is documented as a last resort only.

### Fixed

- Requests no longer fail with `invalid peer certificate: UnknownIssuer` behind
  TLS-intercepting proxies, because the client previously trusted only the
  bundled webpki roots and ignored the OS trust store and `SSL_CERT_FILE`.
