# CI / Release workflows

The GitHub Actions workflows live in `.github/workflows/` (`ci.yml` and
`release.yml`). There is no manual activation step.

## Workflow details

### `ci.yml`

Runs on pushes to `main` and on PRs:
- `cargo fmt --all --check` — formatting
- `cargo clippy --workspace --all-targets -- -D warnings` — lints
- `cargo test --workspace` — all tests

### `release.yml`

A single workflow triggered when a PR with a `cargo:patch`, `cargo:minor`, or
`cargo:major` label is merged. Runs five jobs:

1. **bump-and-tag** — reads the current version, bumps it based on the label,
   commits to `main`, pushes, and creates an annotated `v*` tag
2. **publish** — publishes crates to crates.io in dependency order
   (`webtools-core` → `webtools-fetch` → `webtools-search`), skipping any
   version already on the index so a partial run can be retried (needs the
   `CRATES_IO_TOKEN` secret)
3. **create-release** — creates the GitHub release with auto-generated notes
   (runs in parallel with publish)
4. **build** — builds `webtools` for seven targets (Linux gnu/musl x86_64 +
   aarch64, macOS x86_64 + aarch64, Windows x86_64) and attaches each archive
   plus a per-asset `.sha256`
5. **checksums** — aggregates a combined `SHA256SUMS` manifest for downloaders

See [`../docs/install.md`](../docs/install.md) for the asset naming table and
checksum-verification steps.

## PR-based release flow

The recommended release process uses the `/pr` command (see `.agents/commands/pr.md`):

1. **Agent runs `/pr patch`** (or `minor`/`major`) → Creates PR with `cargo:<bump>` label
2. **PR gets merged** → Triggers `release.yml`
3. **Release workflow** → Bumps version, tags, publishes crates, builds
   cross-platform binaries, and uploads checksums — all in one workflow

This ensures version bumps are reviewable and tied to specific changes.

## Why a single workflow?

Previously, version bumping and releasing were split across two workflows
(`merge-release.yml` → tag push → `release.yml`). Tags pushed by the
`GITHUB_TOKEN` do not trigger other workflows (a GitHub Actions safety
measure), so every release required a manual tag re-push. Combining both
into a single workflow eliminates this entirely.
