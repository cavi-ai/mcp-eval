use std::process::Command;

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_mcpeval")
}

fn demo() -> &'static str {
    env!("CARGO_BIN_EXE_mcpeval-demo")
}

fn home() -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!("mcpeval-cancel-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&path).unwrap();
    path
}

fn probe_manifest(body: &str, extra: &[&str]) -> std::process::Output {
    let dir = home();
    let manifest = dir.join("m.json");
    std::fs::write(&manifest, body).unwrap();
    Command::new(bin())
        .args([
            "probe",
            "--server",
            "demo",
            "--manifest",
            manifest.to_str().unwrap(),
            "--format",
            "json",
        ])
        .args(["--", demo()])
        .args(extra)
        .env("MCPEVAL_HOME", &dir)
        .output()
        .unwrap()
}

#[test]
fn an_honoring_server_passes_and_the_defect_fails_with_fixed_reasons() {
    let clean = probe_manifest(
        r#"{"version":1,"probes":[{"id":"cancel-slow","probe":"cancellation","tool":"slow_read","access":"read_only","arguments":{},"grace_seconds":3,"reason":"probe"}]}"#,
        &[],
    );
    assert!(
        clean.status.success(),
        "{}",
        String::from_utf8_lossy(&clean.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&clean.stdout).unwrap();
    assert_eq!(report["passed"], true);
    assert_eq!(report["cases"][0]["probe"], "cancellation");

    let broken = probe_manifest(
        r#"{"version":1,"probes":[{"id":"cancel-slow","probe":"cancellation","tool":"slow_read","access":"read_only","arguments":{},"grace_seconds":3,"reason":"probe"}]}"#,
        &["--broken", "cancellation"],
    );
    assert!(!broken.status.success());
    let report: serde_json::Value = serde_json::from_slice(&broken.stdout).unwrap();
    assert_eq!(report["cases"][0]["reason"], "cancellation-ignored");
}

#[test]
fn a_tool_that_cannot_be_cancelled_unless_it_works() {
    // Preflight: a tool that errors uncancelled must fail with
    // unexpected-outcome, not pass via a broken-server accident.
    let output = probe_manifest(
        r#"{"version":1,"probes":[{"id":"cancel-broken-tool","probe":"cancellation","tool":"break_session","access":"read_only","arguments":{},"grace_seconds":3,"reason":"probe"}]}"#,
        &[],
    );
    assert!(!output.status.success());
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["cases"][0]["reason"], "unexpected-outcome");
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum CancelMode {
    /// Answer the cancelled call with the structured -32800 error.
    Acknowledge,
    /// Never answer the cancelled call: hold the connection past grace.
    Silence,
    /// Finish the work and return the full result anyway.
    Ignore,
}

/// Run the cancellation probe against a thread-based Streamable HTTP
/// fixture and return the CLI output with its wall-clock duration.
fn probe_http(mode: CancelMode, grace_seconds: u64) -> (std::process::Output, std::time::Duration) {
    let dir = home();
    let manifest = dir.join("m.json");
    std::fs::write(
        &manifest,
        format!(
            r#"{{"version":1,"probes":[{{"id":"cancel-http","probe":"cancellation","tool":"slow_read","access":"read_only","arguments":{{}},"grace_seconds":{grace_seconds},"reason":"probe"}}]}}"#
        ),
    )
    .unwrap();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    // The fixture flips to "cancelled-aware" when a cancellation
    // notification arrives, mirroring a server that observes the
    // notification mid-flight.
    let cancel_seen = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let server = std::thread::spawn(move || {
        // initialize, initialized, tools/list, preflight call, cancelled
        // call, cancellation notification: six connections per run.
        for _ in 0..6 {
            let Ok((mut stream, _)) = listener.accept() else {
                break;
            };
            let cancel_seen = std::sync::Arc::clone(&cancel_seen);
            std::thread::spawn(move || {
                handle_http_connection(&mut stream, &cancel_seen, mode);
            });
        }
    });
    let started = std::time::Instant::now();
    let output = Command::new(bin())
        .args([
            "probe",
            "--server",
            "fixture",
            "--manifest",
            manifest.to_str().unwrap(),
            "--format",
            "json",
            "--url",
            &endpoint,
        ])
        .env("MCPEVAL_HOME", &dir)
        .output()
        .unwrap();
    let elapsed = started.elapsed();
    server.join().unwrap();
    (output, elapsed)
}

#[test]
fn cancellation_works_over_streamable_http() {
    // The production-server pattern: the cancelled call is answered with
    // the structured "Request cancelled" error.
    let (output, _) = probe_http(CancelMode::Acknowledge, 3);
    assert!(
        output.status.success(),
        "{} {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["cases"][0]["probe"], "cancellation");
    assert_eq!(report["cases"][0]["passed"], true);
}

#[test]
fn http_silence_within_grace_is_honored() {
    // A server that never answers the cancelled call is honoring the
    // cancellation. The wait is bounded by the manifest's grace_seconds,
    // not by the client's fixed read timeout: one second of grace must
    // finish well under the five-second agent default.
    let (output, elapsed) = probe_http(CancelMode::Silence, 1);
    assert!(
        output.status.success(),
        "{} {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["cases"][0]["passed"], true);
    assert!(
        elapsed < std::time::Duration::from_secs(3),
        "silence took {elapsed:?}; grace_seconds was not applied"
    );
}

#[test]
fn http_full_result_after_cancel_is_ignored() {
    let (output, _) = probe_http(CancelMode::Ignore, 3);
    assert!(!output.status.success());
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["cases"][0]["reason"], "cancellation-ignored");
}

/// Minimal Streamable HTTP MCP fixture: initialize/tools-list plus a
/// tools/call handler whose response to a cancelled call depends on `mode`.
fn handle_http_connection(
    stream: &mut std::net::TcpStream,
    cancel_seen: &std::sync::atomic::AtomicBool,
    mode: CancelMode,
) {
    use std::io::{BufRead, BufReader, Read, Write};
    let mut reader = BufReader::new(stream.try_clone().unwrap());
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
    let request: serde_json::Value = serde_json::from_slice(&body).unwrap();
    if request["method"] == "notifications/cancelled" {
        cancel_seen.store(true, std::sync::atomic::Ordering::SeqCst);
    }
    if request.get("id").is_none() {
        stream
            .write_all(b"HTTP/1.1 202 Accepted\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
            .unwrap();
        return;
    }
    let result = match request["method"].as_str().unwrap() {
        "initialize" => serde_json::json!({
            "protocolVersion": "2025-06-18",
            "capabilities": {"tools": {}},
            "serverInfo": {"name": "fixture", "version": "1"}
        }),
        "tools/list" => serde_json::json!({"tools": [
            {"name": "slow_read", "inputSchema": {"type": "object", "properties": {}}}
        ]}),
        _ => serde_json::json!({}),
    };
    let mut frame = serde_json::json!({"jsonrpc": "2.0", "id": request["id"], "result": result});
    // A tools/call models a long-running operation: it holds its response
    // until either the cancellation notification arrives or a bounded hold
    // elapses (the uncancelled preflight call takes that path and is
    // answered normally). This is what makes the cancellation observable
    // mid-flight for the client.
    if request["method"] == "tools/call" {
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(300);
        while !cancel_seen.load(std::sync::atomic::Ordering::SeqCst)
            && std::time::Instant::now() < deadline
        {
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        if cancel_seen.load(std::sync::atomic::Ordering::SeqCst) {
            match mode {
                CancelMode::Acknowledge => {
                    frame = serde_json::json!({"jsonrpc": "2.0", "id": request["id"],
                        "error": {"code": -32800, "message": "Request cancelled"}});
                }
                CancelMode::Silence => {
                    // Never answer: hold the connection until the client has
                    // given up, then drop it without writing a frame.
                    std::thread::sleep(std::time::Duration::from_secs(3));
                    return;
                }
                CancelMode::Ignore => {}
            }
        }
    }
    let payload = frame.to_string();
    write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{payload}",
        payload.len()
    )
    .unwrap();
}

#[test]
fn cancellation_counts_toward_reliability() {
    let dir = home();
    let manifest = dir.join("m.json");
    std::fs::write(
        &manifest,
        r#"{"version":1,"probes":[{"id":"cancel-slow","probe":"cancellation","tool":"slow_read","access":"read_only","arguments":{},"grace_seconds":3,"reason":"probe"}]}"#,
    )
    .unwrap();
    let output = Command::new(bin())
        .args([
            "probe",
            "--server",
            "demo",
            "--manifest",
            manifest.to_str().unwrap(),
        ])
        .args(["--", demo()])
        .env("MCPEVAL_HOME", &dir)
        .output()
        .unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("reliability=1/1"), "{stdout}");
}

#[test]
fn manifest_validation_rejects_out_of_bounds_cancellation() {
    let dir = home();
    let manifest = dir.join("m.json");
    std::fs::write(
        &manifest,
        r#"{"version":1,"probes":[{"id":"cancel","probe":"cancellation","tool":"slow_read","access":"read_only","arguments":{},"grace_seconds":0,"reason":"probe"}]}"#,
    )
    .unwrap();
    let output = Command::new(bin())
        .args([
            "probe",
            "--server",
            "demo",
            "--manifest",
            manifest.to_str().unwrap(),
        ])
        .args(["--", demo()])
        .env("MCPEVAL_HOME", &dir)
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("grace_seconds"));

    let manifest2 = dir.join("m2.json");
    std::fs::write(
        &manifest2,
        r#"{"version":1,"probes":[{"id":"cancel","probe":"cancellation","tool":"slow_read","access":"read_only","arguments":{},"grace_seconds":3,"reason":"has spaces"}]}"#,
    )
    .unwrap();
    let output = Command::new(bin())
        .args([
            "probe",
            "--server",
            "demo",
            "--manifest",
            manifest2.to_str().unwrap(),
        ])
        .args(["--", demo()])
        .env("MCPEVAL_HOME", &dir)
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("reason is invalid"));
}
