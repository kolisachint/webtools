//! Shared TLS trust configuration for the HTTP clients.
//!
//! By default `reqwest`'s rustls backend trusts only the bundled webpki root
//! set, which ignores any CA the host operating system trusts. Behind a
//! TLS-intercepting proxy (common in corporate networks) the proxy presents a
//! certificate signed by an org root CA that lives in the OS trust store but
//! not in webpki, so the handshake fails with `UnknownIssuer`.
//!
//! [`TlsConfig::apply`] fixes this by assembling the trust anchors explicitly:
//!
//! 1. the OS / system trust store (via `rustls-native-certs`), so org root CAs
//!    — including proxy-injected ones — are trusted;
//! 2. the bundled webpki roots, always, so a thin or partial system store
//!    cannot leave the client unable to verify ordinary public sites;
//! 3. any certs in `SSL_CERT_FILE`, if it is set and readable;
//! 4. any explicit `--ca-cert` PEM bundles.
//!
//! `--insecure` (`danger_accept_invalid_certs`) is a strictly opt-in last
//! resort: it disables verification entirely and prints a loud warning.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;

use anyhow::Context;
use reqwest::{Certificate, ClientBuilder};
use serde::Deserialize;

/// The system trust anchors, in DER, loaded at most once per process.
///
/// [`TlsConfig::apply`] runs once per client, and the fetch path builds a fresh
/// client for every redirect hop (to keep per-URL IP pinning), so without this
/// cache a single redirected fetch would read and PEM-parse the whole OS
/// certificate bundle several times over.
fn native_root_ders() -> &'static [Vec<u8>] {
    static ROOTS: OnceLock<Vec<Vec<u8>>> = OnceLock::new();
    ROOTS.get_or_init(|| {
        let native = rustls_native_certs::load_native_certs();
        for err in &native.errors {
            eprintln!("webtools: warning: reading a system certificate failed: {err}");
        }
        native.certs.into_iter().map(|c| c.to_vec()).collect()
    })
}

/// How an HTTP client should establish TLS trust.
///
/// The OS trust store and `SSL_CERT_FILE` are always honoured; the fields here
/// carry the explicit, opt-in CLI overrides.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct TlsConfig {
    /// Extra PEM trust anchors supplied via `--ca-cert` (each file may hold one
    /// or more certificates).
    #[serde(default)]
    pub ca_certs: Vec<PathBuf>,
    /// Disable certificate verification entirely (`--insecure`). Last resort.
    #[serde(default)]
    pub insecure: bool,
}

impl TlsConfig {
    /// Apply the trust configuration to a `reqwest` client builder.
    ///
    /// When `insecure` is set, verification is turned off and trust-anchor
    /// assembly is skipped. Otherwise the OS store, the bundled webpki roots,
    /// `SSL_CERT_FILE` and `--ca-cert` are all layered together as roots.
    pub fn apply(&self, builder: ClientBuilder) -> anyhow::Result<ClientBuilder> {
        if self.insecure {
            warn_insecure_once();
            // Nothing is verified, so assembling trust anchors is pointless.
            return Ok(builder.danger_accept_invalid_certs(true));
        }

        let mut builder = builder;

        // 1. OS / system trust store. This is what lets an org root CA — or one
        //    injected by a TLS-intercepting proxy — be trusted.
        for cert in native_root_ders() {
            if let Ok(c) = Certificate::from_der(cert) {
                builder = builder.add_root_certificate(c);
            }
        }

        // 2. The bundled webpki roots, always — not only when the OS store came
        //    back empty.
        //
        //    Treating them as a fallback meant a *partial* OS store silently
        //    replaced a complete root set with an incomplete one: a container
        //    with three certificates in /etc/ssl would lose webpki entirely and
        //    fail to verify most of the web, with no signal beyond a handshake
        //    error. Only a fully empty store triggered the fallback, which is
        //    the one case that failure mode does not cover.
        //
        //    The cost is that removing a CA from the OS store does not distrust
        //    it here, since webpki may still carry it. That is a real trade, but
        //    a tool that stops reaching the web on a thin base image is the
        //    worse outcome — and it is the behaviour the changelog described.
        builder = builder.tls_built_in_root_certs(true);

        // 3. SSL_CERT_FILE — a common override in corp/proxy environments.
        //    `load_native_certs` already consults it, but we read it explicitly
        //    too so the certs are guaranteed to load as roots and an unreadable
        //    value surfaces a clear, dedicated warning rather than failing
        //    silently. (Per OpenSSL conventions, setting it points the default
        //    file at this bundle, so prefer --ca-cert to layer onto the OS store.)
        if let Some(path) = std::env::var_os("SSL_CERT_FILE") {
            let path = PathBuf::from(path);
            match std::fs::read(&path) {
                Ok(pem) => builder = add_pem_bundle(builder, &pem, &path)?,
                Err(e) => eprintln!(
                    "webtools: warning: SSL_CERT_FILE ({}) is set but unreadable: {e}",
                    path.display()
                ),
            }
        }

        // 4. Explicit --ca-cert PEM bundles (extra roots).
        for path in &self.ca_certs {
            let pem = std::fs::read(path)
                .with_context(|| format!("reading --ca-cert {}", path.display()))?;
            builder = add_pem_bundle(builder, &pem, path)?;
        }

        Ok(builder)
    }
}

/// Parse every certificate in a PEM bundle and add each as a trust anchor.
fn add_pem_bundle(
    mut builder: ClientBuilder,
    pem: &[u8],
    path: &Path,
) -> anyhow::Result<ClientBuilder> {
    let certs = Certificate::from_pem_bundle(pem)
        .with_context(|| format!("parsing PEM certificates from {}", path.display()))?;
    if certs.is_empty() {
        eprintln!(
            "webtools: warning: no certificates found in {}",
            path.display()
        );
    }
    for cert in certs {
        builder = builder.add_root_certificate(cert);
    }
    Ok(builder)
}

/// Print the `--insecure` warning at most once per process.
fn warn_insecure_once() {
    static WARNED: AtomicBool = AtomicBool::new(false);
    if !WARNED.swap(true, Ordering::Relaxed) {
        eprintln!(
            "webtools: WARNING: --insecure disables TLS certificate verification; \
             the connection can be intercepted. Use only as a last resort — \
             prefer the OS trust store, SSL_CERT_FILE, or --ca-cert."
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_secure_with_no_extra_roots() {
        let cfg = TlsConfig::default();
        assert!(!cfg.insecure);
        assert!(cfg.ca_certs.is_empty());
        // Applying the default config must succeed (loads the OS store).
        assert!(cfg.apply(reqwest::Client::builder()).is_ok());
    }

    #[test]
    fn insecure_config_applies() {
        let cfg = TlsConfig {
            insecure: true,
            ..Default::default()
        };
        assert!(cfg.apply(reqwest::Client::builder()).is_ok());
    }

    #[test]
    fn missing_ca_cert_is_an_error() {
        let cfg = TlsConfig {
            ca_certs: vec![PathBuf::from("/no/such/ca-cert.pem")],
            ..Default::default()
        };
        assert!(cfg.apply(reqwest::Client::builder()).is_err());
    }
}
