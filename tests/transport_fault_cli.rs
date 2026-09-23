use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::process::{Command, Output};
use std::time::Duration;

use serde_json::{json, Value};

const FAULT: &str = "tests/fixtures/transport_fault_server.py";

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_mcpeval")
}

fn home() -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!("mcpeval-fault-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&path).unwrap();
    path
}

fn case(id: &str, tool: &str, arguments: Value, max_latency_ms: u64) -> Value {
    json!({
        "id": id, "probe": "latency-budget", "tool": tool, "access": "read_only",
        "arguments": arguments, "attempts": 2, "max_latency_ms": max_latency_ms
    })
}

fn probe(manifest: Value, target: &[&str]) -> (Output, Option<Value>) {
    let dir = home();
    let output = probe_in(&dir, manifest, "json", target);
    let report = serde_json::from_slice(&output.stdout).ok();
    (output, report)
}

fn probe_in(dir: &std::path::Path, manifest: Value, format: &str, target: &[&str]) -> Output {
    let path = dir.join("m.json");
    std::fs::write(&path, manifest.to_string()).unwrap();
    Command::new(bin())
        .args([
            "probe",
            "--server",
            "fault",
            "--manifest",
            path.to_str().unwrap(),
            "--format",
            format,
        ])
        .args(target)
        .env("MCPEVAL_HOME", dir)
        .output()
        .unwrap()
}

fn stdio() -> [&'static str; 3] {
    ["--", "python3", FAULT]
}

fn reasons(report: &Value) -> Vec<Value> {
    report["cases"]
        .as_array()
        .unwrap()
        .iter()
        .map(|case| case["reason"].clone())
        .collect()
}

#[test]
fn a_server_crash_mid_battery_still_emits_the_report_and_exits_3() {
    let manifest = json!({"version": 1, "probes": [
        case("before", "ok", json!({}), 600_000),
        case("crashes", "crash", json!({}), 600_000),
        case("after", "ok", json!({}), 600_000),
    ]});
    let (output, report) = probe(manifest, &stdio());
    assert_eq!(
        output.status.code(),
        Some(3),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report = report.expect("a report is emitted despite the crash");
    assert_eq!(
        reasons(&report),
        vec![Value::Null, json!("transport-closed"), Value::Null],
        "the case after the crash runs on a fresh connection"
    );
    assert_eq!(report["passed"], false);
}

#[test]
fn errored_cases_render_as_errors_and_record_no_trend() {
    let dir = home();
    let manifest = json!({"version": 1, "probes": [
        case("before", "ok", json!({}), 600_000),
        case("crashes", "crash", json!({}), 600_000),
    ]});
    let text = probe_in(&dir, manifest.clone(), "text", &stdio());
    assert_eq!(text.status.code(), Some(3));
    let stdout = String::from_utf8(text.stdout).unwrap();
    assert!(
        stdout.contains("crashes latency-budget error reason=transport-closed"),
        "{stdout}"
    );
    assert!(
        !dir.join("store")
            .join("probes")
            .join("history.jsonl")
            .exists(),
        "an incomplete battery records no trend point"
    );

    let json = probe_in(&dir, manifest, "json", &stdio());
    std::fs::write(dir.join("report.json"), &json.stdout).unwrap();
    let markdown = Command::new(bin())
        .args(["report", "report.json", "--format", "markdown"])
        .current_dir(&dir)
        .env("MCPEVAL_HOME", &dir)
        .output()
        .unwrap();
    assert_eq!(
        markdown.status.code(),
        Some(3),
        "re-rendering keeps the exit"
    );
    let body = String::from_utf8(markdown.stdout).unwrap();
    assert!(
        body.contains("| crashes | latency-budget | error | 0 | — | transport-closed |"),
        "{body}"
    );
}

#[test]
fn a_call_past_the_manifest_timeout_errors_that_case_only() {
    let manifest = json!({"version": 1, "timeout_ms": 300, "probes": [
        {"id": "hangs", "probe": "degradation-over-n", "tool": "slow", "access": "read_only",
         "arguments": {"ms": 3000}, "max_attempts": 2},
        case("after", "ok", json!({}), 600_000),
    ]});
    let (output, report) = probe(manifest, &stdio());
    assert_eq!(
        output.status.code(),
        Some(3),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report = report.expect("a report is emitted despite the timeout");
    assert_eq!(
        reasons(&report),
        vec![json!("transport-timeout"), Value::Null]
    );
}

#[test]
fn a_latency_budget_above_the_timeout_is_measured_not_errored() {
    let manifest = json!({"version": 1, "timeout_ms": 1000, "probes": [
        case("slow-call", "slow", json!({"ms": 1500}), 1000),
    ]});
    let (output, report) = probe(manifest, &stdio());
    assert_eq!(
        output.status.code(),
        Some(1),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report = report.unwrap();
    assert_eq!(reasons(&report), vec![json!("latency-budget-exceeded")]);
    assert!(
        report["cases"][0]["measurements"]["latency_ms"]
            .as_u64()
            .unwrap()
            >= 1000
    );
}

#[test]
fn the_manifest_timeout_bounds_http_calls_too() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/mcp", listener.local_addr().unwrap());
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(stream) = stream else { break };
            std::thread::spawn(move || serve_http(stream));
        }
    });
    let manifest = json!({"version": 1, "timeout_ms": 300, "probes": [
        {"id": "hangs", "probe": "degradation-over-n", "tool": "slow", "access": "read_only",
         "arguments": {}, "max_attempts": 2},
        case("after", "ok", json!({}), 600_000),
    ]});
    let (output, report) = probe(manifest, &["--url", &url]);
    assert_eq!(
        output.status.code(),
        Some(3),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        reasons(&report.unwrap()),
        vec![json!("transport-timeout"), Value::Null]
    );
}

