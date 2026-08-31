//! Unified CLI: a single `webtools` binary exposing `fetch`, `search`, and an
//! `mcp` stdio server, the way `cargo`/`rg` ship one binary with many commands.

mod config;
mod mcp;

use std::io::Read;
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};

use webfetch::tls::TlsConfig;
use webfetch::types::{ContentType, FetchOptions, FetchResult};
use websearch::types::{SearchOptions, SearchOutput};

#[derive(Parser)]
#[command(name = "webtools", version, about)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Fetch a URL and convert it to token-efficient, reference-style output.
    Fetch {
        /// URL to fetch (and the base for resolving relative links).
        #[arg(long)]
        url: Option<String>,
        /// Read the body from a file (or `-` for stdin) instead of the
        /// network; pair with --url to set the base for relative links.
        #[arg(long)]
        from_file: Option<String>,
        /// Output format: text | markdown | structured.
        #[arg(long, default_value = "text")]
        output: String,
        /// Emit the full FetchResult as JSON.
        #[arg(long)]
        json: bool,
        /// Soft cap on output size, in estimated tokens. The body is truncated
        /// first and only the references it still cites are kept, so the whole
        /// output stays inside the cap.
        #[arg(long)]
        max_tokens: Option<usize>,
        /// Byte offset into the extracted text to start from, for reading a
        /// long page one window at a time. Take it from the previous run's
        /// `next_offset` (or the "continue with --offset N" footer); successive
        /// windows tile the document exactly.
        #[arg(long, default_value_t = 0)]
        offset: usize,
        /// Timeout in seconds for each request. The whole fetch, including
        /// redirects and retries, is bounded at three times this.
        #[arg(long)]
        timeout: Option<u64>,
        /// Extra PEM CA certificate file(s) to trust as additional roots
        /// (repeatable). Use behind a TLS-intercepting proxy whose root CA is
        /// not in the OS store.
        #[arg(long = "ca-cert", value_name = "PATH")]
        ca_cert: Vec<PathBuf>,
        /// Disable TLS certificate verification. LAST RESORT only: insecure and
        /// open to interception. Prefer the OS trust store, SSL_CERT_FILE, or
        /// --ca-cert.
        #[arg(long)]
        insecure: bool,
    },
    /// Search the web with reference-style result URLs.
    Search {
        #[arg(long)]
        query: String,
        /// Maximum number of results to return.
        #[arg(long, default_value_t = 5)]
        max_results: usize,
        /// Emit the full SearchOutput as JSON.
        #[arg(long)]
        json: bool,
        /// Safe search: "on" or "off" (omit to use the provider's default).
        #[arg(long)]
        safe_search: Option<String>,
        /// Search backend: duckduckgo (default, no key) | brave | tavily |
        /// searxng. Keys come from the environment or ~/.hoocode/settings.json.
        #[arg(long)]
        provider: Option<String>,
        /// Request timeout in seconds.
        #[arg(long, default_value_t = 10)]
        timeout: u64,
        /// Extra PEM CA certificate file(s) to trust as additional roots
        /// (repeatable). Use behind a TLS-intercepting proxy whose root CA is
        /// not in the OS store.
        #[arg(long = "ca-cert", value_name = "PATH")]
        ca_cert: Vec<PathBuf>,
        /// Disable TLS certificate verification. LAST RESORT only: insecure and
        /// open to interception. Prefer the OS trust store, SSL_CERT_FILE, or
        /// --ca-cert.
        #[arg(long)]
        insecure: bool,
    },
    /// Run as an MCP stdio server exposing `fetch` and `search` as tools.
    Mcp,
}

#[tokio::main]
async fn main() -> ExitCode {
    match run().await {
        Ok(code) => code,
        Err(err) => {
            // Concise, single-line error chain for a CLI — no backtrace dump.
            eprintln!("webtools: {err:#}");
            ExitCode::FAILURE
        }
    }
}

/// Read a local body and decode it with whatever charset it declares.
///
/// Returns the text plus a charset label to warn about when nothing could
/// decode it. There is no `Content-Type` for a local file, so the declaration
/// can only come from `<meta charset>` — but the offline path has to honour it
/// too, or `--from-file` mangles the same pages the network path handles.
fn read_input(from_file: &str) -> anyhow::Result<(String, Option<String>)> {
    let bytes = if from_file == "-" {
        let mut buf = Vec::new();
        std::io::stdin().read_to_end(&mut buf)?;
        buf
    } else {
        std::fs::read(from_file)
            .map_err(|e| anyhow::anyhow!("reading --from-file {from_file}: {e}"))?
    };
    let declared = webfetch::charset::sniff_meta(&bytes);
    Ok(webfetch::charset::decode(&bytes, declared.as_deref()))
}

fn parse_safe_search(value: Option<&str>) -> Option<bool> {
    match value.map(|s| s.to_ascii_lowercase()) {
        Some(ref s) if s == "on" || s == "strict" => Some(true),
        Some(ref s) if s == "off" || s == "none" => Some(false),
        _ => None,
    }
}

/// Merge `--ca-cert` with the paths configured in `settings.json`.
fn tls_config(flag_certs: Vec<PathBuf>, insecure: bool, configured: &[PathBuf]) -> TlsConfig {
    let mut ca_certs = configured.to_vec();
    ca_certs.extend(flag_certs);
    TlsConfig { ca_certs, insecure }
}

