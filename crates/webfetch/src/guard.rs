//! SSRF guard for the fetch path.
//!
//! `fetch` is reachable from the CLI and the MCP server, so a crafted URL or a
//! prompt-injected link could otherwise be used to reach the cloud metadata
//! endpoint (`169.254.169.254`), `localhost`, or services on the private
//! network. This module rejects non-`http(s)` schemes and any URL whose host
//! resolves to a non-public IP address, on both the initial request and every
//! redirect hop.
//!
//! Set `WEBFETCH_ALLOW_PRIVATE=1` to disable the guard (for trusted internal
//! use or tests).

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, ToSocketAddrs};

use url::{Host, Url};

/// Env var that, when set to `1`/`true`, disables the SSRF guard.
const ALLOW_PRIVATE_ENV: &str = "WEBFETCH_ALLOW_PRIVATE";

/// Whether the guard is disabled via environment opt-out.
pub fn allow_private() -> bool {
    matches!(
        std::env::var(ALLOW_PRIVATE_ENV).ok().as_deref(),
        Some("1") | Some("true") | Some("TRUE")
    )
}

/// Returns true if `ip` is not safe to fetch from a public-web client:
/// loopback, private, link-local (incl. cloud metadata), CGNAT, unspecified,
/// multicast, broadcast, documentation/benchmark ranges, and the IPv6
/// equivalents (ULA, link-local, IPv4-mapped).
pub fn is_blocked_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => is_blocked_ipv4(v4),
        IpAddr::V6(v6) => is_blocked_ipv6(v6),
    }
}

fn is_blocked_ipv4(ip: Ipv4Addr) -> bool {
    let o = ip.octets();
    ip.is_loopback()           // 127.0.0.0/8
        || ip.is_private()         // 10/8, 172.16/12, 192.168/16
        || ip.is_link_local()     // 169.254.0.0/16 (cloud metadata)
        || ip.is_broadcast()      // 255.255.255.255
        || ip.is_unspecified()    // 0.0.0.0
        || ip.is_multicast()      // 224.0.0.0/4
        || ip.is_documentation()  // 192.0.2/24, 198.51.100/24, 203.0.113/24
        || o[0] == 0              // 0.0.0.0/8 "this network"
        || (o[0] == 100 && (o[1] & 0xc0) == 64) // 100.64.0.0/10 CGNAT
        || (o[0] == 192 && o[1] == 0 && o[2] == 0) // 192.0.0.0/24 IETF protocol
        || (o[0] == 198 && (o[1] & 0xfe) == 18) // 198.18.0.0/15 benchmarking
        || o[0] >= 240 // 240.0.0.0/4 reserved (excludes broadcast already)
}

fn is_blocked_ipv6(ip: Ipv6Addr) -> bool {
    // IPv4-mapped / -compatible: classify by the embedded IPv4 address.
    if let Some(v4) = ip.to_ipv4_mapped() {
        return is_blocked_ipv4(v4);
    }
    if let Some(v4) = ip.to_ipv4() {
        // Covers ::a.b.c.d (incl. ::1 loopback and :: unspecified).
        return is_blocked_ipv4(v4);
    }
    let seg = ip.segments();
    ip.is_loopback()
        || ip.is_unspecified()
        || ip.is_multicast()
        || (seg[0] & 0xffc0) == 0xfe80 // fe80::/10 link-local
        || (seg[0] & 0xfe00) == 0xfc00 // fc00::/7 unique local (ULA)
        || (seg[0] == 0x2001 && seg[1] == 0x0db8) // 2001:db8::/32 documentation
}

/// An error describing why a URL was rejected by the guard.
#[derive(Debug)]
pub struct BlockedUrl(pub String);

impl std::fmt::Display for BlockedUrl {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "blocked URL: {}", self.0)
    }
}

impl std::error::Error for BlockedUrl {}

