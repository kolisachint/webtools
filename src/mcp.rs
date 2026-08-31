//! Minimal MCP (Model Context Protocol) stdio server.
//!
//! Speaks line-delimited JSON-RPC 2.0 over stdin/stdout — the MCP stdio
//! transport — and exposes two tools, `fetch` and `search`, so any MCP-aware
//! LLM can call them natively without shell glue. Implemented directly (no SDK
//! dependency) to keep the binary small.
//!
//! Requests are handled concurrently: each one runs as its own task and replies
//! are serialized through a single writer. The previous loop awaited each tool
//! call before reading the next line, so one slow fetch stalled every other
//! request on the connection.

use std::sync::Arc;

use anyhow::Result;
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::{mpsc, Semaphore};
use tokio::task::JoinSet;

use webfetch::types::{ContentStatus, ContentType, FetchOptions};
use websearch::types::SearchOptions;

use crate::config::Config;

/// The protocol version this server implements. A client asking for a different
/// one is answered with its own version when we can speak it; see [`negotiate`].
const DEFAULT_PROTOCOL_VERSION: &str = "2024-11-05";

/// Protocol revisions this server is compatible with. The wire format it uses —
/// JSON-RPC framing, `tools/list`, `tools/call` — is unchanged across these.
const SUPPORTED_PROTOCOL_VERSIONS: [&str; 3] = ["2024-11-05", "2025-03-26", "2025-06-18"];

/// Default token cap for MCP `fetch` calls.
///
/// The CLI can afford an unbounded page — a terminal scrolls. An MCP result
/// goes straight into a model's context, so a large page silently costs the
/// caller its context window. Callers that want more can pass `max_tokens`.
const DEFAULT_MCP_MAX_TOKENS: usize = 6_000;

/// Maximum length of a single JSON-RPC line we will buffer (8 MiB). Without a
/// bound, a peer can make the server allocate without limit.
const MAX_LINE_BYTES: usize = 8 * 1024 * 1024;

/// How many tool calls may be in flight at once. Concurrency is the point —
/// one slow fetch must not stall the rest — but unbounded concurrency lets a
/// peer open as many sockets as it can write lines.
const MAX_CONCURRENT_CALLS: usize = 8;

pub async fn serve(config: Config) -> Result<()> {
    let config = Arc::new(config);
    let stdin = tokio::io::stdin();
    let mut lines = BufReader::with_capacity(64 * 1024, stdin).lines();

    // One writer task owns stdout, so concurrent handlers cannot interleave
    // halves of two JSON frames on the same line.
    let (tx, mut rx) = mpsc::channel::<Value>(64);
    let writer = tokio::spawn(async move {
        let mut stdout = tokio::io::stdout();
        while let Some(message) = rx.recv().await {
            let Ok(mut bytes) = serde_json::to_vec(&message) else {
                continue;
            };
            bytes.push(b'\n');
            if stdout.write_all(&bytes).await.is_err() || stdout.flush().await.is_err() {
                break;
            }
        }
    });

    // A `JoinSet` rather than a `Vec` of handles: finished tasks are reaped as
    // we go, so a long-lived server does not accumulate one entry per request
    // it has ever answered.
    let mut tasks = JoinSet::new();
    // Concurrency is bounded so a peer cannot make the server open an unlimited
    // number of simultaneous connections just by writing lines quickly.
    let permits = Arc::new(Semaphore::new(MAX_CONCURRENT_CALLS));

    while let Some(line) = lines.next_line().await? {
        // Reap anything that finished while we were blocked on stdin.
        while tasks.try_join_next().is_some() {}

        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if line.len() > MAX_LINE_BYTES {
            continue;
        }
        let Ok(msg) = serde_json::from_str::<Value>(line) else {
            continue; // ignore malformed frames
        };

        // No "id" means a notification — act on it, but never reply.
        let Some(id) = msg.get("id").cloned() else {
            continue;
        };
        let method = msg
            .get("method")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();

        let tx = tx.clone();
        let config = Arc::clone(&config);
        let permits = Arc::clone(&permits);
        tasks.spawn(async move {
            // Held for the duration of the call; dropped with the task.
            let _permit = permits.acquire_owned().await;
            let response = match method.as_str() {
                "initialize" => ok(id, initialize_result(&msg)),
                "tools/list" => ok(id, tools_list()),
                "tools/call" => match handle_tool_call(&msg, &config).await {
                    Ok(result) => ok(id, result),
                    Err(e) => ok(id, tool_error(&format!("{e:#}"))),
                },
                "ping" => ok(id, json!({})),
                _ => err(id, -32601, "method not found"),
            };
            let _ = tx.send(response).await;
        });
    }

    // stdin closed: let in-flight calls finish, then close the writer.
    while tasks.join_next().await.is_some() {}
    drop(tx);
    let _ = writer.await;
    Ok(())
}