/// Print a fetch result for a human, with a diagnostic line when nothing was
/// extracted. An empty page and a page that needs a browser used to look
/// identical: no output, exit 0.
fn print_fetch(result: &FetchResult) {
    if !result.title.is_empty() {
        println!("{}", result.title);
    }
    if !result.final_url.is_empty() {
        println!("{}", result.final_url);
    }
    if !result.title.is_empty() || !result.final_url.is_empty() {
        println!();
    }
    println!("{}", result.content);

    // Without this a budgeted fetch ends mid-sentence with nothing to act on.
    if let Some(next) = result.next_offset {
        println!(
            "\n[showing bytes {}-{} of {} (~{} of ~{} tokens); continue with --offset {}]",
            result.offset,
            next,
            result.total_bytes,
            result.token_estimate,
            result.total_token_estimate,
            next
        );
    }
    if let Some(note) = result.status.note() {
        eprintln!("webtools: {note}");
    }
    if let Some(charset) = &result.metadata.charset {
        eprintln!(
            "webtools: warning: page declares charset {charset}; \
             it was decoded as UTF-8 and may be garbled"
        );
    }
}

fn print_search(output: &SearchOutput) {
    let rendered = websearch::render_output(output);
    if !rendered.is_empty() {
        println!("{rendered}");
    }
}

async fn run() -> anyhow::Result<ExitCode> {
    let settings = config::load();

    match Cli::parse().command {
        Commands::Fetch {
            url,
            from_file,
            output,
            json,
            max_tokens,
            offset,
            timeout,
            ca_cert,
            insecure,
        } => {
            let fetch_config = &settings.webtools.fetch;
            let base = url.clone().unwrap_or_default();
            let options = FetchOptions {
                url: base.clone(),
                content_type: ContentType::parse(&output),
                max_tokens: max_tokens.or(fetch_config.max_tokens),
                offset,
                timeout_secs: timeout.or(fetch_config.timeout_secs).unwrap_or(10),
                tls: tls_config(ca_cert, insecure, &fetch_config.ca_certs),
            };

            let result = match from_file {
                Some(path) => {
                    // Offline: convert a local/piped body (content-type sniffed).
                    if base.is_empty() {
                        eprintln!(
                            "webtools: warning: no --url, so relative links cannot be \
                             resolved and will not appear as references"
                        );
                    }
                    let (body, undecodable) = read_input(&path)?;
                    let mut result = webfetch::convert_body(&body, &base, None, &options);
                    if undecodable.is_some() {
                        result.metadata.charset = undecodable;
                    }
                    result
                }
                None => {
                    if base.is_empty() {
                        anyhow::bail!("provide --url, or --from-file to read a local body");
                    }
                    webfetch::fetch_and_convert(options).await?
                }
            };

            if json {
                println!("{}", serde_json::to_string_pretty(&result)?);
                if let Some(note) = result.status.note() {
                    eprintln!("webtools: {note}");
                }
            } else {
                print_fetch(&result);
            }
            // The request succeeded even when extraction found nothing, so the
            // exit code stays 0; `status` and the stderr note carry the detail.
            Ok(ExitCode::SUCCESS)
        }
        Commands::Search {
            query,
            max_results,
            json,
            safe_search,
            provider,
            timeout,
            ca_cert,
            insecure,
        } => {
            let search_config = &settings.webtools.search;
            let primary = search_config.resolve_primary(provider.as_deref())?;
            let fallback = search_config.resolve_fallback(&primary);

            let options = SearchOptions {
                query,
                max_results: Some(max_results),
                safe_search: parse_safe_search(safe_search.as_deref()),
                timeout_secs: timeout,
                tls: tls_config(ca_cert, insecure, &settings.webtools.fetch.ca_certs),
                provider: primary,
                fallback,
            };
            let output = websearch::run_search(options).await?;

            if json {
                println!("{}", serde_json::to_string_pretty(&output)?);
            } else {
                print_search(&output);
            }

            // A blocked search did not answer the question. Exiting 0 here is
            // what made a rate-limited search look like an empty one.
            Ok(if output.status.is_failure() {
                ExitCode::FAILURE
            } else {
                ExitCode::SUCCESS
            })
        }
        Commands::Mcp => {
            mcp::serve(settings).await?;
            Ok(ExitCode::SUCCESS)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_search_accepts_both_spellings() {
        assert_eq!(parse_safe_search(Some("on")), Some(true));
        assert_eq!(parse_safe_search(Some("STRICT")), Some(true));
        assert_eq!(parse_safe_search(Some("off")), Some(false));
        assert_eq!(parse_safe_search(None), None);
        assert_eq!(parse_safe_search(Some("maybe")), None);
    }

    #[test]
    fn ca_certs_from_flags_and_config_are_merged() {
        let configured = vec![PathBuf::from("/etc/ssl/corp.pem")];
        let tls = tls_config(vec![PathBuf::from("/tmp/extra.pem")], false, &configured);
        assert_eq!(tls.ca_certs.len(), 2);
        assert!(!tls.insecure);
    }
}
