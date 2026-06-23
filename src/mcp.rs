//! Minimal MCP (Model Context Protocol) stdio server.
//!
//! Speaks line-delimited JSON-RPC 2.0 over stdin/stdout — the MCP stdio
//! transport — and exposes two tools, `fetch` and `search`, so any MCP-aware
//! LLM can call them natively without shell glue. Implemented directly (no SDK
//! dependency) to keep the binary small.

use anyhow::Result;
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use webfetch::types::{ContentType, FetchOptions};
use websearch::types::SearchOptions;

const PROTOCOL_VERSION: &str = "2024-11-05";

pub async fn serve() -> Result<()> {
    let stdin = tokio::io::stdin();
    let mut lines = BufReader::new(stdin).lines();
    let mut stdout = tokio::io::stdout();

    while let Some(line) = lines.next_line().await? {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let msg: Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => continue, // ignore malformed frames
        };

        // No "id" means a notification — act on it, but never reply.
        let id = msg.get("id").cloned();
        let method = msg.get("method").and_then(Value::as_str).unwrap_or("");

        if id.is_none() {
            continue;
        }
        let id = id.unwrap();

        let response = match method {
            "initialize" => ok(id, initialize_result()),
            "tools/list" => ok(id, tools_list()),
            "tools/call" => match handle_tool_call(&msg).await {
                Ok(result) => ok(id, result),
                Err(e) => ok(id, tool_error(&format!("{e:#}"))),
            },
            "ping" => ok(id, json!({})),
            _ => err(id, -32601, "method not found"),
        };

        let mut bytes = serde_json::to_vec(&response)?;
        bytes.push(b'\n');
        stdout.write_all(&bytes).await?;
        stdout.flush().await?;
    }
    Ok(())
}

fn ok(id: Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

fn err(id: Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

fn initialize_result() -> Value {
    json!({
        "protocolVersion": PROTOCOL_VERSION,
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
                        "max_tokens": { "type": "integer", "description": "Soft cap on output size in estimated tokens" },
                        "timeout": { "type": "integer", "description": "Request timeout in seconds (default 10)" }
                    },
                    "required": ["url"]
                }
            },
            {
                "name": "search",
                "description": "Search the web (DuckDuckGo Lite) and return results with reference-style URLs. No API key required.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "query": { "type": "string", "description": "Search query" },
                        "max_results": { "type": "integer", "description": "Max results (default 5)" },
                        "safe_search": { "type": "string", "enum": ["on", "off"], "description": "Safe search toggle" },
                        "timeout": { "type": "integer", "description": "Request timeout in seconds (default 10)" }
                    },
                    "required": ["query"]
                }
            }
        ]
    })
}

/// Wrap a JSON payload as a successful MCP tool result (text content).
fn tool_text(text: String) -> Value {
    json!({ "content": [ { "type": "text", "text": text } ] })
}

fn tool_error(message: &str) -> Value {
    json!({ "content": [ { "type": "text", "text": message } ], "isError": true })
}

async fn handle_tool_call(msg: &Value) -> Result<Value> {
    let params = msg.get("params").cloned().unwrap_or(json!({}));
    let name = params.get("name").and_then(Value::as_str).unwrap_or("");
    let args = params.get("arguments").cloned().unwrap_or(json!({}));

    match name {
        "fetch" => {
            let url = args
                .get("url")
                .and_then(Value::as_str)
                .ok_or_else(|| anyhow::anyhow!("missing required argument: url"))?
                .to_string();
            let options = FetchOptions {
                url,
                content_type: ContentType::parse(
                    args.get("output").and_then(Value::as_str).unwrap_or("text"),
                ),
                max_tokens: args
                    .get("max_tokens")
                    .and_then(Value::as_u64)
                    .map(|n| n as usize),
                timeout_secs: args.get("timeout").and_then(Value::as_u64).unwrap_or(10),
                // The MCP server uses the default trust setup (OS store +
                // SSL_CERT_FILE); it exposes no insecure/extra-CA knobs.
                tls: Default::default(),
            };
            let result = webfetch::fetch_and_convert(options).await?;
            Ok(tool_text(serde_json::to_string_pretty(&result)?))
        }
        "search" => {
            let query = args
                .get("query")
                .and_then(Value::as_str)
                .ok_or_else(|| anyhow::anyhow!("missing required argument: query"))?
                .to_string();
            let safe_search = match args.get("safe_search").and_then(Value::as_str) {
                Some("on") => Some(true),
                Some("off") => Some(false),
                _ => None,
            };
            let options = SearchOptions {
                query,
                max_results: Some(
                    args.get("max_results").and_then(Value::as_u64).unwrap_or(5) as usize
                ),
                safe_search,
                timeout_secs: args.get("timeout").and_then(Value::as_u64).unwrap_or(10),
                tls: Default::default(),
            };
            let output = websearch::run_search(options).await?;
            Ok(tool_text(serde_json::to_string_pretty(&output)?))
        }
        other => Err(anyhow::anyhow!("unknown tool: {other}")),
    }
}
