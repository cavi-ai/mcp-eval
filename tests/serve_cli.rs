use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Command, Stdio};

use serde_json::{json, Value};

const CLEAN: &str = "tests/fixtures/probe_clean_server.py";
const MANIFEST: &str = "tests/fixtures/mcp-eval.manifest.json";

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_mcpeval")
}

fn home() -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!("mcpeval-serve-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&path).unwrap();
    path
}

fn probe_run(dir: &std::path::Path) {
    let output = Command::new(bin())
        .args([
            "probe",
            "--server",
            "fixture",
            "--manifest",
            MANIFEST,
            "--format",
            "json",
        ])
        .args(["--", "python3", CLEAN])
        .env("MCPEVAL_HOME", dir)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// One-shot HTTP POST returning (status, parsed JSON body).
fn raw_call(endpoint: &str, message: &Value) -> (u16, Value) {
    let authority = endpoint
        .trim_start_matches("http://")
        .trim_end_matches("/mcp");
    let mut stream = std::net::TcpStream::connect(authority).unwrap();
    let body = serde_json::to_vec(message).unwrap();
    write!(
        stream,
        "POST /mcp HTTP/1.1\r\nHost: {authority}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    )
    .unwrap();
    stream.write_all(&body).unwrap();
    stream.flush().unwrap();

    let mut reader = BufReader::new(stream);
    let mut status_line = String::new();
    reader.read_line(&mut status_line).unwrap();
    let status: u16 = status_line
        .split_whitespace()
        .nth(1)
        .unwrap()
        .parse()
        .unwrap();
    let mut content_length = 0usize;
    loop {
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        if line == "\r\n" {
            break;
        }
        if let Some(value) = line.to_ascii_lowercase().strip_prefix("content-length:") {
            content_length = value.trim().parse().unwrap();
        }
    }
    let mut raw = vec![0; content_length];
    reader.read_exact(&mut raw).unwrap();
    let parsed: Value = if raw.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&raw).unwrap()
    };
    (status, parsed)
}

