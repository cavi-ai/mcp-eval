use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::process::Command;

use serde_json::{json, Value};

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

/// One-request-per-connection Streamable HTTP fixture. `fail_calls` makes
/// tools/call return a JSON-RPC error so the two endpoints diverge.
fn respond(stream: &mut TcpStream, fail_calls: bool) {
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
        "tools/call" => {
            if fail_calls {
                let response = json!({
                    "jsonrpc":"2.0","id":request["id"],
                    "error":{"code":-32001,"message":"CANARY raw failure"}
                });
                let body = response.to_string();
                write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                )
                .unwrap();
                return;
            }
            json!({"status":"ready","content":[{"type":"text","text":"CANARY raw HTTP response"}]})
        }
        _ => json!({}),
    };
    let response = json!({"jsonrpc":"2.0","id":request["id"],"result":result});
    let body = response.to_string();
    write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
    .unwrap();
}

fn fixture(fail_calls: bool, requests: usize) -> (String, std::thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = format!("http://{}/mcp", listener.local_addr().unwrap());
    let server = std::thread::spawn(move || {
        for _ in 0..requests {
            let Ok((mut stream, _)) = listener.accept() else {
                break;
            };
            respond(&mut stream, fail_calls);
        }
    });
    (endpoint, server)
}

fn manifest(dir: &std::path::Path) -> String {
    let path = dir.join("compare.manifest.json");
    std::fs::write(
        &path,
        r#"{
          "version": 1,
          "probes": [
            {"id":"naive-status","probe":"schema-guessability","tool":"describe_status","access":"read_only","arguments":{}},
            {"id":"literal-status","probe":"instruction-fidelity","tool":"describe_status","access":"read_only","arguments":{},
             "expect":{"outcome":"ok","required_result_fields":["status"]}}
          ]
        }"#,
    )
    .unwrap();
    path.to_string_lossy().into_owned()
}

#[test]
fn compare_diffs_two_http_endpoints_across_formats() {
    // Each `compare` invocation makes initialize, initialized notification,
    // tools/list, and two tools/call requests per endpoint. The test runs four
    // comparisons, including the final JSON assertion.
    let (good, good_server) = fixture(false, 20);
    let (bad, bad_server) = fixture(true, 20);
    let dir = std::env::temp_dir().join(format!("mcpeval-compare-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let home = dir.join("home");
    std::fs::create_dir_all(&home).unwrap();
    let manifest_path = manifest(&dir);

    for (format, needle) in [
        ("text", "readiness"),
        ("markdown", "# mcp-eval comparison — fixture"),
        ("json", "\"endpoint\""),
    ] {
        let output = Command::new(bin())
            .args([
                "compare",
                "--server",
                "fixture",
                "--manifest",
                &manifest_path,
                "--format",
                format,
                "--endpoint",
                &format!("good={good}"),
                "--endpoint",
                &format!("regressed={bad}"),
            ])
            .env("MCPEVAL_HOME", &home)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{format}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = String::from_utf8(output.stdout).unwrap();
        assert!(stdout.contains(needle), "{format}: {stdout}");
        assert!(stdout.contains("pass"));
        assert!(stdout.contains("unexpected-outcome"));
        // Comparison is informational: failures do not gate.
        assert!(!stdout.contains("CANARY"));
    }

    // The JSON variant parses as an array of per-endpoint reports.
    let output = Command::new(bin())
        .args([
            "compare",
            "--server",
            "fixture",
            "--manifest",
            &manifest_path,
            "--format",
            "json",
            "--endpoint",
            &format!("a={good}"),
            "--endpoint",
            &format!("b={bad}"),
        ])
        .env("MCPEVAL_HOME", &home)
        .output()
        .unwrap();
    let documents: Vec<Value> = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(documents.len(), 2);
    assert_eq!(documents[0]["endpoint"], "a");
    assert_eq!(documents[1]["endpoint"], "b");
    assert_eq!(documents[0]["report"]["passed"], true);
    assert_eq!(documents[1]["report"]["passed"], false);
    good_server.join().unwrap();
    bad_server.join().unwrap();
}

#[test]
fn compare_requires_two_endpoints_and_unique_labels() {
    let dir = std::env::temp_dir().join(format!("mcpeval-compare-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let home = dir.join("home");
    std::fs::create_dir_all(&home).unwrap();
    let manifest_path = manifest(&dir);

    let one = Command::new(bin())
        .args([
            "compare",
            "--server",
            "fixture",
            "--manifest",
            &manifest_path,
            "--endpoint",
            "solo=http://127.0.0.1:9/mcp",
        ])
        .env("MCPEVAL_HOME", &home)
        .output()
        .unwrap();
    assert!(!one.status.success());

    let (endpoint, server) = fixture(false, 0);
    let duplicate = Command::new(bin())
        .args([
            "compare",
            "--server",
            "fixture",
            "--manifest",
            &manifest_path,
            "--endpoint",
            &format!("same={endpoint}"),
            "--endpoint",
            &format!("same={endpoint}"),
        ])
        .env("MCPEVAL_HOME", &home)
        .output()
        .unwrap();
    assert!(!duplicate.status.success());
    server.join().unwrap();
}

#[test]
fn compare_accepts_a_stdio_command_alongside_endpoints() {
    let dir = std::env::temp_dir().join(format!("mcpeval-compare-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let home = dir.join("home");
    std::fs::create_dir_all(&home).unwrap();
    let manifest_path = manifest(&dir);

    // One stdio command alone is not a comparison.
    let solo = Command::new(bin())
        .args(["compare", "--server", "demo", "--manifest", &manifest_path])
        .args(["--", env!("CARGO_BIN_EXE_mcpeval-demo")])
        .env("MCPEVAL_HOME", &home)
        .output()
        .unwrap();
    assert!(!solo.status.success());
    let stderr = String::from_utf8_lossy(&solo.stderr);
    assert!(stderr.contains("at least two targets"), "{stderr}");

    // An endpoint plus the stdio command produces a two-column grid whose
    // stdio column reflects the demo server.
    let (endpoint, server) = fixture(false, 5);
    let output = Command::new(bin())
        .args([
            "compare",
            "--server",
            "demo",
            "--manifest",
            &manifest_path,
            "--endpoint",
            &format!("http-target={endpoint}"),
        ])
        .args(["--", env!("CARGO_BIN_EXE_mcpeval-demo")])
        .env("MCPEVAL_HOME", &home)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("stdio"), "{stdout}");
    assert!(stdout.contains("http-target"), "{stdout}");
    assert!(stdout.contains("readiness"), "{stdout}");
    server.join().unwrap();
}
