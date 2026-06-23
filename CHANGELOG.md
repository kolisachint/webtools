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

- TLS trust now uses the operating system certificate store (via
  `rustls-native-certs`) in addition to the bundled webpki roots, so requests
  succeed behind TLS-intercepting proxies whose organization root CA lives in
  the OS store. The bundled webpki roots remain as a fallback when the OS store
  yields no usable certificates.
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
