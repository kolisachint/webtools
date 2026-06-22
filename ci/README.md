# CI / Release workflow templates

These are the GitHub Actions workflows for the project. They live here rather
than in `.github/workflows/` because the session that generated them pushed
over an OAuth token **without `workflow` scope**, which GitHub refuses for any
commit touching `.github/workflows/`.

## Current status

✅ **Workflows are now active** in `.github/workflows/`.

## To activate

Move all workflow files into place:

```bash
mkdir -p .github/workflows
git mv ci/ci.yml               .github/workflows/ci.yml
git mv ci/release.yml          .github/workflows/release.yml
git mv ci/merge-release.yml    .github/workflows/merge-release.yml
git commit -m "Enable CI and release workflows"
git push
```

**Alternatively**, use the GitHub web UI:
1. Go to your repo → Actions → New workflow
2. Select "set up a workflow yourself"
3. Delete the placeholder and paste the contents of each workflow file
4. Commit via the web UI (this bypasses the OAuth scope issue)

## Workflow details

### `ci.yml`

Runs on pushes to `main` and on PRs:
- `cargo fmt --all --check` — formatting
- `cargo clippy --workspace --all-targets -- -D warnings` — lints
- `cargo test --workspace` — all tests

### `release.yml`

Runs on `v*` tags (e.g. `v0.1.0`):
- Builds `webtools` for Linux (`x86_64-unknown-linux-gnu`) and macOS (`aarch64-apple-darwin`)
- Packages each as a `.tar.gz` and attaches to the GitHub release
- Generates release notes automatically

### `merge-release.yml`

Runs when a PR with a `cargo:patch`, `cargo:minor`, or `cargo:major` label is merged:
- Reads the current version from `Cargo.toml`
- Bumps the version based on the label
- Updates `Cargo.lock`
- Commits the version change
- Creates a `v*` git tag
- Pushes to `main`

This triggers `release.yml` which builds and publishes binaries.

## PR-based release flow

The recommended release process uses the `/pr` command (see `.agents/commands/pr.md`):

1. **Agent runs `/pr patch`** (or `minor`/`major`) → Creates PR with `cargo:<bump>` label
2. **PR gets merged** → Triggers `merge-release.yml`
3. **Merge workflow** → Bumps version, tags, pushes
4. **Tag push** → Triggers `release.yml` → Builds binaries

This ensures version bumps are reviewable and tied to specific changes.

## Cutting a release (manual)

If you need to release without a PR:

```bash
git tag v0.1.0
git push origin v0.1.0
```

This triggers the release workflow directly.
