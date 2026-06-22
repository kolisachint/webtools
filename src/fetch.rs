use reqwest::{redirect::Policy, Client};
use std::time::Duration;

const USER_AGENT: &str = concat!("webfetch/", env!("CARGO_PKG_VERSION"));

/// Outcome of an HTTP fetch: the body plus the URL we actually landed on
/// after following redirects.
pub struct FetchedPage {
    pub html: String,
    pub final_url: String,
}

pub async fn fetch_html(url: &str, timeout_secs: u64) -> anyhow::Result<FetchedPage> {
    let client = Client::builder()
        .timeout(Duration::from_secs(timeout_secs))
        .redirect(Policy::limited(5))
        .user_agent(USER_AGENT)
        .gzip(true)
        .brotli(true)
        .build()?;

    let resp = client.get(url).send().await?.error_for_status()?;
    let final_url = resp.url().to_string();
    let html = resp.text().await?;
    Ok(FetchedPage { html, final_url })
}
