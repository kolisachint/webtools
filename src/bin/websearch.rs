use clap::Parser;

use webfetch::search::{run_search, types::SearchOptions};

/// Zero-infrastructure web search (DuckDuckGo Lite) with reference-style URLs.
#[derive(Parser)]
#[command(
    name = "websearch",
    version,
    about = "Zero-infrastructure web search (DuckDuckGo Lite) with reference-style URLs"
)]
struct Cli {
    /// Search query.
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
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    let options = SearchOptions {
        query: cli.query.clone(),
        max_results: Some(cli.max_results),
        safe_search: if cli.safe_search { Some(true) } else { None },
        timeout_secs: cli.timeout,
    };

    let output = run_search(options).await?;

    if cli.json {
        println!("{}", serde_json::to_string_pretty(&output)?);
    } else {
        println!("{}", webfetch::search::format_results(&output.results));
        let refs = webfetch::search::render_references(&output.references);
        if !refs.is_empty() {
            println!("\n{refs}");
        }
    }

    Ok(())
}
