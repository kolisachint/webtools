# CI / Release workflows

The GitHub Actions workflows live in `.github/workflows/` (`ci.yml` and
`release.yml`). There is no manual activation step.

## Workflow details

### `ci.yml`

Runs on pushes to `main` and on PRs:
- **test** (Linux) — `cargo fmt --all --check`, then
  `cargo clippy --workspace --all-targets --locked -- -D warnings`, then
  `cargo test --workspace --locked`
- **cross-compile** — `cargo check` on macOS and Windows. Both are release
  targets, so a platform-specific break has to fail here rather than at release
  time.
- **msrv** — builds the workspace on the `rust-version` declared in
  `Cargo.toml`. The libraries are published, so downstream users hold us to it.

`--locked` everywhere: a run that quietly resolves different dependencies than
`Cargo.lock` pins is not testing what ships.

### `release.yml`

A single workflow triggered when a PR with a `cargo:patch`, `cargo:minor`, or
`cargo:major` label is merged. It does **not** trigger on a tag push — pushing a
tag by hand releases nothing.

Runs five jobs:

1. **bump-and-tag** — derives the next version from the latest `v*` tag (not
   from the manifest, so a stray manual edit cannot shift the sequence), bumps
   it based on the label, stamps `CHANGELOG.md`'s `[Unreleased]` section with
   the new version and date, commits to `main`, pushes, and creates an
   annotated `v*` tag
2. **publish** — publishes crates to crates.io in dependency order
   (`webtools-core` → `webtools-fetch` → `webtools-search`), skipping any
   version already on the index so a partial run can be retried (needs the
   `CRATES_IO_TOKEN` secret)
3. **create-release** — creates the GitHub release with auto-generated notes
   (runs in parallel with publish)

Jobs 2-4 check out the tag rather than `main`: another PR can merge between
jobs, and publishing a later commit under this version would be wrong.
4. **build** — builds `webtools` for six targets (Linux gnu x86_64, Linux musl
   x86_64 + aarch64, macOS x86_64 + aarch64, Windows x86_64) and attaches each
   archive plus a per-asset `.sha256`
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
