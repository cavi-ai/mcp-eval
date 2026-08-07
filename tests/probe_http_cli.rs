use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::process::Command;

use serde_json::{json, Value};

const MANIFEST: &str = "tests/fixtures/mcp-eval.manifest.json";

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_mcpeval")
}

fn read_request(stream: &TcpStream) -> Value {
    let mut reader = BufReader::new(stream);
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
    let mut body = vec![0; content_length];
    reader.read_exact(&mut body).unwrap();
    serde_json::from_slice(&body).unwrap()
}

fn respond(stream: &mut TcpStream, sse: bool) {
    let request = read_request(stream);
    if request.get("id").is_none() {
        stream
            .write_all(b"HTTP/1.1 202 Accepted\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
            .unwrap();
        return;
    }
    let result = match request["method"].as_str().unwrap() {
        "initialize" => json!({
            "protocolVersion":"2025-06-18","capabilities":{"tools":{}},
            "serverInfo":{"name":"fixture","version":"1"}
        }),
        "tools/list" => json!({"tools":[{
            "name":"describe_status","inputSchema":{"type":"object","properties":{}}
        }]}),
        "tools/call" => json!({
            "status":"ready","content":[{"type":"text","text":"CANARY raw HTTP response"}]
        }),
        _ => json!({}),
    };
    let response = json!({"jsonrpc":"2.0","id":request["id"],"result":result});
    let (content_type, body) = if sse {
        ("text/event-stream", format!("data: {response}\r\n\r\n"))
    } else {
        ("application/json", response.to_string())
    };
    write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
    .unwrap();
}

fn fixture(sse: bool) -> (String, std::thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = format!("http://{}/mcp", listener.local_addr().unwrap());
    let server = std::thread::spawn(move || {
        for _ in 0..4 {
            let (mut stream, _) = listener.accept().unwrap();
            respond(&mut stream, sse);
        }
    });
    (endpoint, server)
}

fn run(sse: bool) {
    let (endpoint, server) = fixture(sse);
    let home = std::env::temp_dir().join(format!("mcpeval-http-cli-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&home).unwrap();
    let output = Command::new(bin())
        .args([
            "probe",
            "--server",
            "fixture",
            "--manifest",
            MANIFEST,
            "--probe",
            "instruction-fidelity",
            "--url",
            &endpoint,
        ])
        .env("MCPEVAL_HOME", &home)
        .output()
        .unwrap();
    server.join().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("literal-status instruction-fidelity pass attempts=1"));
    assert!(!stdout.contains("CANARY"));
    let stored = std::fs::read_dir(home.join("store"))
        .unwrap()
        .map(|entry| std::fs::read_to_string(entry.unwrap().path()).unwrap())
        .collect::<String>();
    assert!(stored.contains("\"kind\":\"synthetic\""));
    assert!(!stored.contains("CANARY"));
}

#[test]
fn probe_cli_supports_json_and_sse_streamable_http() {
    run(false);
    run(true);
}
