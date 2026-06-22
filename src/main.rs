//! Unified CLI: a single binary exposing the `webfetch` and `websearch`
//! tools as subcommands, the way `cargo`/`rg` ship one binary with many
//! commands.

use clap::{Parser, Subcommand};

use webfetch::types::{ContentType, FetchOptions};
use websearch::types::SearchOptions;

#[derive(Parser)]
#[command(name = "webfetch-tools", version, about)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Fetch a URL and convert it to token-efficient, reference-style output.
    Webfetch {
        #[arg(long)]
        url: String,
        /// Output format: text | markdown | structured.
        #[arg(long, default_value = "text")]
        output: String,
        /// Emit the full FetchResult as JSON.
        #[arg(long)]
        json: bool,
        /// Soft cap on output size, in estimated tokens.
        #[arg(long)]
        max_tokens: Option<usize>,
        /// Request timeout in seconds.
        #[arg(long, default_value_t = 10)]
        timeout: u64,
    },
    /// Search the web (DuckDuckGo Lite) with reference-style result URLs.
    Websearch {
        #[arg(long)]
        query: String,
        /// Maximum number of results to return.
        #[arg(long, default_value_t = 5)]
        max_results: usize,
        /// Emit the full SearchOutput as JSON.
        #[arg(long)]
        json: bool,
        /// Enable strict safe search (omit to use DDG's default).
        #[arg(long)]
        safe_search: bool,
        /// Request timeout in seconds.
        #[arg(long, default_value_t = 10)]
        timeout: u64,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    match Cli::parse().command {
        Commands::Webfetch {
            url,
            output,
            json,
            max_tokens,
            timeout,
        } => {
            let options = FetchOptions {
                url,
                content_type: ContentType::parse(&output),
                max_tokens,
                timeout_secs: timeout,
            };
            let result = webfetch::fetch_and_convert(options).await?;
            if json {
                println!("{}", serde_json::to_string_pretty(&result)?);
            } else {
                println!("{}", result.content);
            }
        }
        Commands::Websearch {
            query,
            max_results,
            json,
            safe_search,
            timeout,
        } => {
            let options = SearchOptions {
                query,
                max_results: Some(max_results),
                safe_search: if safe_search { Some(true) } else { None },
                timeout_secs: timeout,
            };
            let output = websearch::run_search(options).await?;
            if json {
                println!("{}", serde_json::to_string_pretty(&output)?);
            } else {
                println!("{}", websearch::format_results(&output.results));
                let refs = websearch::render_references(&output.references);
                if !refs.is_empty() {
                    println!("\n{refs}");
                }
            }
        }
    }
    Ok(())
}
