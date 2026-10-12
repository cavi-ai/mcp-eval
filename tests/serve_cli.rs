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
    stream
        .set_read_timeout(Some(std::time::Duration::from_secs(10)))
        .unwrap();
    stream
        .set_write_timeout(Some(std::time::Duration::from_secs(10)))
        .unwrap();
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

/// A running `mcpeval serve`, killed when dropped so a failing assertion
/// never leaks the process.
struct Serve(std::process::Child);

impl Drop for Serve {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

impl std::ops::Deref for Serve {
    type Target = std::process::Child;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl std::ops::DerefMut for Serve {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

/// Start `mcpeval serve` on a free loopback port. The port is released
/// before serve binds it, so a parallel test can take it first: wait for
/// this child's own bind announcement, and retry on a fresh port when it
/// exits instead.
fn start_serve(dir: &std::path::Path, extra: &[&str]) -> (Serve, u16) {
    for _ in 0..5 {
        let port = std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let mut server = Serve(
            Command::new(bin())
                .args(["serve", "--listen", &format!("127.0.0.1:{port}")])
                .args(extra)
                .env("MCPEVAL_HOME", dir)
                .stdout(Stdio::null())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap(),
        );
        if announced(&mut server, port) {
            return (server, port);
        }
    }
    panic!("serve could not bind a free loopback port in five attempts");
}

/// Whether serve announced its listener on `port` (false: it exited first).
/// The rest of stderr is drained so serve never blocks on a full pipe.
fn announced(server: &mut Serve, port: u16) -> bool {
    let stderr = server.stderr.take().unwrap();
    let listening = format!("http://127.0.0.1:{port}/mcp");
    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut lines = BufReader::new(stderr).lines();
        let first = lines.next().and_then(Result::ok).unwrap_or_default();
        let _ = sender.send(first.contains(&listening));
        lines.for_each(drop);
    });
    receiver
        .recv_timeout(std::time::Duration::from_secs(30))
        .expect("serve never announced its listener")
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
fn findings_queries_report_unavailable_evidence_without_private_diagnostics() {
    let dir = home();
    let (_server, port) = start_serve(&dir, &[]);
    let endpoint = format!("http://127.0.0.1:{port}/mcp");
    for database in [None, Some("PRIVATE corrupt database bytes"), Some("")] {
        if let Some(bytes) = database {
            std::fs::write(dir.join("index.db"), bytes).unwrap();
        }
        for (tool, arguments) in [
            ("list_findings", json!({})),
            ("get_finding", json!({"finding_id":"finding-unknown"})),
        ] {
            let (status, response) = call(&endpoint, tool, arguments);
            assert_eq!(status, 200);
            assert!(response.get("error").is_some(), "{response}");
            assert_eq!(
                response["error"]["message"],
                "findings unavailable; run `mcpeval index` and `mcpeval promote` and retry"
            );
            assert!(!response.to_string().contains("PRIVATE"));
            assert!(!response.to_string().contains(dir.to_str().unwrap()));
        }
    }
}

#[test]
fn invalid_tool_arguments_are_refused_with_32602_naming_the_path_without_values() {
    let dir = home();
    let (_server, port) = start_serve(&dir, &[]);
    let endpoint = format!("http://127.0.0.1:{port}/mcp");
    let mut messages = Vec::new();
    for (tool, arguments, needle, canary) in [
        ("list_findings", json!({"state": 5}), "/state", false),
        ("get_finding", json!({}), "finding_id", false),
        (
            "list_findings",
            json!({"state": {"k": "CANARY_7Q"}}),
            "/state",
            true,
        ),
        (
            "get_finding",
            json!({"finding_id": ["CANARY_7Q"]}),
            "/finding_id",
            true,
        ),
        (
            "get_finding",
            json!({"finding_id": 5, "extra": "CANARY_7Q"}),
            "/finding_id",
            true,
        ),
    ] {
        let (status, response) = call(&endpoint, tool, arguments);
        assert_eq!(status, 200);
        assert_eq!(response["error"]["code"], -32602, "{response}");
        let message = response["error"]["message"].as_str().unwrap().to_owned();
        assert!(message.contains(needle), "{response}");
        if canary {
            assert!(!response.to_string().contains("CANARY_7Q"), "{response}");
        }
        messages.push(message);
    }
    for message in &messages {
        assert!(message.chars().count() <= 240, "{message}");
    }
    let other = home();
    promoted_finding(&other);
    let (_loaded, port) = start_serve(&other, &[]);
    let (status, response) = call(
        &format!("http://127.0.0.1:{port}/mcp"),
        "get_finding",
        json!({"finding_id": "finding-0000000000000000"}),
    );
    assert_eq!(status, 200);
    assert_eq!(response["error"]["code"], -32000, "{response}");
    assert_eq!(
        response["error"]["message"], "no such finding",
        "{response}"
    );
}

#[test]
fn serve_exposes_findings_and_trends_over_streamable_http() {
    let dir = home();
    // Two full-battery runs record two trend points.
    probe_run(&dir);
    probe_run(&dir);

    let (_server, port) = start_serve(&dir, &[]);
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
    for tool in ["run_probe", "scaffold", "score", "verify_finding"] {
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
    assert!(text.contains(" cases=7/7 manifest="), "{text}");
    let points = trends["result"]["structuredContent"]["points"]
        .as_array()
        .unwrap_or_else(|| panic!("no structured points: {trends}"));
    assert_eq!(points.len(), 2, "{trends}");
    assert!(
        points[0]["ts"].as_str() < points[1]["ts"].as_str(),
        "oldest first: {trends}"
    );
    let manifest = points[1]["manifest_sha256"].as_str().unwrap();
    assert_eq!(manifest.len(), 64, "{trends}");
    assert_eq!(points[0]["manifest_sha256"], points[1]["manifest_sha256"]);
    assert!(
        text.lines().nth(1).is_some_and(
            |line| line.ends_with(&format!(" cases=7/7 +0 manifest={}", &manifest[..8]))
        ),
        "{text}"
    );

    mcpeval::index::build(&dir).unwrap();
    mcpeval::promote::promote(
        &dir,
        mcpeval::promote::PromotionConfig {
            threshold: 0.0,
            now: chrono::Utc::now(),
        },
    )
    .unwrap();
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
}

#[test]
fn event_annotations_match_advertised_schema_and_link_only_the_named_call() {
    let dir = home();
    let mut store = mcpeval::store::Store::open(Some(dir.clone())).unwrap();
    let mut events = Vec::new();
    for server in ["first", "second"] {
        let mut recorder = mcpeval::correlate::Correlator::new(
            server.into(),
            "shared-session".into(),
            mcpeval::fingerprint::Salt::for_tests(),
        );
        recorder.on_outbound(
            &json!({"id":1,"method":"tools/call","params":{"name":"lookup"}}),
            0,
        );
        let record = recorder
            .on_inbound(&json!({"id":1,"result":{}}), 1)
            .unwrap();
        events.push(record.identity.as_ref().unwrap().event_id);
        store.append(&record).unwrap();
    }
    let (_server, port) = start_serve(&dir, &[]);
    let http = format!("http://127.0.0.1:{port}/mcp");
    let (_, catalog) = raw_call(
        &http,
        &json!({"jsonrpc":"2.0","id":1,"method":"tools/list"}),
    );
    let schema = &catalog["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|tool| tool["name"] == "record_annotation")
        .unwrap()["inputSchema"];
    let args = json!({"event_id":events[0],"kind":"false-success","note":"did not change state"});
    let validator = jsonschema::options()
        .should_validate_formats(true)
        .build(schema)
        .unwrap();
    assert!(validator.is_valid(&args));
    let (_, result) = call(&http, "record_annotation", args);
    assert!(result.get("error").is_none(), "{result}");
    let stored = read_annotations(&dir);
    assert_eq!(stored.len(), 1);
    assert_eq!(stored[0]["event_id"], events[0].to_string());
    assert!(stored[0].get("session").is_none());
    assert!(stored[0].get("seq").is_none());
    for invalid in [
        json!({"event_id":events[0],"session":"s","seq":1}),
        json!({"event_id":events[0],"session":null}),
        json!({"event_id":null,"session":"s","seq":1}),
        json!({"session":"s","seq":null}),
        json!({"event_id":"not-a-uuid"}),
        json!({"session":"s","seq":1,"extra":true}),
        json!({"session":"s","seq":-1}),
    ] {
        let mut invalid = invalid;
        invalid["kind"] = json!("workaround");
        invalid["note"] = json!("n");
        assert!(!validator.is_valid(&invalid), "{invalid}");
        let (_, result) = call(&http, "record_annotation", invalid);
        assert!(result.get("error").is_some(), "{result}");
    }
    assert_eq!(read_annotations(&dir).len(), 1);
    mcpeval::index::build(&dir).unwrap();
    let db = rusqlite::Connection::open(dir.join("index.db")).unwrap();
    let target: String = db
        .query_row(
            "SELECT c.event_id FROM annotations a JOIN calls c ON a.call_id=c.id",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(target, events[0].to_string());
}

fn promoted_finding(dir: &std::path::Path) -> String {
    let mut store = mcpeval::store::Store::open(Some(dir.to_owned())).unwrap();
    for session in ["first", "second"] {
        store
            .append(&mcpeval::record::CallRecord {
                identity: None,
                ts: "2026-08-05T00:00:01Z".into(),
                session: session.into(),
                seq: 1,
                server: "fixture".into(),
                method: "tools/call".into(),
                tool: Some("describe_status".into()),
                args: Some(json!({})),
                latency_ms: Some(1),
                outcome: "error".into(),
                error: Some(mcpeval::record::ErrorInfo {
                    code: Some(json!("broken")),
                    layer: None,
                    retryable: Some(false),
                    kind: None,
                    template: None,
                    template_id: Some("aaaaaaaaaaaaaaaa".into()),
                }),
                shim_self_us: 1,
                kind: "real".into(),
            })
            .unwrap();
    }
    mcpeval::index::build(dir).unwrap();
    mcpeval::promote::promote(
        dir,
        mcpeval::promote::PromotionConfig {
            threshold: 0.0,
            now: chrono::Utc::now(),
        },
    )
    .unwrap();
    let db = rusqlite::Connection::open(dir.join("index.db")).unwrap();
    db.query_row("SELECT finding_id FROM findings", [], |row| row.get(0))
        .unwrap()
}

#[test]
fn queries_remain_responsive_and_overlapping_evaluations_are_refused() {
    let dir = home();
    let _store = mcpeval::store::Store::open(Some(dir.clone())).unwrap();
    mcpeval::index::build(&dir).unwrap();
    mcpeval::promote::promote(
        &dir,
        mcpeval::promote::PromotionConfig {
            threshold: 0.0,
            now: chrono::Utc::now(),
        },
    )
    .unwrap();
    let marker = dir.join("evaluation-started");
    let (_server, port) = start_serve(&dir, &["--allow-spawn"]);
    let http = format!("http://127.0.0.1:{port}/mcp");
    let slow = json!({"manifest": {"version": 1, "timeout_ms": 5000, "probes": [{
        "id": "slow-case", "probe": "instruction-fidelity", "access": "read_only", "tool": "slow",
        "arguments": {"ms": 3500, "marker": marker}, "expect": {"outcome": "ok"}
    }]}, "command": ["python3", "tests/fixtures/transport_fault_server.py"]});
    let endpoint = http.clone();
    let arguments = slow.clone();
    let job = std::thread::spawn(move || call(&endpoint, "run_probe", arguments));
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while !marker.exists() && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(marker.exists(), "evaluation never started");
    let start = std::time::Instant::now();
    let (_, findings) = call(&http, "list_findings", json!({}));
    assert!(findings.get("result").is_some(), "{findings}");
    assert!(
        start.elapsed() < std::time::Duration::from_secs(2),
        "query waited for the evaluation"
    );
    let (_, overlapping) = call(&http, "run_probe", slow.clone());
    assert!(
        overlapping["error"]["message"]
            .as_str()
            .unwrap()
            .contains("evaluation already running"),
        "{overlapping}"
    );
    let (_, report) = job.join().unwrap();
    assert_eq!(
        report["result"]["structuredContent"]["passed"], true,
        "{report}"
    );
    let mut fast = slow;
    fast["manifest"]["probes"][0]["arguments"]["ms"] = json!(0);
    let (_, next) = call(&http, "run_probe", fast);
    assert_eq!(
        next["result"]["structuredContent"]["passed"], true,
        "permit was not released: {next}"
    );
}

#[test]
fn shipped_service_passes_ping_and_exercises_its_read_only_contract_over_http() {
    let dir = home();
    let (_server, port) = start_serve(&dir, &[]);
    let http = format!("http://127.0.0.1:{port}/mcp");
    let (_, ping) = raw_call(&http, &json!({"jsonrpc": "2.0", "id": 1, "method": "ping"}));
    assert_eq!(ping["result"], json!({}), "{ping}");
    let output = Command::new(bin())
        .args([
            "score", "--server", "mcpeval", "--format", "json", "--url", &http,
        ])
        .env("MCPEVAL_HOME", &dir)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    let stdio = Command::new(bin())
        .args([
            "score",
            "--server",
            "mcpeval",
            "--format",
            "json",
            "--",
            "python3",
            "tests/fixtures/stdio_http_bridge.py",
            &http,
        ])
        .env("MCPEVAL_HOME", &dir)
        .output()
        .unwrap();
    assert!(
        stdio.status.success(),
        "{}",
        String::from_utf8_lossy(&stdio.stderr)
    );
    let bridged: Value = serde_json::from_slice(&stdio.stdout).unwrap();
    let contract = |document: &Value| -> Vec<Value> {
        document["readiness"]["areas"]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|area| {
                area["checks"].as_array().unwrap().iter().map(|check| {
                json!({"id": check["id"], "reason": check["reason"], "tool": check["tool"]})
            })
            })
            .collect()
    };
    assert_eq!(
        contract(&report),
        contract(&bridged),
        "HTTP and stdio disagree on the same service contract"
    );
    assert_eq!(
        report["readiness"]["surface"],
        bridged["readiness"]["surface"]
    );
    assert_eq!(report["readiness"]["surface"]["read_only"], 3, "{report}");
    for area in report["readiness"]["areas"].as_array().unwrap() {
        for check in area["checks"].as_array().unwrap() {
            assert!(
                !matches!(
                    check["reason"].as_str(),
                    Some(
                        "protocol-ping-failed"
                            | "catalog-no-output-schema"
                            | "reliability-output-schema-broken"
                            | "honesty-wrong-code"
                    )
                ),
                "{check}"
            );
        }
    }
    let (_, invalid) = call(&http, "list_findings", json!({"state": 42}));
    assert!(
        invalid.get("error").is_some(),
        "invalid declared input was accepted: {invalid}"
    );
    assert_eq!(invalid["error"]["code"], -32602, "{invalid}");
    let honesty = report["readiness"]["areas"]
        .as_array()
        .unwrap()
        .iter()
        .find(|area| area["name"] == "error-honesty")
        .unwrap_or_else(|| panic!("error-honesty area missing: {report}"));
    assert_eq!(honesty["score"], 100, "{honesty}");
}

#[test]
fn slow_body_uploads_cannot_extend_the_request_deadline() {
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    };
    let dir = home();
    let (_server, port) = start_serve(&dir, &[]);
    let mut stream = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
    stream
        .set_read_timeout(Some(std::time::Duration::from_secs(7)))
        .unwrap();
    write!(stream, "POST /mcp HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Type: application/json\r\nContent-Length: 1000\r\n\r\n").unwrap();
    let mut writer = stream.try_clone().unwrap();
    let stop = Arc::new(AtomicBool::new(false));
    let stopped = Arc::clone(&stop);
    let upload = std::thread::spawn(move || {
        for _ in 0..20 {
            if stopped.load(Ordering::Acquire) || writer.write_all(b" ").is_err() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(500));
        }
    });
    let start = std::time::Instant::now();
    let mut response = Vec::new();
    let ended = stream.read_to_end(&mut response);
    stop.store(true, Ordering::Release);
    upload.join().unwrap();
    assert!(
        !matches!(ended, Err(ref error) if matches!(error.kind(), std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock)),
        "upload kept the worker indefinitely"
    );
    assert!(
        start.elapsed() < std::time::Duration::from_secs(7),
        "request exceeded its whole-request deadline"
    );
}

#[test]
fn advertised_output_schemas_validate_real_results_and_annotations_do_not_authorize_spawning() {
    let dir = home();
    let id = promoted_finding(&dir);
    let (_server, port) = start_serve(&dir, &["--allow-spawn"]);
    let http = format!("http://127.0.0.1:{port}/mcp");
    let (_, catalog) = raw_call(
        &http,
        &json!({"jsonrpc":"2.0","id":1,"method":"tools/list"}),
    );
    let manifest: Value = serde_json::from_slice(&std::fs::read(MANIFEST).unwrap()).unwrap();
    let calls = [
        ("list_findings", json!({})),
        ("get_finding", json!({"finding_id": id})),
        ("get_readiness_trends", json!({})),
        (
            "record_annotation",
            json!({"session":"s", "seq":1,"kind":"workaround","note":"safe"}),
        ),
        (
            "run_probe",
            json!({"manifest": {"version":1,"probes":[manifest["probes"][6].clone()]},"command":["python3",CLEAN]}),
        ),
        ("scaffold", json!({"command":["python3",CLEAN]})),
        ("score", json!({"command":["python3",CLEAN]})),
        (
            "verify_finding",
            json!({"finding_id":id,"case_id":"literal-status","manifest":manifest,"command":["python3",CLEAN]}),
        ),
    ];
    for (name, arguments) in calls {
        let tool = catalog["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .find(|tool| tool["name"] == name)
            .unwrap();
        let read_only = matches!(
            name,
            "list_findings" | "get_finding" | "get_readiness_trends"
        );
        assert_eq!(tool["annotations"]["readOnlyHint"], read_only, "{tool}");
        assert_eq!(
            tool["annotations"]["destructiveHint"],
            !read_only && name != "record_annotation",
            "{tool}"
        );
        assert!(
            tool["outputSchema"].is_object(),
            "no result contract: {tool}"
        );
        let validator = jsonschema::validator_for(&tool["outputSchema"]).unwrap();
        let (_, result) = call(&http, name, arguments);
        let document = &result["result"]["structuredContent"];
        assert!(
            validator.is_valid(document),
            "{name} does not satisfy its output schema: {result}"
        );
        assert!(
            !validator.is_valid(&json!({})),
            "{name} advertises an empty contract"
        );
    }
}

#[test]
fn native_verification_shares_cli_credit_and_preserves_transport_failure_state() {
    let dir = home();
    let id = promoted_finding(&dir);
    let (_server, port) = start_serve(&dir, &["--allow-spawn"]);
    let http = format!("http://127.0.0.1:{port}/mcp");
    let manifest: Value = serde_json::from_slice(&std::fs::read(MANIFEST).unwrap()).unwrap();
    let args = json!({"finding_id": id, "case_id": "literal-status", "manifest": manifest,
        "command": ["python3", CLEAN]});
    let (_, first) = call(&http, "verify_finding", args.clone());
    let result = &first["result"]["structuredContent"];
    assert_eq!(result["verified"], true, "{first}");
    assert_eq!(result["lifecycle"]["consecutive_passes"], 1);
    assert_eq!(result["report"]["cases"].as_array().unwrap().len(), 1);
    assert!(!first.to_string().contains("CANARY-manifest-argument"));

    let cli = Command::new(bin())
        .args([
            "verify",
            "--finding",
            &id,
            "--case",
            "literal-status",
            "--manifest",
            MANIFEST,
            "--",
            "python3",
            CLEAN,
        ])
        .env("MCPEVAL_HOME", &dir)
        .output()
        .unwrap();
    assert!(
        cli.status.success(),
        "{}",
        String::from_utf8_lossy(&cli.stderr)
    );
    assert!(String::from_utf8_lossy(&cli.stdout).contains("consecutive_passes=2"));
    let (_, third) = call(&http, "verify_finding", args.clone());
    assert_eq!(
        third["result"]["structuredContent"]["lifecycle"]["state"], "closed",
        "{third}"
    );

    let mut unavailable = args.clone();
    unavailable["command"] = json!(["python3", CLEAN, "early-exit"]);
    let (_, startup) = call(&http, "verify_finding", unavailable.clone());
    assert!(startup.get("error").is_some(), "{startup}");
    unavailable["command"] = json!([
        "python3",
        "tests/fixtures/transport_fault_server.py",
        "describe_status"
    ]);
    let (_, failed) = call(&http, "verify_finding", unavailable);
    assert_eq!(
        failed["result"]["structuredContent"]["verified"], false,
        "{failed}"
    );
    assert_eq!(
        failed["result"]["structuredContent"]["lifecycle"],
        Value::Null
    );
    let db = rusqlite::Connection::open(dir.join("lifecycle.db")).unwrap();
    let count: i64 = db
        .query_row("SELECT COUNT(*) FROM probe_history", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 3);

    let mut red = args;
    red["manifest"]["probes"][6]["expect"]["equals"]["status"] = json!("not-ready");
    let (_, failed) = call(&http, "verify_finding", red);
    assert_eq!(failed["result"]["structuredContent"]["verified"], false);
    assert_eq!(
        failed["result"]["structuredContent"]["lifecycle"]["state"],
        "open"
    );
    assert_eq!(
        failed["result"]["structuredContent"]["lifecycle"]["consecutive_passes"],
        0
    );
}

#[test]
fn native_verification_rejects_invalid_or_mutating_cases_before_launch() {
    let dir = home();
    let id = promoted_finding(&dir);
    let marker = dir.join("launched");
    let (_server, port) = start_serve(&dir, &["--allow-spawn"]);
    let http = format!("http://127.0.0.1:{port}/mcp");
    let manifest: Value = serde_json::from_slice(&std::fs::read(MANIFEST).unwrap()).unwrap();
    let args = json!({"finding_id": id, "case_id": "literal-status", "manifest": manifest,
        "command": ["python3", "-c", format!("open({:?}, 'w').close()", marker)]});
    let mut unknown = args.clone();
    unknown["finding_id"] = json!("finding-0000000000000000");
    let mut wrong_case = args.clone();
    wrong_case["case_id"] = json!("not-declared");
    let mut wrong_tool = args.clone();
    wrong_tool["manifest"]["probes"][6]["tool"] = json!("read_counter");
    let mut mutating = args;
    mutating["allow_mutation"] = json!(true);
    mutating["manifest"]["probes"][6]["access"] = json!("mutating");
    let mut forbidden_flag = mutating.clone();
    forbidden_flag["allow_mutation"] = json!(true);
    mutating.as_object_mut().unwrap().remove("allow_mutation");
    for (arguments, expected) in [
        (unknown, "finding is unavailable"),
        (wrong_case, "not declared"),
        (wrong_tool, "does not match"),
        (mutating, "mutating"),
        (forbidden_flag, "invalid verify_finding arguments"),
    ] {
        let (_, denied) = call(&http, "verify_finding", arguments);
        assert!(
            denied["error"]["message"]
                .as_str()
                .unwrap()
                .contains(expected),
            "{denied}"
        );
        assert!(!marker.exists(), "invalid verification launched its target");
    }
    let db = rusqlite::Connection::open(dir.join("lifecycle.db")).unwrap();
    let count: i64 = db
        .query_row("SELECT COUNT(*) FROM probe_history", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 0);
}

#[test]
fn native_score_uses_the_cli_standard_and_reports_unavailable_targets() {
    let dir = home();
    let (_server, port) = start_serve(&dir, &["--allow-spawn"]);
    let http = format!("http://127.0.0.1:{port}/mcp");
    let cli = Command::new(bin())
        .args([
            "score",
            "--server",
            "fixture",
            "--format",
            "json",
            "--confirm-read-only",
            "--skip-tool",
            "shared_read",
            "--",
            "python3",
            CLEAN,
        ])
        .env("MCPEVAL_HOME", &dir)
        .output()
        .unwrap();
    let expected: Value = serde_json::from_slice(&cli.stdout).unwrap();
    let (_, scored) = call(
        &http,
        "score",
        json!({"server_label": "fixture",
        "command": ["python3", CLEAN], "confirm_read_only": true, "skip_tools": ["shared_read"]}),
    );
    let document = &scored["result"]["structuredContent"];
    assert_eq!(document["schema"], expected["schema"], "{scored}");
    assert_eq!(
        document["readiness"]["standard"],
        expected["readiness"]["standard"]
    );
    let checks = |doc: &Value| -> Vec<Value> {
        doc["readiness"]["areas"]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|area| {
                area["checks"].as_array().unwrap().iter().map(|check| {
                json!({"id": check["id"], "reason": check["reason"], "tool": check["tool"]})
            })
            })
            .collect()
    };
    assert_eq!(checks(document), checks(&expected));
    let (_, unavailable) = call(
        &http,
        "score",
        json!({"command": ["python3", CLEAN, "early-exit"]}),
    );
    assert!(
        !unavailable["result"]["structuredContent"]["readiness_error"].is_null(),
        "{unavailable}"
    );
    let text = unavailable["result"]["content"][0]["text"]
        .as_str()
        .unwrap();
    assert!(!text.contains("all cases passed"), "{text}");
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
    let (_server, port) = start_serve(&dir, &[]);
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
}

/// A legitimate request whose body exceeds the header budget, with
/// `Content-Length` sent before other headers: the header limit counts
/// header bytes only, never the body length.
#[test]
fn large_request_body_is_accepted_whatever_the_header_order() {
    let dir = home();
    let (_server, port) = start_serve(&dir, &[]);
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
}

/// A rejected request with a body larger than the server's header read:
/// the server must consume the body before closing, or the close resets
/// the connection (Linux sends RST when unread input remains) and the
/// client never sees the 400.
#[test]
fn duplicate_header_rejection_is_readable_despite_a_large_body() {
    let dir = home();
    let (_server, port) = start_serve(&dir, &[]);
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
}

/// The request a hostile page sends with `fetch(url, {method: "POST",
/// mode: "no-cors", body})`: cross-origin and `text/plain`. Even with
/// --allow-spawn it must not launch the command it names.
#[cfg(unix)]
#[test]
fn forged_cross_origin_run_probe_never_launches_its_command() {
    let dir = home();
    let marker = dir.join("launched");
    let (_server, port) = start_serve(&dir, &["--allow-spawn"]);
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
}