/// Start `mcpeval serve` on a free loopback port and wait for its listener.
fn start_serve(dir: &std::path::Path, extra: &[&str]) -> (std::process::Child, u16) {
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let mut server = Command::new(bin())
        .args(["serve", "--listen", &format!("127.0.0.1:{port}")])
        .args(extra)
        .env("MCPEVAL_HOME", dir)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    for _ in 0..50 {
        if std::net::TcpStream::connect(("127.0.0.1", port)).is_ok() {
            return (server, port);
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    server.kill().ok();
    let _ = server.wait();
    panic!("serve listener never came up");
}

/// One POST carrying exactly `headers` (each line CRLF-terminated) plus
/// Content-Length; returns the status code.
fn status_with_headers(port: u16, headers: &str, body: &[u8]) -> u16 {
    let mut stream = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
    write!(
        stream,
        "POST /mcp HTTP/1.1\r\n{headers}Content-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    )
    .unwrap();
    stream.write_all(body).unwrap();
    stream.flush().unwrap();
    let mut status_line = String::new();
    BufReader::new(stream).read_line(&mut status_line).unwrap();
    status_line
        .split_whitespace()
        .nth(1)
        .unwrap()
        .parse()
        .unwrap()
}

fn call(endpoint: &str, tool: &str, arguments: Value) -> (u16, Value) {
    raw_call(
        endpoint,
        &json!({
            "jsonrpc": "2.0",
            "id": 7,
            "method": "tools/call",
            "params": {"name": tool, "arguments": arguments}
        }),
    )
}

#[test]
fn serve_exposes_findings_and_trends_over_streamable_http() {
    let dir = home();
    // Two full-battery runs record two trend points.
    probe_run(&dir);
    probe_run(&dir);

    let (mut server, port) = start_serve(&dir, &[]);
    let http = format!("http://127.0.0.1:{port}/mcp");

    let (status, response) = raw_call(
        &http,
        &json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}),
    );
    assert_eq!(status, 200);
    assert_eq!(response["result"]["serverInfo"]["name"], "mcpeval");

    let (status, _) = raw_call(
        &http,
        &json!({"jsonrpc":"2.0","method":"notifications/initialized","params":{}}),
    );
    assert_eq!(status, 202);

    let (status, response) = raw_call(
        &http,
        &json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}),
    );
    assert_eq!(status, 200);
    let names: Vec<&str> = response["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|tool| tool["name"].as_str().unwrap())
        .collect();
    // Without --allow-spawn the process-launching tools are not listed.
    assert_eq!(
        names,
        [
            "list_findings",
            "get_finding",
            "get_readiness_trends",
            "record_annotation"
        ]
    );
    for tool in ["run_probe", "scaffold"] {
        let (_, denied) = call(
            &http,
            tool,
            json!({"command": ["python3", CLEAN], "manifest": {"version": 1, "probes": []}}),
        );
        let message = denied["error"]["message"].as_str().unwrap();
        assert!(message.contains("--allow-spawn"), "{message}");
    }

    let (_, trends) = call(&http, "get_readiness_trends", json!({}));
    let text = trends["result"]["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("score=100/100 cases=7/7"), "{text}");

    let (_, findings) = call(&http, "list_findings", json!({}));
    let text = findings["result"]["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("no findings"), "{text}");

    let (_, unknown) = call(&http, "nope", json!({}));
    assert!(unknown["error"]["message"]
        .as_str()
        .unwrap()
        .contains("unknown tool"));

    let (_, rejected) = raw_call(
        &http,
        &json!({"jsonrpc":"2.0","id":3,"method":"resources/list"}),
    );
    assert!(rejected["error"]["message"]
        .as_str()
        .unwrap()
        .contains("unknown method"));

    // The write-side tool records an annotation through the same path the
    // CLI command uses: the session is hashed before persistence.
    let (_, recorded) = call(
        &http,
        "record_annotation",
        json!({
            "session": "agent-session-1",
            "seq": 11,
            "kind": "workaround",
            "note": "used list_resource_fallback after list_resources failed"
        }),
    );
    let recorded_text = recorded["result"]["content"][0]["text"].as_str().unwrap();
    assert!(
        recorded_text.contains("recorded workaround"),
        "{recorded_text}"
    );
    let stored = read_annotations(&dir);
    assert_eq!(stored.len(), 1, "exactly one annotation line");
    assert!(stored[0]["session"]
        .as_str()
        .unwrap()
        .starts_with("session:"));
    assert!(stored[0]["session"].as_str().unwrap().len() == 72);
    assert_eq!(stored[0]["seq"], 11);
    assert_eq!(stored[0]["kind"], "workaround");
    assert_eq!(
        stored[0]["note"],
        "used list_resource_fallback after list_resources failed"
    );

    // An unknown kind is rejected with the same message the CLI prints.
    let (_, bad_kind) = call(
        &http,
        "record_annotation",
        json!({
            "session": "s",
            "seq": 1,
            "kind": "made-up-kind",
            "note": "fine"
        }),
    );
    assert!(bad_kind["error"]["message"]
        .as_str()
        .unwrap()
        .contains("unknown annotation kind"));

    // Control characters and overlong notes are rejected.
    let (_, control) = call(
        &http,
        "record_annotation",
        json!({
            "session": "s",
            "seq": 2,
            "kind": "workaround",
            "note": "line one\nline two"
        }),
    );
    assert!(control["error"]["message"]
        .as_str()
        .unwrap()
        .contains("control characters"));
    let long = "x".repeat(241);
    let (_, overlong) = call(
        &http,
        "record_annotation",
        json!({
            "session": "s",
            "seq": 3,
            "kind": "workaround",
            "note": long
        }),
    );
    assert!(overlong["error"]["message"]
        .as_str()
        .unwrap()
        .contains("240 characters"));

    // The store still holds only the one accepted record.
    assert_eq!(read_annotations(&dir).len(), 1);

    server.kill().ok();
    let _ = server.wait();
}

fn read_annotations(dir: &std::path::Path) -> Vec<Value> {
    let mut records = Vec::new();
    for entry in std::fs::read_dir(dir.join("store")).unwrap() {
        let path = entry.unwrap().path();
        let name = path.file_name().unwrap().to_str().unwrap().to_owned();
        if !name.starts_with("annotations-") || !name.ends_with(".jsonl") {
            continue;
        }
        for line in std::fs::read_to_string(&path).unwrap().lines() {
            records.push(serde_json::from_str(line).unwrap());
        }
    }
    records
}

#[test]
fn serve_rejects_requests_a_browser_page_can_forge() {
    let dir = home();
    let (mut server, port) = start_serve(&dir, &[]);
    let body = serde_json::to_vec(&json!({
        "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {}
    }))
    .unwrap();
    let host = format!("Host: 127.0.0.1:{port}\r\n");
    let json_type = "Content-Type: application/json\r\n";
    let cases: Vec<(String, u16)> = vec![
        (format!("{host}{json_type}"), 200),
        (format!("Host: localhost:{port}\r\n{json_type}"), 200),
        (format!("Host: [::1]:{port}\r\n{json_type}"), 200),
        (
            format!("{host}Content-Type: application/json; charset=utf-8\r\n"),
            200,
        ),
        (
            format!("{host}{json_type}Origin: http://localhost:6274\r\n"),
            200,
        ),
        // A page on another origin, or a sandboxed frame or local file.
        (
            format!("{host}{json_type}Origin: https://evil.example\r\n"),
            403,
        ),
        (format!("{host}{json_type}Origin: null\r\n"), 403),
        // DNS rebinding: a hostile name resolved to 127.0.0.1.
        (format!("Host: evil.example:{port}\r\n{json_type}"), 403),
        (
            format!("Host: 127.0.0.1@evil.example:{port}\r\n{json_type}"),
            403,
        ),
        (json_type.to_owned(), 400),
        (
            format!("{host}Host: evil.example:{port}\r\n{json_type}"),
            400,
        ),
        // CORS-safelisted content types a no-cors fetch can send.
        (format!("{host}Content-Type: text/plain\r\n"), 415),
        (
            format!("{host}Content-Type: application/x-www-form-urlencoded\r\n"),
            415,
        ),
        (host.clone(), 415),
    ];
    for (headers, expected) in cases {
        assert_eq!(
            status_with_headers(port, &headers, &body),
            expected,
            "{headers:?}"
        );
    }
    server.kill().ok();
    let _ = server.wait();
}

/// A legitimate request whose body exceeds the header budget, with
/// `Content-Length` sent before other headers: the header limit counts
/// header bytes only, never the body length.
#[test]
fn large_request_body_is_accepted_whatever_the_header_order() {
    let dir = home();
    let (mut server, port) = start_serve(&dir, &[]);
    let mut body = br#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#.to_vec();
    body.resize(64 * 1024, b' ');
    let mut stream = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
    write!(
        stream,
        "POST /mcp HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Length: {}\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n",
        body.len()
    )
    .unwrap();
    stream.write_all(&body).unwrap();
    stream.flush().unwrap();
    let mut response = Vec::new();
    stream.read_to_end(&mut response).unwrap();
    assert!(
        response.starts_with(b"HTTP/1.1 200"),
        "{:?}",
        String::from_utf8_lossy(&response)
    );
    server.kill().ok();
    let _ = server.wait();
}

/// A rejected request with a body larger than the server's header read:
/// the server must consume the body before closing, or the close resets
/// the connection (Linux sends RST when unread input remains) and the
/// client never sees the 400.
#[test]
fn duplicate_header_rejection_is_readable_despite_a_large_body() {
    let dir = home();
    let (mut server, port) = start_serve(&dir, &[]);
    let mut body = br#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#.to_vec();
    body.resize(64 * 1024, b' ');
    let mut stream = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
    // The server may answer and close before the body is sent; a failed
    // write is part of what this test exercises, not an error.
    let _ = write!(
        stream,
        "POST /mcp HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nHost: evil.example:{port}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let _ = stream.write_all(&body);
    let _ = stream.flush();
    std::thread::sleep(std::time::Duration::from_millis(300));
    let mut response = Vec::new();
    stream
        .read_to_end(&mut response)
        .expect("the server must close cleanly after the 400, not reset");
    assert!(
        response.starts_with(b"HTTP/1.1 400"),
        "{:?}",
        String::from_utf8_lossy(&response)
    );
    server.kill().ok();
    let _ = server.wait();
}

/// The request a hostile page sends with `fetch(url, {method: "POST",
/// mode: "no-cors", body})`: cross-origin and `text/plain`. Even with
/// --allow-spawn it must not launch the command it names.
#[cfg(unix)]
#[test]
fn forged_cross_origin_run_probe_never_launches_its_command() {
    let dir = home();
    let marker = dir.join("launched");
    let (mut server, port) = start_serve(&dir, &["--allow-spawn"]);
    let body = serde_json::to_vec(&json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": {
            "name": "run_probe",
            "arguments": {
                "manifest": {"version": 1, "probes": [{
                    "id": "d", "probe": "discovery-cost", "access": "read_only",
                    "max_tools": 50, "max_schema_bytes": 100000
                }]},
                "command": ["sh", "-c", format!("touch {}", marker.display())]
            }
        }
    }))
    .unwrap();
    let status = status_with_headers(
        port,
        &format!(
            "Host: 127.0.0.1:{port}\r\nOrigin: https://evil.example\r\nContent-Type: text/plain\r\n"
        ),
        &body,
    );
    assert_eq!(status, 403);
    let status = status_with_headers(
        port,
        &format!("Host: 127.0.0.1:{port}\r\nContent-Type: text/plain\r\n"),
        &body,
    );
    assert_eq!(status, 415);
    std::thread::sleep(std::time::Duration::from_millis(300));
    assert!(!marker.exists(), "the forged request launched its command");
    server.kill().ok();
    let _ = server.wait();
}
