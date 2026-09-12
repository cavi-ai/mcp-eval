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

    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let mut server = Command::new(bin())
        .args(["serve", "--listen", &format!("127.0.0.1:{port}")])
        .env("MCPEVAL_HOME", &dir)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let http = format!("http://127.0.0.1:{port}/mcp");
    let mut up = false;
    for _ in 0..50 {
        if std::net::TcpStream::connect(("127.0.0.1", port)).is_ok() {
            up = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    assert!(up, "serve listener never came up");

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
    assert_eq!(
        names,
        [
            "list_findings",
            "get_finding",
            "get_readiness_trends",
            "run_probe",
            "scaffold",
            "record_annotation"
        ]
    );

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
