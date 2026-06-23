# Development Rules

## Repo Map

`webtools` is a Cargo workspace: a thin binary crate at the root plus three
library crates under `crates/`.

Docs (keep these current when behavior changes):

- `README.md` — minimal entry point; links to the docs below
- `docs/product.md` — what it does, features, usage, MCP, architecture
- `docs/install.md` — install, library use, development, releasing
- `ci/README.md` — CI/release workflow reference

Code:

- `src/main.rs` — unified CLI (`fetch`, `search`, `mcp` subcommands)
- `src/mcp.rs` — MCP stdio server (JSON-RPC over stdin/stdout)
- `crates/core` (`webfetch-core`) — shared primitives: `compress.rs`
  (whitespace/token budgeting), `refs.rs` (reference-style URL rendering)
- `crates/webfetch` (`webfetch`) — fetch + convert library: `fetch.rs`,
  `media.rs`, `extract.rs`, `types.rs`, `convert/` (text | markdown | structured)
- `crates/websearch` (`websearch`) — DuckDuckGo Lite search: `lib.rs`,
  `extract.rs`, `types.rs`
- `examples/latency.rs` — offline latency benchmark
- `tests/mcp.rs` — MCP stdio integration test
- `.github/workflows/` — `ci.yml`, `release.yml`
- `.agents/commands/` — slash-command definitions (`pr.md`)

**Dependency / build order**: `webfetch-core` → `webfetch`, `websearch` →
`webtools` (root binary). Both leaf crates depend only on `webfetch-core`.

## Conversational Style

- Keep answers short and concise
- No emojis in commits, issues, PR comments, or code
- No fluff or cheerful filler text
- Technical prose only, be kind but direct

## Code Quality

- Read files in full before making wide-ranging changes, before editing files
  you have not already fully inspected, and when asked to investigate or audit.
  Do not rely only on search snippets for broad changes.
- Match the surrounding style: import order, naming, error handling (`anyhow`
  for the binary, typed results in libraries)
- Keep the shared reference-style logic in `webfetch-core`; do not duplicate it
  in the leaf crates — re-export via `webfetch::refs` / `websearch::refs`
- Avoid `unwrap()`/`expect()` outside tests; thread errors with `?`
- Do not preserve backward compatibility unless the user explicitly asks
- Always ask before removing functionality that appears intentional

## Commands

- After code changes (not doc-only changes), run all three and fix everything
  before committing:
  ```bash
  cargo fmt --all --check
  cargo clippy --workspace --all-targets -- -D warnings
  cargo test --workspace
  ```
- Offline benchmark: `cargo run --release --example latency`
- If you create or modify a test, run it and iterate until it passes
- NEVER commit unless the user asks

## Slash Commands

- `/pr [patch|minor|major]` — opens a release PR on a feature branch and labels
  it `cargo:<bump>` so `release.yml` bumps the version, publishes, and builds on merge.
  Defined in `.agents/commands/pr.md`. Defaults to `patch`.
- Slash-command definitions live in `.agents/commands/`.

## Releasing

**Lockstep versioning**: all crates share one version. Every release bumps the
root `Cargo.toml`, all `crates/*/Cargo.toml`, and the internal
`path + version` dependency requirements together.

**Version semantics**:

- `patch` — bug fixes and additions
- `minor` — API changes
- `major` — large breaking changes

### Flow (do NOT bump versions or tag by hand)

**Never edit `version = "…"` in any `Cargo.toml` (root, `crates/*`, or the
internal `path + version` deps) inside a feature PR.** The release workflow is
the sole owner of the version: it computes the next version from the latest
`v*` git tag plus the PR's `cargo:<bump>` label, then rewrites every manifest.
A manual bump is at best ignored and at worst confusing — historically it
caused a skipped number (a PR bumped to 0.1.12, the workflow then released
0.1.13). Leave versions untouched and just apply the label.

1. `/pr <bump>` opens a PR labeled `cargo:<bump>`.
2. On merge, `release.yml` derives the next version from the latest `v*` tag,
   bumps every manifest, updates `Cargo.lock`, commits `release: v<version>`,
   tags `v<version>`, and pushes `main`.
3. The tag triggers `release.yml`, which publishes the libraries to crates.io
   (in dependency order, skipping versions already on the index) and then
   builds + attaches Linux (`x86_64-unknown-linux-gnu`) and macOS
   (`aarch64-apple-darwin`) binaries to the GitHub release.

Secrets required: `CRATES_IO_TOKEN` (crates.io publish). `GITHUB_TOKEN` is
provided automatically.

Manual fallback (only if asked): `git tag vX.Y.Z && git push origin vX.Y.Z`.

## **CRITICAL** Git Rules for Parallel Agents **CRITICAL**

Multiple agents may work on different files in the same worktree simultaneously.

### Committing

- ONLY commit files YOU changed in THIS session
- Include `fixes #<number>` / `closes #<number>` when there is a related issue/PR
- NEVER use `git add -A` or `git add .` — these sweep up other agents' changes
- ALWAYS `git add <specific-file-paths>` listing only files you modified
- Run `git status` before committing and verify you are staging only YOUR files

### Forbidden Git Operations

These can destroy other agents' work and are never allowed:

- `git reset --hard`
- `git checkout .`
- `git clean -fd`
- `git stash`
- `git add -A` / `git add .`
- `git commit --no-verify`

### Safe Workflow

```bash
git status                      # 1. check first
git add crates/webfetch/src/fetch.rs  # 2. stage only your files
git commit -m "fix(webfetch): ..."    # 3. commit
git pull --rebase && git push         # 4. push (never reset/checkout)
```

### If Rebase Conflicts Occur

- Resolve conflicts in YOUR files only
- If a conflict is in a file you did not modify, abort and ask the user
- NEVER force push over shared history

### User Override

If the user's instructions conflict with these rules, ask for confirmation that
they want to override. Only then proceed.