/// Validate a URL's scheme and resolve+classify its host. On success returns
/// the validated socket addresses (host resolved to public IPs) so the caller
/// can pin the connection and avoid a DNS-rebinding TOCTOU window.
///
/// A no-op (returns `Ok(vec![])`) when the guard is disabled via env.
pub fn validate_url(url: &Url) -> Result<Vec<std::net::SocketAddr>, BlockedUrl> {
    if allow_private() {
        return Ok(Vec::new());
    }

    match url.scheme() {
        "http" | "https" => {}
        other => return Err(BlockedUrl(format!("scheme `{other}` not allowed"))),
    }

    let host = url
        .host()
        .ok_or_else(|| BlockedUrl(format!("no host in {url}")))?;

    match host {
        Host::Ipv4(ip) => {
            if is_blocked_ip(IpAddr::V4(ip)) {
                return Err(BlockedUrl(format!("host IP {ip} is not public")));
            }
            Ok(Vec::new())
        }
        Host::Ipv6(ip) => {
            if is_blocked_ip(IpAddr::V6(ip)) {
                return Err(BlockedUrl(format!("host IP {ip} is not public")));
            }
            Ok(Vec::new())
        }
        Host::Domain(domain) => validate_domain(url, domain),
    }
}

fn validate_domain(url: &Url, domain: &str) -> Result<Vec<std::net::SocketAddr>, BlockedUrl> {
    // Block obvious local names early; DNS may also resolve these.
    let lower = domain.to_ascii_lowercase();
    if lower == "localhost" || lower.ends_with(".localhost") {
        return Err(BlockedUrl(format!("host `{domain}` is local")));
    }

    let port = url
        .port_or_known_default()
        .ok_or_else(|| BlockedUrl(format!("no port for {url}")))?;

    // Resolve and require that EVERY resolved address is public, then return
    // them so the connection can be pinned to the validated set.
    let addrs: Vec<_> = (domain, port)
        .to_socket_addrs()
        .map_err(|e| BlockedUrl(format!("cannot resolve `{domain}`: {e}")))?
        .collect();

    if addrs.is_empty() {
        return Err(BlockedUrl(format!("`{domain}` resolved to no addresses")));
    }
    for addr in &addrs {
        if is_blocked_ip(addr.ip()) {
            return Err(BlockedUrl(format!(
                "`{domain}` resolves to non-public IP {}",
                addr.ip()
            )));
        }
    }
    Ok(addrs)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn blocked(s: &str) -> bool {
        is_blocked_ip(s.parse::<IpAddr>().unwrap())
    }

    #[test]
    fn blocks_loopback_and_private_and_metadata() {
        assert!(blocked("127.0.0.1"));
        assert!(blocked("10.0.0.1"));
        assert!(blocked("172.16.5.4"));
        assert!(blocked("192.168.1.1"));
        assert!(blocked("169.254.169.254")); // cloud metadata
        assert!(blocked("100.64.0.1")); // CGNAT
        assert!(blocked("0.0.0.0"));
        assert!(blocked("255.255.255.255"));
        assert!(blocked("224.0.0.1")); // multicast
        assert!(blocked("240.0.0.1")); // reserved
    }

    #[test]
    fn blocks_ipv6_local_and_mapped() {
        assert!(blocked("::1")); // loopback
        assert!(blocked("::")); // unspecified
        assert!(blocked("fe80::1")); // link-local
        assert!(blocked("fc00::1")); // ULA
        assert!(blocked("::ffff:127.0.0.1")); // v4-mapped loopback
        assert!(blocked("::ffff:169.254.169.254")); // v4-mapped metadata
    }

    #[test]
    fn allows_public() {
        assert!(!blocked("1.1.1.1"));
        assert!(!blocked("8.8.8.8"));
        assert!(!blocked("93.184.216.34")); // example.com
        assert!(!blocked("2606:4700:4700::1111")); // cloudflare v6
    }

    #[test]
    fn rejects_non_http_scheme() {
        let url = Url::parse("file:///etc/passwd").unwrap();
        assert!(validate_url(&url).is_err());
        let url = Url::parse("ftp://example.com/x").unwrap();
        assert!(validate_url(&url).is_err());
    }

    #[test]
    fn rejects_literal_metadata_ip_url() {
        let url = Url::parse("http://169.254.169.254/latest/meta-data/").unwrap();
        assert!(validate_url(&url).is_err());
    }

    #[test]
    fn rejects_localhost_name() {
        let url = Url::parse("http://localhost:8080/admin").unwrap();
        assert!(validate_url(&url).is_err());
    }
}
