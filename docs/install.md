# Install & Development

## Install the CLI

Grab a prebuilt binary from the [Releases](../../releases) page, or build from
source:

```bash
cargo build --release --bin webtools
# binary at target/release/webtools
```

### Release assets

Each tagged release attaches one archive per platform, a per-archive
`.sha256`, and a combined `SHA256SUMS` manifest. Archive names follow
`webtools-<target>.<ext>` (`.tar.gz` on Unix, `.zip` on Windows):

| OS | Arch | Target triple | Asset |
|----|------|---------------|-------|
| Linux (glibc)  | x86_64  | `x86_64-unknown-linux-gnu`    | `webtools-x86_64-unknown-linux-gnu.tar.gz` |
| Linux (musl)   | x86_64  | `x86_64-unknown-linux-musl`   | `webtools-x86_64-unknown-linux-musl.tar.gz` |
| Linux (musl)   | aarch64 | `aarch64-unknown-linux-musl`  | `webtools-aarch64-unknown-linux-musl.tar.gz` |
| macOS          | x86_64  | `x86_64-apple-darwin`         | `webtools-x86_64-apple-darwin.tar.gz` |
| macOS          | aarch64 | `aarch64-apple-darwin`        | `webtools-aarch64-apple-darwin.tar.gz` |
| Windows        | x86_64  | `x86_64-pc-windows-msvc`      | `webtools-x86_64-pc-windows-msvc.zip` |

### Verify the download

```bash
# Resolve the latest tag rather than pinning one here: a hard-coded version in
# this file goes stale on the very next release.
TAG=$(curl -fsSL https://api.github.com/repos/kolisachint/webtools/releases/latest \
      | sed -n 's/.*"tag_name": *"\([^"]*\)".*/\1/p')
ASSET=webtools-x86_64-unknown-linux-gnu.tar.gz
base="https://github.com/kolisachint/webtools/releases/download/$TAG"
curl -fsSLO "$base/$ASSET"
curl -fsSLO "$base/SHA256SUMS"
sha256sum --check --ignore-missing SHA256SUMS   # expect: $ASSET: OK
```

### Programmatic install (ensureTool)

A fetcher can resolve the asset deterministically from the running platform and
verify it against the manifest:

1. Map host OS/arch to a target triple (table above); pick `.zip` for Windows,
   else `.tar.gz`. On Linux prefer the `musl` asset for a static, distro-
   independent binary.
2. Download `webtools-<target>.<ext>` and `SHA256SUMS` from the release for the
   pinned tag.
3. Look up the asset's line in `SHA256SUMS` (format: `<sha256>  <filename>`)
   and compare against the SHA-256 of the downloaded archive before extracting.
4. Extract; the binary is named `webtools` (`webtools.exe` on Windows).

The per-archive `<asset>.sha256` files are also attached if verifying a single
asset without the combined manifest.

## Use as a library

The libraries are published on crates.io as `webtools-fetch` and
`webtools-search` (with shared primitives in `webtools-core`). Their library
import paths remain `webfetch` and `websearch`:

```toml
[dependencies]
webfetch = { package = "webtools-fetch", version = "0.2" }
websearch = { package = "webtools-search", version = "0.2" }
```

Or pull straight from git:

```toml
[dependencies]
webfetch = { package = "webtools-fetch", git = "https://github.com/kolisachint/webtools" }
websearch = { package = "webtools-search", git = "https://github.com/kolisachint/webtools" }
```

```rust
use webfetch::types::{ContentType, FetchOptions};
use websearch::types::SearchOptions;

// ── Fetch: convert HTML without network I/O ──────────────────────────
let opts = FetchOptions {
    content_type: ContentType::Text,
    ..Default::default()
};
let result = webfetch::convert_html(html, "https://example.com/page", &opts);

// Access the compact content and recover URLs:
println!("{}", result.content);          // text with [N] markers
for r in &result.references {
    println!("[{}] {}", r.index, r.url); // recover full URLs
}
println!("~{} tokens", result.token_estimate);

// ── Fetch: network request with retry/backoff ────────────────────────
let result = webfetch::fetch_and_convert(FetchOptions {
    url: "https://docs.example.com/api".into(),
    ..opts
}).await?;

// ── Search: keyless DuckDuckGo by default ───────────────────────────
let search_opts = SearchOptions {
    query: "rust async runtime".into(),
    max_results: Some(5),
    // Or a keyed backend. The libraries never read a config file or the
    // environment — the caller resolves credentials and passes them in.
    // provider: websearch::Provider::Brave { api_key },
    ..Default::default()
};
let output = websearch::run_search(search_opts).await?;
if output.status.is_failure() {
    // Blocked or unparseable. Distinct from "no hits", which is `Empty`.
    eprintln!("search did not answer: {:?}", output.status);
}
for hit in &output.results {
    println!("{} [{}]", hit.title, hit.ref_index);
}
```

## Development

```bash
cargo build --workspace
cargo test --workspace
cargo clippy --workspace --all-targets
cargo run --release --example latency   # offline latency benchmark
```

Before committing code changes, run all three checks and fix every error,
warning, and info:

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

## Releasing

Releases are label-driven, and a single workflow does all of it. Open a PR with
the `/pr <patch|minor|major>` command (see `.agents/commands/pr.md`); merging a
PR labeled `cargo:<bump>` runs `.github/workflows/release.yml`, which derives
the next version from the latest `v*` tag, rewrites every manifest, stamps the
changelog, tags, publishes the libraries to crates.io, and attaches binaries for
all six targets. See [`ci/README.md`](../ci/README.md) for details.

**Pushing a tag by hand does not release anything.** `release.yml` triggers on a
merged pull request, not on a tag push — an earlier split design triggered on
tags, and the documentation outlived it. To release without a code change, open
a trivial PR and label it, or run the workflow from the Actions tab.

### Development checks

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
```

CI runs these on Linux, compiles the workspace on macOS and Windows (both are
release targets), and builds once on the declared `rust-version` so the MSRV
stays honest.