fn ok(id: Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

fn err(id: Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

/// Answer with the client's protocol version when we speak it, so a newer
/// client is not told to downgrade for no reason.
fn negotiate(requested: Option<&str>) -> &str {
    match requested {
        Some(v) if SUPPORTED_PROTOCOL_VERSIONS.contains(&v) => {
            SUPPORTED_PROTOCOL_VERSIONS[SUPPORTED_PROTOCOL_VERSIONS
                .iter()
                .position(|s| *s == v)
                .unwrap_or(0)]
        }
        _ => DEFAULT_PROTOCOL_VERSION,
    }
}

fn initialize_result(msg: &Value) -> Value {
    let requested = msg
        .get("params")
        .and_then(|p| p.get("protocolVersion"))
        .and_then(Value::as_str);
    json!({
        "protocolVersion": negotiate(requested),
        "capabilities": { "tools": {} },
        "serverInfo": { "name": "webtools", "version": env!("CARGO_PKG_VERSION") }
    })
}

fn tools_list() -> Value {
    json!({
        "tools": [
            {
                "name": "fetch",
                "description": "Fetch a URL and return token-efficient, reference-style content. Links become inline [N] markers with full URLs in a references list. Handles HTML, JSON, and plain text.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "url": { "type": "string", "description": "URL to fetch" },
                        "output": {
                            "type": "string",
                            "enum": ["text", "markdown", "structured"],
                            "description": "Output format (default text)"
                        },
                        "max_tokens": {
                            "type": "integer",
                            "description": "Cap on output size in estimated tokens (default 6000)"
                        },
                        "offset": {
                            "type": "integer",
                            "description": "Byte offset into the extracted text to start from, for reading a long page one window at a time. Use the next_offset reported by the previous call; windows tile the document exactly."
                        },
                        "timeout": { "type": "integer", "description": "Request timeout in seconds (default 10)" },
                        "json": {
                            "type": "boolean",
                            "description": "Return the full FetchResult as JSON instead of rendered text (default false)"
                        }
                    },
                    "required": ["url"]
                }
            },
            {
                "name": "search",
                "description": "Search the web and return results with reference-style URLs. Uses DuckDuckGo by default (no API key); a keyed provider can be configured.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "query": { "type": "string", "description": "Search query" },
                        "max_results": { "type": "integer", "description": "Max results (default 5)" },
                        "safe_search": { "type": "string", "enum": ["on", "off"], "description": "Safe search toggle" },
                        "timeout": { "type": "integer", "description": "Request timeout in seconds (default 10)" },
                        "json": {
                            "type": "boolean",
                            "description": "Return the full SearchOutput as JSON instead of rendered text (default false)"
                        }
                    },
                    "required": ["query"]
                }
            }
        ]
    })
}

/// Wrap text as a successful MCP tool result.
fn tool_text(text: String) -> Value {
    json!({ "content": [ { "type": "text", "text": text } ] })
}

/// Wrap text as a failed tool result. The model needs to see that a call did
/// not answer — a blocked search or an unextractable page is not a success.
fn tool_failure(text: String) -> Value {
    json!({ "content": [ { "type": "text", "text": text } ], "isError": true })
}

fn tool_error(message: &str) -> Value {
    tool_failure(message.to_string())
}

fn arg_str<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key).and_then(Value::as_str)
}

