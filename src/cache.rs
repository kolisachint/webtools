//! On-disk cache of fetched pages, so reading one document costs one download.
//!
//! Paging exists to keep a long page out of a context window; without a cache
//! it trades that for a download per window, and worse, each window sees a
//! different snapshot if the page changes mid-read — offsets from one response
//! then address text that no longer exists. The cache is what makes a windowed
//! read both cheap and coherent.
//!
//! Raw pages are cached, not converted output: extraction is deterministic, so
//! one entry serves every `--output` format and every offset of the same
//! document, and re-parsing per window costs far less than re-fetching.
//!
//! Living in the binary is deliberate, like `config`: the published libraries
//! never touch a caller's filesystem behind its back.
//!
//! Every operation is best-effort. A cache that cannot be read, written, or
//! pruned degrades to no cache at all — it must never turn a working fetch into
//! a failure.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use webfetch::fetch::FetchedPage;

/// Overrides the cache location entirely.
const DIR_ENV: &str = "WEBTOOLS_CACHE_DIR";
/// Set to a truthy value to disable the cache.
const OFF_ENV: &str = "WEBTOOLS_NO_CACHE";
/// Overrides how long an entry stays fresh, in seconds.
const TTL_ENV: &str = "WEBTOOLS_CACHE_TTL";

/// Default freshness window. Long enough to page a document without refetching
/// it, short enough that a page read twice in a session is not read stale.
pub const DEFAULT_TTL_SECS: u64 = 900;

/// Entries kept before the oldest are evicted. A cached page is at most the
/// 5 MiB body cap, so this bounds the directory at a few hundred MiB in the
/// worst case and far less in practice.
const MAX_ENTRIES: usize = 256;

/// A cached page plus what is needed to validate the entry on read.
#[derive(Serialize, Deserialize)]
struct Entry {
    /// The requested URL. Stored because the file name is a non-cryptographic
    /// hash: on a collision the URLs differ and the entry is simply a miss,
    /// rather than one page being served as another.
    url: String,
    /// Unix seconds at which this was fetched.
    fetched_at: u64,
    final_url: String,
    content_type: Option<String>,
    undecodable_charset: Option<String>,
    body: String,
}

pub struct Cache {
    dir: Option<PathBuf>,
    ttl_secs: u64,
}

impl Cache {
    /// Resolve the cache from flag, environment, config and defaults, in that
    /// order. A disabled cache, or one with no usable directory, is a `Cache`
    /// whose operations do nothing — callers need no second code path.
    pub fn resolve(disabled_by_flag: bool, configured_ttl: Option<u64>) -> Self {
        match effective_ttl(
            disabled_by_flag,
            truthy_env(OFF_ENV),
            env_u64(TTL_ENV),
            configured_ttl,
        ) {
            Some(ttl_secs) => Self {
                dir: cache_dir(),
                ttl_secs,
            },
            None => Self {
                dir: None,
                ttl_secs: 0,
            },
        }
    }

    /// The still-fresh page for `url`, if one was stored.
    pub fn load(&self, url: &str) -> Option<FetchedPage> {
        let path = self.path_for(url)?;
        let entry: Entry = serde_json::from_str(&fs::read_to_string(&path).ok()?).ok()?;
        if entry.url != url {
            return None;
        }
        if now_secs().saturating_sub(entry.fetched_at) > self.ttl_secs {
            // Expired entries are removed on sight rather than only during a
            // prune, so a directory of stale pages does not survive on reads.
            let _ = fs::remove_file(&path);
            return None;
        }
        Some(FetchedPage {
            body: entry.body,
            final_url: entry.final_url,
            content_type: entry.content_type,
            undecodable_charset: entry.undecodable_charset,
        })
    }

    /// Store a freshly fetched page. Failures are silent by design.
    pub fn store(&self, url: &str, page: &FetchedPage) {
        let Some(path) = self.path_for(url) else {
            return;
        };
        let Some(dir) = path.parent() else {
            return;
        };
        if create_dir(dir).is_err() {
            return;
        }
        let entry = Entry {
            url: url.to_string(),
            fetched_at: now_secs(),
            final_url: page.final_url.clone(),
            content_type: page.content_type.clone(),
            undecodable_charset: page.undecodable_charset.clone(),
            body: page.body.clone(),
        };
        let Ok(json) = serde_json::to_string(&entry) else {
            return;
        };
        // Write-then-rename: another process reading this key concurrently sees
        // either the old entry or the new one, never a half-written file.
        let tmp = path.with_extension(format!("tmp{}", std::process::id()));
        if fs::write(&tmp, json).is_err() {
            let _ = fs::remove_file(&tmp);
            return;
        }
        if fs::rename(&tmp, &path).is_err() {
            let _ = fs::remove_file(&tmp);
            return;
        }
        self.prune(dir);
    }

