//! Drives the real `webtools mcp` stdio server with JSON-RPC frames and checks
//! the responses, including that requests are actually served concurrently.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Send `requests` to a fresh `webtools mcp`, close stdin, and return the parsed
/// response frames. `env` is applied to the child.
fn exchange(requests: &str, env: &[(&str, &str)]) -> Vec<serde_json::Value> {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_webtools"));
    cmd.arg("mcp")
        .env_remove("HOOCODE_CONFIG")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    for (k, v) in env {
        cmd.env(k, v);
    }
    let mut child = cmd.spawn().expect("spawn webtools mcp");

    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(requests.as_bytes())
        .expect("write requests");
    // Dropping stdin (taken above) closes it, so the server loop ends.

    let mut out = String::new();
    child
        .stdout
        .take()
        .expect("stdout")
        .read_to_string(&mut out)
        .expect("read stdout");
    child.wait().expect("wait");

    out.lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).unwrap_or_else(|e| panic!("bad frame {l:?}: {e}")))
        .collect()
}

fn by_id(frames: &[serde_json::Value], id: i64) -> &serde_json::Value {
    frames
        .iter()
        .find(|f| f["id"] == id)
        .unwrap_or_else(|| panic!("no response with id {id}; got {frames:?}"))
}

#[test]
fn mcp_initialize_and_tools_list() {
    let requests = concat!(
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#,
        "\n",
        r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#,
        "\n",
    );
    let frames = exchange(requests, &[]);
    assert_eq!(frames.len(), 2, "frames: {frames:?}");

    let init = by_id(&frames, 1);
    assert_eq!(init["result"]["serverInfo"]["name"], "webtools");
    assert!(init["result"]["capabilities"]["tools"].is_object());

    let list = by_id(&frames, 2);
    let tools = list["result"]["tools"].as_array().unwrap();
    let names: Vec<&str> = tools.iter().filter_map(|t| t["name"].as_str()).collect();
    assert!(names.contains(&"fetch"), "tools: {names:?}");
    assert!(names.contains(&"search"), "tools: {names:?}");
}

/// A client asking for a version this server speaks should get that version
/// back, not be told to downgrade.
#[test]
fn mcp_echoes_a_supported_protocol_version() {
    let frames = exchange(
        concat!(
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18"}}"#,
            "\n"
        ),
        &[],
    );
    assert_eq!(by_id(&frames, 1)["result"]["protocolVersion"], "2025-06-18");
}

/// A call that did not answer has to be marked, or the model treats a refusal
/// as a result.
#[test]
fn mcp_marks_failed_calls_as_errors() {
    let requests = concat!(
        r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"fetch","arguments":{"url":"http://169.254.169.254/latest/meta-data/"}}}"#,
        "\n",
        r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"nope","arguments":{}}}"#,
        "\n",
        r#"{"jsonrpc":"2.0","id":3,"method":"resources/list"}"#,
        "\n",
    );
    let frames = exchange(requests, &[]);

    let blocked = by_id(&frames, 1);
    assert_eq!(blocked["result"]["isError"], true);
    assert!(blocked["result"]["content"][0]["text"]
        .as_str()
        .unwrap()
        .contains("169.254.169.254"));

    assert_eq!(by_id(&frames, 2)["result"]["isError"], true);
    // An unimplemented method is a protocol error, not a tool error.
    assert_eq!(by_id(&frames, 3)["error"]["code"], -32601);
}

#[test]
fn mcp_ignores_notifications_and_malformed_frames() {
    let requests = concat!(
        r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
        "\n",
        "this is not json\n",
        "\n",
        r#"{"jsonrpc":"2.0","id":7,"method":"ping"}"#,
        "\n",
    );
    let frames = exchange(requests, &[]);
    assert_eq!(
        frames.len(),
        1,
        "only the ping deserves a reply: {frames:?}"
    );
    assert_eq!(frames[0]["id"], 7);
}

/// The headline fix for the MCP server: it used to await each tool call before
/// reading the next line, so one slow fetch stalled every other request.
#[test]
fn mcp_serves_requests_concurrently() {
    const DELAY: Duration = Duration::from_millis(900);
    const REQUESTS: usize = 3;

    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().expect("addr");
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { return };
            std::thread::spawn(move || {
                let mut buf = [0u8; 2048];
                let _ = stream.read(&mut buf);
                std::thread::sleep(DELAY);
                let body = "<html><body><article><p>slow</p></article></body></html>";
                let _ = stream.write_all(
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\n\
                         Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    )
                    .as_bytes(),
                );
            });
        }
    });

    let requests: String = (1..=REQUESTS)
        .map(|i| {
            format!(
                r#"{{"jsonrpc":"2.0","id":{i},"method":"tools/call","params":{{"name":"fetch","arguments":{{"url":"http://{addr}/"}}}}}}"#
            ) + "\n"
        })
        .collect();

    let started = Instant::now();
    // The server binds loopback, so the guard has to be relaxed for the child.
    let frames = exchange(&requests, &[("WEBFETCH_ALLOW_PRIVATE", "1")]);
    let elapsed = started.elapsed();

    assert_eq!(frames.len(), REQUESTS, "frames: {frames:?}");
    for i in 1..=REQUESTS as i64 {
        let text = by_id(&frames, i)["result"]["content"][0]["text"]
            .as_str()
            .unwrap_or_default();
        assert!(text.contains("slow"), "id {i} did not fetch: {text}");
    }

    // Served one after another this would take REQUESTS * DELAY.
    assert!(
        elapsed < DELAY * 2,
        "took {elapsed:?} for {REQUESTS} requests of {DELAY:?} each — served sequentially"
    );
}

/// Results are rendered text by default: a pretty-printed FetchResult escapes
/// every newline and repeats the reference list the content already carries.
#[test]
fn mcp_returns_rendered_text_unless_json_is_asked_for() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().expect("addr");
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { return };
            std::thread::spawn(move || {
                let mut buf = [0u8; 2048];
                let _ = stream.read(&mut buf);
                let body = "<html><head><title>Doc</title></head><body><article>\
                            <p>See the <a href=\"/api\">API</a>.</p></article></body></html>";
                let _ = stream.write_all(
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\n\
                         Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    )
                    .as_bytes(),
                );
            });
        }
    });

    let requests = format!(
        "{}\n{}\n",
        format_args!(
            r#"{{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{{"name":"fetch","arguments":{{"url":"http://{addr}/"}}}}}}"#
        ),
        format_args!(
            r#"{{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{{"name":"fetch","arguments":{{"url":"http://{addr}/","json":true}}}}}}"#
        ),
    );
    let frames = exchange(&requests, &[("WEBFETCH_ALLOW_PRIVATE", "1")]);

    let rendered = by_id(&frames, 1)["result"]["content"][0]["text"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(rendered.starts_with("Doc\n"), "rendered: {rendered}");
    assert!(rendered.contains("API [1]"), "rendered: {rendered}");
    assert!(
        !rendered.contains("\"token_estimate\""),
        "default output should not be JSON: {rendered}"
    );

    let as_json = by_id(&frames, 2)["result"]["content"][0]["text"]
        .as_str()
        .unwrap()
        .to_string();
    let v: serde_json::Value = serde_json::from_str(&as_json).expect("json:true returns JSON");
    assert_eq!(v["status"], "ok");
    assert_eq!(v["title"], "Doc");
}
