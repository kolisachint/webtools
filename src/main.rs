use clap::Parser;

use webfetch::types::{ContentType, FetchOptions};

/// Token-efficient web content fetcher with reference-style URL preservation.
#[derive(Parser)]
#[command(name = "webfetch", version, about)]
struct Cli {
    /// URL to fetch.
    #[arg(long)]
    url: String,

    /// Output format: text | markdown | structured.
    #[arg(long, default_value = "text")]
    output: String,

    /// Emit the full FetchResult as JSON instead of plain content.
    #[arg(long)]
    json: bool,

    /// Soft cap on output size, in estimated tokens.
    #[arg(long)]
    max_tokens: Option<usize>,

    /// Request timeout in seconds.
    #[arg(long, default_value_t = 10)]
    timeout: u64,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    let content_type = ContentType::parse(&cli.output);
    let options = FetchOptions {
        url: cli.url.clone(),
        content_type,
        max_tokens: cli.max_tokens,
        timeout_secs: cli.timeout,
    };

    let result = webfetch::fetch_and_convert(options).await?;

    if cli.json {
        println!("{}", serde_json::to_string_pretty(&result)?);
    } else {
        println!("{}", result.content);
    }

    Ok(())
}