    /// Drop expired entries, then the oldest ones if the directory is still
    /// over its bound. One `read_dir` pass, so the cost stays flat.
    fn prune(&self, dir: &Path) {
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        let now = now_secs();
        let mut live: Vec<(u64, PathBuf)> = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            let modified = entry
                .metadata()
                .ok()
                .and_then(|m| m.modified().ok())
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map(|d| d.as_secs())
                .unwrap_or(0);
            if now.saturating_sub(modified) > self.ttl_secs {
                let _ = fs::remove_file(&path);
                continue;
            }
            live.push((modified, path));
        }
        if live.len() <= MAX_ENTRIES {
            return;
        }
        live.sort_by_key(|(modified, _)| *modified);
        for (_, path) in live.iter().take(live.len() - MAX_ENTRIES) {
            let _ = fs::remove_file(path);
        }
    }

    fn path_for(&self, url: &str) -> Option<PathBuf> {
        Some(self.dir.as_ref()?.join(format!("{:016x}.json", fnv1a(url))))
    }
}

/// How long entries stay fresh, or `None` when the cache is off.
///
/// Kept separate from [`Cache::resolve`] so the precedence rule is testable
/// without touching process-global environment variables — which tests running
/// in parallel cannot safely share.
fn effective_ttl(
    disabled_by_flag: bool,
    disabled_by_env: bool,
    env_ttl: Option<u64>,
    configured_ttl: Option<u64>,
) -> Option<u64> {
    if disabled_by_flag || disabled_by_env {
        return None;
    }
    let ttl = env_ttl.or(configured_ttl).unwrap_or(DEFAULT_TTL_SECS);
    // A zero TTL is the config-file way of saying "no cache".
    (ttl > 0).then_some(ttl)
}

/// FNV-1a. A cache key needs to be stable across builds and machines, which
/// rules out `DefaultHasher` (unspecified, and free to change between Rust
/// releases); it does not need to be cryptographic, because every entry carries
/// its URL and a mismatch is treated as a miss.
fn fnv1a(text: &str) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in text.as_bytes() {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// `WEBTOOLS_CACHE_DIR`, else the XDG cache directory, else the platform's
/// usual spot under `$HOME`. Returns `None` when there is nowhere to write —
/// a container with no home directory runs uncached rather than failing.
fn cache_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os(DIR_ENV) {
        let path = PathBuf::from(dir);
        if !path.as_os_str().is_empty() {
            return Some(path);
        }
    }
    if let Some(xdg) = std::env::var_os("XDG_CACHE_HOME") {
        let path = PathBuf::from(xdg);
        if path.is_absolute() {
            return Some(path.join("webtools").join("fetch"));
        }
    }
    let home = PathBuf::from(std::env::var_os("HOME")?);
    if home.as_os_str().is_empty() {
        return None;
    }
    let base = if cfg!(target_os = "macos") {
        home.join("Library").join("Caches")
    } else {
        home.join(".cache")
    };
    Some(base.join("webtools").join("fetch"))
}

