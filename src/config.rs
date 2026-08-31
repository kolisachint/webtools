//! Optional configuration from `~/.hoocode/settings.json`.
//!
//! Config loading lives in the binary on purpose. The library crates are
//! published to crates.io, and a library that reaches into a user's home
//! directory behind its caller's back is surprising and hard to test — so the
//! CLI resolves every setting here and hands the libraries fully populated
//! option structs.
//!
//! `settings.json` is shared with other tooling, so this reads only its own
//! `webtools` section and ignores every other key. A missing file is not an
//! error: with no configuration at all, `webtools` still works against keyless
//! DuckDuckGo.
//!
//! Precedence, highest first: command-line flag, environment variable, config
//! file, built-in default. Environment variables matter as much as the file —
//! MCP clients launch servers with an `env` block, and containers often have no
//! home directory to read.

use std::path::{Path, PathBuf};

use serde::Deserialize;
use websearch::providers::Provider;

/// Overrides the config file location entirely.
const CONFIG_ENV: &str = "HOOCODE_CONFIG";
/// Path under the user's home directory when `HOOCODE_CONFIG` is unset.
const CONFIG_RELATIVE: &str = ".hoocode/settings.json";

/// The `webtools` section of `settings.json`. Unknown keys are ignored so a
/// file shared with other tools never fails to load here.
#[derive(Debug, Default, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub webtools: WebtoolsConfig,
}

#[derive(Debug, Default, Deserialize)]
pub struct WebtoolsConfig {
    #[serde(default)]
    pub search: SearchConfig,
    #[serde(default)]
    pub fetch: FetchConfig,
}

#[derive(Debug, Default, Deserialize)]
pub struct SearchConfig {
    /// Provider name to use by default (`brave`, `tavily`, `searxng`, `duckduckgo`).
    #[serde(default)]
    pub provider: Option<String>,
    /// Provider to fall back to when the primary fails or is blocked.
    #[serde(default)]
    pub fallback: Option<String>,
    /// Credentials and endpoints, keyed by provider name.
    #[serde(default)]
    pub providers: ProviderCredentials,
}

#[derive(Debug, Default, Deserialize)]
pub struct ProviderCredentials {
    #[serde(default)]
    pub brave: Option<KeyEntry>,
    #[serde(default)]
    pub tavily: Option<KeyEntry>,
    #[serde(default)]
    pub searxng: Option<SearxngEntry>,
}

#[derive(Default, Deserialize)]
pub struct KeyEntry {
    #[serde(default)]
    pub api_key: Option<String>,
}

/// Redacting `Debug`, so a config dump can never print a credential.
impl std::fmt::Debug for KeyEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "KeyEntry {{ api_key: {} }}",
            if self.api_key.is_some() {
                "<redacted>"
            } else {
                "None"
            }
        )
    }
}

#[derive(Default, Deserialize)]
pub struct SearxngEntry {
    #[serde(default)]
    pub base_url: Option<String>,
    #[serde(default)]
    pub api_key: Option<String>,
}

impl std::fmt::Debug for SearxngEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "SearxngEntry {{ base_url: {:?}, api_key: {} }}",
            self.base_url,
            if self.api_key.is_some() {
                "<redacted>"
            } else {
                "None"
            }
        )
    }
}

#[derive(Debug, Default, Deserialize)]
pub struct FetchConfig {
    #[serde(default)]
    pub timeout_secs: Option<u64>,
    #[serde(default)]
    pub max_tokens: Option<usize>,
    /// Extra PEM trust anchors, the config-file equivalent of `--ca-cert`.
    /// This is the only way to give the MCP server a corporate root CA, since
    /// it takes no command-line flags.
    #[serde(default)]
    pub ca_certs: Vec<PathBuf>,
    /// How long a fetched page stays cached, in seconds. `0` disables the
    /// cache. Defaults to 900; `WEBTOOLS_CACHE_TTL` overrides this, and
    /// `--no-cache` / `WEBTOOLS_NO_CACHE` turn it off outright.
    #[serde(default)]
    pub cache_ttl_secs: Option<u64>,
}

