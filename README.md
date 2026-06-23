# webtools

[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](LICENSE)

A unified, **token-efficient** web `fetch` + `search` CLI for LLM agents,
built around **reference-style URL preservation**. One small, fast binary; no
API keys, no backend.

```bash
webtools fetch  --url https://docs.example.com/api   # page → compact text + refs
webtools search --query "rust async runtime"          # web search → results + refs
```

Links become inline `[N]` markers with the full URLs collected into a trailing
reference block — the agent sees `[1]` (≈1 token) but can still recover the
exact URL.

## Behind a proxy?

`webtools` trusts the OS certificate store (plus `SSL_CERT_FILE`), so it works
through TLS-intercepting corporate proxies out of the box. Add a custom root CA
with `--ca-cert <PATH>`, or, as a last resort, skip verification with
`--insecure`. See [TLS, proxies, and custom CAs](docs/product.md#tls-proxies-and-custom-cas).

## Docs

- [Product](docs/product.md) — what it does, features, usage, MCP, architecture
- [Install & Development](docs/install.md) — install, library use, releasing

## License

[MIT](LICENSE)