/// Create the cache directory, owner-only on unix: fetched pages are the user's
/// browsing, not something to leave world-readable on a shared machine.
fn create_dir(dir: &Path) -> std::io::Result<()> {
    fs::create_dir_all(dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(dir, fs::Permissions::from_mode(0o700));
    }
    Ok(())
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn env_u64(name: &str) -> Option<u64> {
    std::env::var(name).ok()?.trim().parse().ok()
}

fn truthy_env(name: &str) -> bool {
    matches!(
        std::env::var(name)
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase()
            .as_str(),
        "1" | "true" | "yes"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn page(body: &str) -> FetchedPage {
        FetchedPage {
            body: body.to_string(),
            final_url: "https://example.com/final".to_string(),
            content_type: Some("text/html".to_string()),
            undecodable_charset: None,
        }
    }

    fn cache_in(dir: &Path, ttl_secs: u64) -> Cache {
        Cache {
            dir: Some(dir.to_path_buf()),
            ttl_secs,
        }
    }

    fn tempdir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("webtools-cache-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    #[test]
    fn a_stored_page_comes_back_intact() {
        let dir = tempdir("roundtrip");
        let cache = cache_in(&dir, 900);
        cache.store("https://example.com/doc", &page("hello"));

        let got = cache.load("https://example.com/doc").expect("cache hit");
        assert_eq!(got.body, "hello");
        assert_eq!(got.final_url, "https://example.com/final");
        assert_eq!(got.content_type.as_deref(), Some("text/html"));
    }

    #[test]
    fn a_different_url_is_a_miss() {
        let dir = tempdir("miss");
        let cache = cache_in(&dir, 900);
        cache.store("https://example.com/a", &page("a"));

        assert!(cache.load("https://example.com/b").is_none());
    }

    /// Freshness is what keeps a paged read coherent without pinning a page
    /// forever, so an expired entry must not be served — and must not linger.
    #[test]
    fn an_expired_entry_is_a_miss_and_is_removed() {
        let dir = tempdir("expiry");
        let cache = cache_in(&dir, 900);
        cache.store("https://example.com/doc", &page("stale"));

        // Age the entry rather than sleeping: the clock is the input under
        // test, so moving it is the honest way to reach the expired branch.
        let path = cache.path_for("https://example.com/doc").expect("path");
        let mut entry: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&path).expect("read")).expect("json");
        entry["fetched_at"] = serde_json::json!(now_secs() - 901);
        fs::write(&path, entry.to_string()).expect("write");

        assert!(cache.load("https://example.com/doc").is_none());
        assert!(!path.exists(), "expired entry left behind");
    }

    /// The freshness window is the whole guarantee that paging sees one
    /// snapshot, so an entry inside it must still be served.
    #[test]
    fn an_entry_inside_the_window_is_still_served() {
        let dir = tempdir("fresh");
        let cache = cache_in(&dir, 900);
        cache.store("https://example.com/doc", &page("fresh"));

        let path = cache.path_for("https://example.com/doc").expect("path");
        let mut entry: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&path).expect("read")).expect("json");
        entry["fetched_at"] = serde_json::json!(now_secs() - 899);
        fs::write(&path, entry.to_string()).expect("write");

        assert_eq!(
            cache
                .load("https://example.com/doc")
                .expect("cache hit")
                .body,
            "fresh"
        );
    }

    /// The file name is a hash, so an entry whose URL does not match is another
    /// page's, not this one's.
    #[test]
    fn an_entry_whose_url_does_not_match_is_ignored() {
        let dir = tempdir("collision");
        let cache = cache_in(&dir, 900);
        cache.store("https://example.com/doc", &page("body"));
        let path = cache.path_for("https://example.com/doc").expect("path");
        let mut entry: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&path).expect("read")).expect("json");
        entry["url"] = serde_json::Value::String("https://elsewhere.test/other".into());
        fs::write(&path, entry.to_string()).expect("write");

        assert!(cache.load("https://example.com/doc").is_none());
    }

    #[test]
    fn either_switch_turns_the_cache_off() {
        // The flag wins over every source of a TTL, including an explicit one.
        assert_eq!(effective_ttl(true, false, Some(60), Some(60)), None);
        assert_eq!(effective_ttl(false, true, Some(60), Some(60)), None);
        // A zero TTL is the config-file way of saying the same thing.
        assert_eq!(effective_ttl(false, false, None, Some(0)), None);
        assert_eq!(effective_ttl(false, false, Some(0), None), None);
    }

    #[test]
    fn ttl_precedence_is_env_then_config_then_default() {
        assert_eq!(effective_ttl(false, false, Some(30), Some(60)), Some(30));
        assert_eq!(effective_ttl(false, false, None, Some(60)), Some(60));
        assert_eq!(
            effective_ttl(false, false, None, None),
            Some(DEFAULT_TTL_SECS)
        );
    }

    /// A cache with nowhere to write (no flag, no home) must be inert rather
    /// than a second code path for every caller.
    #[test]
    fn a_cache_without_a_directory_stores_and_serves_nothing() {
        let cache = Cache {
            dir: None,
            ttl_secs: 900,
        };
        cache.store("https://example.com/doc", &page("body"));

        assert!(cache.load("https://example.com/doc").is_none());
    }

    #[test]
    fn keys_are_stable_and_distinct() {
        assert_eq!(
            fnv1a("https://example.com/a"),
            fnv1a("https://example.com/a")
        );
        assert_ne!(
            fnv1a("https://example.com/a"),
            fnv1a("https://example.com/b")
        );
    }
}