/// Where the config file would be, if the platform can tell us.
pub fn config_path() -> Option<PathBuf> {
    if let Some(explicit) = std::env::var_os(CONFIG_ENV) {
        return Some(PathBuf::from(explicit));
    }
    home_dir().map(|home| home.join(CONFIG_RELATIVE))
}

/// The user's home directory. `HOME` on Unix, `USERPROFILE` on Windows — the
/// Windows target is shipped, so `HOME` alone would not do.
fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
}

/// Load the configuration, or the defaults when there is none.
///
/// A missing file is silent. A malformed one warns and is skipped rather than
/// failing the command: a broken config should not make an otherwise working
/// tool refuse to run. The warning reports the location of the problem, never
/// the file's contents, which hold credentials.
pub fn load() -> Config {
    let Some(path) = config_path() else {
        return Config::default();
    };
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Config::default();
    };
    warn_if_world_readable(&path);

    match serde_json::from_str::<Config>(&text) {
        Ok(config) => config,
        Err(e) => {
            eprintln!(
                "webtools: warning: ignoring {} — invalid JSON at line {}, column {}",
                path.display(),
                e.line(),
                e.column()
            );
            Config::default()
        }
    }
}

/// Warn when a file holding API keys is readable by other users.
#[cfg(unix)]
fn warn_if_world_readable(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    if let Ok(meta) = std::fs::metadata(path) {
        let mode = meta.permissions().mode();
        if mode & 0o077 != 0 {
            eprintln!(
                "webtools: warning: {} is readable by other users (mode {:o}); \
                 it holds API keys — consider chmod 600",
                path.display(),
                mode & 0o777
            );
        }
    }
}

#[cfg(not(unix))]
fn warn_if_world_readable(_path: &Path) {}

/// Read a provider's key from the environment, which outranks the file.
fn env_key(names: &[&str]) -> Option<String> {
    names
        .iter()
        .find_map(|n| std::env::var(n).ok())
        .filter(|v| !v.trim().is_empty())
}

impl SearchConfig {
    /// Build a [`Provider`] by name, pulling the credential from the
    /// environment first and the config file second.
    ///
    /// Returns an error rather than quietly falling back when a provider was
    /// asked for by name but has no key: silently searching a different engine
    /// than the one requested is exactly the kind of invisible degradation this
    /// whole area is meant to remove.
    pub fn build(&self, name: &str) -> anyhow::Result<Provider> {
        match name {
            "duckduckgo" => Ok(Provider::Duckduckgo),
            "brave" => {
                let api_key = env_key(&["WEBTOOLS_BRAVE_API_KEY", "BRAVE_API_KEY"])
                    .or_else(|| self.providers.brave.as_ref()?.api_key.clone())
                    .ok_or_else(|| missing_key("brave", "BRAVE_API_KEY"))?;
                Ok(Provider::Brave { api_key })
            }
            "tavily" => {
                let api_key = env_key(&["WEBTOOLS_TAVILY_API_KEY", "TAVILY_API_KEY"])
                    .or_else(|| self.providers.tavily.as_ref()?.api_key.clone())
                    .ok_or_else(|| missing_key("tavily", "TAVILY_API_KEY"))?;
                Ok(Provider::Tavily { api_key })
            }
            "searxng" => {
                let entry = self.providers.searxng.as_ref();
                let base_url = env_key(&["WEBTOOLS_SEARXNG_URL"])
                    .or_else(|| entry?.base_url.clone())
                    .ok_or_else(|| {
                        anyhow::anyhow!(
                            "searxng needs a base_url: set WEBTOOLS_SEARXNG_URL, or \
                             webtools.search.providers.searxng.base_url in the config file"
                        )
                    })?;
                let api_key = env_key(&["WEBTOOLS_SEARXNG_API_KEY"])
                    .or_else(|| entry.and_then(|e| e.api_key.clone()));
                Ok(Provider::Searxng { base_url, api_key })
            }
            other => anyhow::bail!(
                "unknown search provider `{other}` (expected duckduckgo, brave, tavily, or searxng)"
            ),
        }
    }