#[test]
fn compare_exits_3_when_a_column_could_not_be_evaluated() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/mcp", listener.local_addr().unwrap());
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(stream) = stream else { break };
            std::thread::spawn(move || serve_http(stream));
        }
    });
    let dir = home();
    let path = dir.join("m.json");
    std::fs::write(
        &path,
        json!({"version": 1, "probes": [case("crashes", "crash", json!({}), 600_000)]}).to_string(),
    )
    .unwrap();
    let output = Command::new(bin())
        .args(["compare", "--server", "fault", "--manifest"])
        .arg(&path)
        .args(["--endpoint", &format!("http={url}")])
        .args(stdio())
        .env("MCPEVAL_HOME", &dir)
        .output()
        .unwrap();
    assert_eq!(
        output.status.code(),
        Some(3),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("error(transport-closed)"), "{stdout}");
}

fn serve_http(mut stream: TcpStream) {
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut content_length = 0usize;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).unwrap_or(0) == 0 {
            return;
        }
        if line == "\r\n" {
            break;
        }
        if let Some(value) = line.to_ascii_lowercase().strip_prefix("content-length:") {
            content_length = value.trim().parse().unwrap();
        }
    }
    let mut body = vec![0; content_length];
    if reader.read_exact(&mut body).is_err() {
        return;
    }
    let request: Value = serde_json::from_slice(&body).unwrap();
    if request.get("id").is_none() {
        let _ = stream
            .write_all(b"HTTP/1.1 202 Accepted\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
        return;
    }
    let result = match request["method"].as_str().unwrap() {
        "initialize" => json!({
            "protocolVersion": "2025-06-18", "capabilities": {"tools": {}},
            "serverInfo": {"name": "fault", "version": "1"}
        }),
        "tools/list" => json!({"tools": [
            {"name": "ok", "inputSchema": {"type": "object", "properties": {}}},
            {"name": "slow", "inputSchema": {"type": "object", "properties": {}}},
            {"name": "crash", "inputSchema": {"type": "object", "properties": {}}}
        ]}),
        "tools/call" => {
            if request["params"]["name"] == "slow" {
                std::thread::sleep(Duration::from_millis(2000));
            }
            json!({"content": [{"type": "text", "text": "done"}]})
        }
        _ => json!({}),
    };
    let body = json!({"jsonrpc": "2.0", "id": request["id"], "result": result}).to_string();
    let _ = write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
}

#[test]
fn usage_errors_exit_2_and_unreachable_servers_exit_3() {
    let (invalid, _) = probe(json!({"version": 2, "probes": []}), &stdio());
    assert_eq!(invalid.status.code(), Some(2));

    let (bad_timeout, _) = probe(
        json!({"version": 1, "timeout_ms": 5, "probes": [case("a", "ok", json!({}), 10)]}),
        &stdio(),
    );
    assert_eq!(bad_timeout.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&bad_timeout.stderr).contains("timeout_ms"));

    let (unknown_tool, _) = probe(
        json!({"version": 1, "probes": [case("a", "missing", json!({}), 10)]}),
        &stdio(),
    );
    assert_eq!(unknown_tool.status.code(), Some(2));

    let (unreachable, _) = probe(
        json!({"version": 1, "probes": [case("a", "ok", json!({}), 10)]}),
        &["--", "/nonexistent/mcpeval-no-such-server"],
    );
    assert_eq!(unreachable.status.code(), Some(3));
}

#[test]
fn a_stalled_call_judged_as_a_verdict_does_not_poison_the_next_case() {
    let manifest = json!({"version": 1, "timeout_ms": 300, "probes": [
        {"id": "samples", "probe": "sampling", "tool": "slow", "access": "read_only",
         "arguments": {"ms": 1500}, "max_requests": 1},
        {"id": "elicits", "probe": "elicitation", "tool": "slow", "access": "read_only",
         "arguments": {"ms": 1500}, "max_requests": 1, "respond": "accept"},
        case("after", "ok", json!({}), 600_000),
    ]});
    let (output, report) = probe(manifest, &stdio());
    assert_eq!(
        output.status.code(),
        Some(1),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        reasons(&report.unwrap()),
        vec![
            json!("sampling-stalled-call"),
            json!("elicitation-stalled-call"),
            Value::Null
        ]
    );
}

#[test]
fn an_errored_case_still_reports_when_stderr_is_closed() {
    let dir = home();
    let path = dir.join("m.json");
    std::fs::write(
        &path,
        json!({"version": 1, "probes": [case("crashes", "crash", json!({}), 600_000)]}).to_string(),
    )
    .unwrap();
    let mut child = Command::new(bin())
        .args(["probe", "--server", "fault", "--manifest"])
        .arg(&path)
        .args(["--format", "json"])
        .args(stdio())
        .env("MCPEVAL_HOME", &dir)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    drop(child.stderr.take());
    let output = child.wait_with_output().unwrap();
    assert_eq!(output.status.code(), Some(3));
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(reasons(&report), vec![json!("transport-closed")]);
}