async fn handle_tool_call(msg: &Value, config: &Config) -> Result<Value> {
    let params = msg.get("params").cloned().unwrap_or(json!({}));
    let name = params.get("name").and_then(Value::as_str).unwrap_or("");
    let args = params.get("arguments").cloned().unwrap_or(json!({}));
    let as_json = args.get("json").and_then(Value::as_bool).unwrap_or(false);

    match name {
        "fetch" => {
            let url = arg_str(&args, "url")
                .ok_or_else(|| anyhow::anyhow!("missing required argument: url"))?
                .to_string();
            let fetch_config = &config.webtools.fetch;
            let options = FetchOptions {
                url,
                content_type: ContentType::parse(arg_str(&args, "output").unwrap_or("text")),
                max_tokens: Some(
                    args.get("max_tokens")
                        .and_then(Value::as_u64)
                        .map(|n| n as usize)
                        .or(fetch_config.max_tokens)
                        .unwrap_or(DEFAULT_MCP_MAX_TOKENS),
                ),
                offset: args
                    .get("offset")
                    .and_then(Value::as_u64)
                    .map(|n| n as usize)
                    .unwrap_or(0),
                timeout_secs: args
                    .get("timeout")
                    .and_then(Value::as_u64)
                    .or(fetch_config.timeout_secs)
                    .unwrap_or(10),
                // The MCP server takes no command-line flags, so extra trust
                // anchors can only come from the config file.
                tls: webfetch::tls::TlsConfig {
                    ca_certs: fetch_config.ca_certs.clone(),
                    insecure: false,
                },
            };
            // Same cache as the CLI: an MCP client reading a long page window
            // by window issues one tool call each, so without it every window
            // is another download of the same document.
            let cache = crate::cache::Cache::resolve(false, fetch_config.cache_ttl_secs);
            let page = match cache.load(&options.url) {
                Some(page) => page,
                None => {
                    let page =
                        webfetch::fetch_page(&options.url, options.timeout_secs, &options.tls)
                            .await?;
                    cache.store(&options.url, &page);
                    page
                }
            };
            let result = webfetch::convert_page(page, &options);

            // Rendered text by default. Returning the whole FetchResult as
            // pretty JSON meant the model paid for escaped newlines and a
            // duplicate reference list — costly, from a tool whose entire point
            // is token efficiency.
            let body = if as_json {
                serde_json::to_string_pretty(&result)?
            } else {
                render_fetch(&result)
            };

            Ok(match result.status {
                ContentStatus::Ok => tool_text(body),
                status => tool_failure(format!(
                    "{}\n\n[{}]",
                    body,
                    status.note().unwrap_or("no content extracted")
                )),
            })
        }
        "search" => {
            let query = arg_str(&args, "query")
                .ok_or_else(|| anyhow::anyhow!("missing required argument: query"))?
                .to_string();
            let safe_search = match arg_str(&args, "safe_search") {
                Some("on") => Some(true),
                Some("off") => Some(false),
                _ => None,
            };
            let search_config = &config.webtools.search;
            let primary = search_config.resolve_primary(None)?;
            let fallback = search_config.resolve_fallback(&primary);

            let options = SearchOptions {
                query,
                max_results: Some(
                    args.get("max_results").and_then(Value::as_u64).unwrap_or(5) as usize
                ),
                safe_search,
                timeout_secs: args.get("timeout").and_then(Value::as_u64).unwrap_or(10),
                tls: webfetch::tls::TlsConfig {
                    ca_certs: config.webtools.fetch.ca_certs.clone(),
                    insecure: false,
                },
                provider: primary,
                fallback,
            };
            let output = websearch::run_search(options).await?;

            let body = if as_json {
                serde_json::to_string_pretty(&output)?
            } else {
                websearch::render_output(&output)
            };

            Ok(if output.status.is_failure() {
                tool_failure(body)
            } else {
                tool_text(body)
            })
        }
        other => Err(anyhow::anyhow!("unknown tool: {other}")),
    }
}

/// The compact rendering an LLM actually wants: a citation header, then the
/// content (which already carries its own reference block).
fn render_fetch(result: &webfetch::types::FetchResult) -> String {
    let mut s = String::new();
    if !result.title.is_empty() {
        s.push_str(&result.title);
        s.push('\n');
    }
    if !result.final_url.is_empty() {
        s.push_str(&result.final_url);
        s.push('\n');
    }
    if !s.is_empty() {
        s.push('\n');
    }
    s.push_str(&result.content);
    // An MCP client sees only this text, so the continuation has to live in it:
    // otherwise a budgeted page ends mid-sentence with no way to ask for more.
    if let Some(next) = result.next_offset {
        s.push_str(&format!(
            "\n\n[showing bytes {}-{} of {} (~{} of ~{} tokens); continue with offset={}]",
            result.offset,
            next,
            result.total_bytes,
            result.token_estimate,
            result.total_token_estimate,
            next
        ));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_known_protocol_version_is_echoed_back() {
        assert_eq!(negotiate(Some("2025-06-18")), "2025-06-18");
        assert_eq!(negotiate(Some("2024-11-05")), "2024-11-05");
    }

    #[test]
    fn an_unknown_protocol_version_falls_back_to_the_default() {
        assert_eq!(negotiate(Some("1999-01-01")), DEFAULT_PROTOCOL_VERSION);
        assert_eq!(negotiate(None), DEFAULT_PROTOCOL_VERSION);
    }

    #[test]
    fn failed_calls_are_marked_as_errors() {
        let v = tool_failure("blocked".into());
        assert_eq!(v["isError"], true);
        let v = tool_text("fine".into());
        assert!(v.get("isError").is_none());
    }
}