    /// Resolve the primary provider: explicit flag, then environment, then
    /// config file, then keyless DuckDuckGo.
    pub fn resolve_primary(&self, flag: Option<&str>) -> anyhow::Result<Provider> {
        let requested = flag
            .map(str::to_string)
            .or_else(|| std::env::var("WEBTOOLS_SEARCH_PROVIDER").ok())
            .or_else(|| self.provider.clone());

        let Some(requested) = requested else {
            return Ok(Provider::Duckduckgo);
        };
        let name = Provider::parse_name(&requested)
            .ok_or_else(|| anyhow::anyhow!("unknown search provider `{requested}`"))?;
        self.build(name)
    }

    /// Resolve the fallback provider. Unlike the primary, a fallback that is
    /// configured but unusable is dropped with a warning instead of failing the
    /// command — it is a safety net, not the thing the user asked for.
    pub fn resolve_fallback(&self, primary: &Provider) -> Option<Provider> {
        let requested = std::env::var("WEBTOOLS_SEARCH_FALLBACK")
            .ok()
            .or_else(|| self.fallback.clone())?;
        if requested.trim().eq_ignore_ascii_case("none") {
            return None;
        }
        let name = Provider::parse_name(&requested)?;
        if name == primary.label() {
            return None;
        }
        match self.build(name) {
            Ok(provider) => Some(provider),
            Err(e) => {
                eprintln!("webtools: warning: ignoring search fallback: {e:#}");
                None
            }
        }
    }
}

fn missing_key(provider: &str, env_name: &str) -> anyhow::Error {
    anyhow::anyhow!(
        "{provider} needs an API key: set {env_name}, or \
         webtools.search.providers.{provider}.api_key in the config file"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{
      "someOtherTool": { "unrelated": true },
      "webtools": {
        "search": {
          "provider": "brave",
          "fallback": "duckduckgo",
          "providers": {
            "brave": { "api_key": "BSA-from-file" },
            "searxng": { "base_url": "https://searx.internal" }
          }
        },
        "fetch": { "timeout_secs": 20, "max_tokens": 4000 }
      }
    }"#;

    fn sample() -> Config {
        serde_json::from_str(SAMPLE).expect("sample config parses")
    }

    #[test]
    fn unrelated_sections_are_ignored() {
        let config = sample();
        assert_eq!(config.webtools.search.provider.as_deref(), Some("brave"));
        assert_eq!(config.webtools.fetch.timeout_secs, Some(20));
        assert_eq!(config.webtools.fetch.max_tokens, Some(4000));
    }

    #[test]
    fn an_empty_file_is_all_defaults() {
        let config: Config = serde_json::from_str("{}").unwrap();
        assert!(config.webtools.search.provider.is_none());
        assert!(config.webtools.fetch.ca_certs.is_empty());
    }

    #[test]
    fn a_key_from_the_file_builds_a_provider() {
        let provider = sample().webtools.search.build("brave").unwrap();
        assert_eq!(provider.label(), "brave");
    }

    #[test]
    fn a_provider_without_a_key_is_an_error_not_a_silent_fallback() {
        let config: Config = serde_json::from_str("{}").unwrap();
        let err = config.webtools.search.build("tavily").unwrap_err();
        let message = format!("{err:#}");
        assert!(message.contains("TAVILY_API_KEY"), "{message}");
    }

    #[test]
    fn searxng_needs_a_base_url() {
        let config: Config = serde_json::from_str("{}").unwrap();
        assert!(config.webtools.search.build("searxng").is_err());
        let provider = sample().webtools.search.build("searxng").unwrap();
        assert_eq!(provider.label(), "searxng");
    }

    #[test]
    fn unknown_provider_names_are_rejected() {
        assert!(sample().webtools.search.build("altavista").is_err());
    }

    #[test]
    fn debug_output_never_carries_a_key() {
        let shown = format!("{:?}", sample());
        assert!(!shown.contains("BSA-from-file"), "leaked: {shown}");
        assert!(shown.contains("<redacted>"));
    }

    #[test]
    fn a_fallback_matching_the_primary_is_dropped() {
        let config = sample();
        let primary = Provider::Duckduckgo;
        assert!(config.webtools.search.resolve_fallback(&primary).is_none());
    }
}
