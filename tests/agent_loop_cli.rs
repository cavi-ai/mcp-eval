use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Command, Stdio};

use serde_json::{json, Value};

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_mcpeval")
}

fn demo() -> &'static str {
    env!("CARGO_BIN_EXE_mcpeval-demo")
}

fn home() -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!("mcpeval-agentloop-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&path).unwrap();
    path
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
    (status, serde_json::from_slice(&raw).unwrap())
}

fn call(endpoint: &str, tool: &str, arguments: Value) -> Value {
    let (_, response) = raw_call(
        endpoint,
        &json!({
            "jsonrpc": "2.0",
            "id": 7,
            "method": "tools/call",
            "params": {"name": tool, "arguments": arguments}
        }),
    );
    response
}

#[test]
fn the_agent_loop_is_native_scaffold_then_run_probe() {
    let dir = home();
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

    // tools/list advertises the agent-loop surface.
    let (_, listed) = raw_call(
        &http,
        &json!({"jsonrpc":"2.0","id":1,"method":"tools/list"}),
    );
    let names: Vec<&str> = listed["result"]["tools"]
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
            "scaffold"
        ]
    );

    // Step 1: scaffold the bundled demo server over MCP.
    let scaffolded = call(
        &http,
        "scaffold",
        json!({
            "command": [demo()],
            "server_label": "demo",
            "confirm_read_only": true
        }),
    );
    let manifest: Value =
        serde_json::from_str(scaffolded["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(manifest["version"], 1);
    assert!(manifest["probes"].as_array().unwrap().len() >= 2);

    // Step 2: run the scaffolded manifest back through run_probe.
    let report = call(
        &http,
        "run_probe",
        json!({
            "manifest": manifest,
            "command": [demo()],
            "server_label": "demo"
        }),
    );
    let document = &report["result"]["structuredContent"];
    assert_eq!(document["schema"], "mcpeval.probe-report/v1");
    assert_eq!(document["passed"], true);
    assert!(document["readiness"]["score"].as_u64().unwrap() >= 80);

    // Step 3: a broken server yields a failing report whose text carries
    // the remediation hint.
    let failing = call(
        &http,
        "run_probe",
        json!({
            "manifest": {"version": 1, "probes": [
                {"id": "p", "probe": "pagination", "access": "read_only", "max_pages": 2}
            ]},
            "command": [demo(), "--broken", "duplicate-page"],
            "server_label": "demo"
        }),
    );
    assert_eq!(failing["result"]["structuredContent"]["passed"], false);
    let text = failing["result"]["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("pagination-duplicate-tool"), "{text}");
    assert!(
        text.contains("paginate the catalog without overlap"),
        "{text}"
    );

    // A missing target is a clean tool error, not a hang.
    let invalid = call(&http, "run_probe", json!({"manifest": manifest}));
    assert!(invalid["error"]["message"]
        .as_str()
        .unwrap()
        .contains("command array or a url"));

    server.kill().ok();
    let _ = server.wait();
}
