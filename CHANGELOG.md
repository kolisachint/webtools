# Changelog

All notable changes to this project are documented here. The format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project uses
lockstep semantic versioning across all crates.

> Version numbers are owned by the release workflow (it derives the next version
> from the latest `v*` tag plus the PR's `cargo:<bump>` label and rewrites every
> manifest on merge). Entries land under **Unreleased** and are stamped with the
> released version by that workflow — see `AGENTS.md` → Releasing.

## [Unreleased]

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
