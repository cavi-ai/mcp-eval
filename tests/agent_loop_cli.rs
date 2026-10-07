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

/// A running `mcpeval serve`, killed when dropped so a failing assertion
/// never leaks the process.
struct Serve(std::process::Child);

impl Drop for Serve {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
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
    let mut server = Serve(
        Command::new(bin())
            .args([
                "serve",
                "--listen",
                &format!("127.0.0.1:{port}"),
                "--allow-spawn",
            ])
            .env("MCPEVAL_HOME", &dir)
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap(),
    );
    let http = format!("http://127.0.0.1:{port}/mcp");
    // Wait for this child's own bind announcement, not merely an open port;
    // keep draining stderr so serve never writes into a closed pipe.
    let mut stderr = BufReader::new(server.0.stderr.take().unwrap());
    let mut announcement = String::new();
    stderr.read_line(&mut announcement).unwrap();
    assert!(
        announcement.contains(&http),
        "serve did not start: {announcement}"
    );
    std::thread::spawn(move || stderr.lines().for_each(drop));

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
            "scaffold",
            "score",
            "verify_finding",
            "record_annotation"
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
    // The agent surface scaffolds the same battery as `mcpeval init`.
    let init_path = dir.join("init.manifest.json");
    let init = Command::new(bin())
        .args([
            "init",
            "--server",
            "demo",
            "--confirm-read-only",
            "--output",
        ])
        .arg(&init_path)
        .args(["--", demo()])
        .env("MCPEVAL_HOME", &dir)
        .output()
        .unwrap();
    assert!(
        init.status.success(),
        "{}",
        String::from_utf8_lossy(&init.stderr)
    );
    let initialized: Value = serde_json::from_slice(&std::fs::read(&init_path).unwrap()).unwrap();
    let battery = |manifest: &Value| -> Vec<(String, String)> {
        manifest["probes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|case| {
                (
                    case["id"].as_str().unwrap().to_owned(),
                    case["probe"].as_str().unwrap().to_owned(),
                )
            })
            .collect()
    };
    assert_eq!(battery(&manifest), battery(&initialized));
    assert_eq!(battery(&manifest).len(), 25);

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
    assert_eq!(document["schema"], "mcpeval.probe-report/v2");
    assert_eq!(document["passed"], true);
    assert!(document["readiness"]["score"].as_u64().unwrap() >= 80);

    // Workflow guidance must agree between MCP text and structured content.
    let workflow_fixture =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/workflow_server.py");
    let workflow = call(
        &http,
        "run_probe",
        json!({
            "manifest":{"version":1,"probes":[{"id":"state-check","probe":"workflow","access":"read_only",
                "repetitions":1,"steps":[
                    {"tool":"read_status","arguments":{},"expect":{"outcome":"ok"}},
                    {"tool":"read_other","arguments":{},"expect":{"outcome":"ok"}},
                    {"tool":"read_status","arguments":{},"expect":{"outcome":"ok","equals_paths":{"/structuredContent/status":"ready"}}}
                ]}]},
            "command":["python3",workflow_fixture.to_str().unwrap(),"poison"],"server_label":"fixture"
        }),
    );
    let workflow_case = &workflow["result"]["structuredContent"]["cases"][0];
    assert_eq!(workflow_case["reason"], "value-mismatch");
    let hint = workflow_case["hint"].as_str().unwrap();
    assert!(hint.contains("Workflow call 3"), "{hint}");
    assert!(hint.contains("fresh session"), "{hint}");
    let workflow_text = workflow["result"]["content"][0]["text"].as_str().unwrap();
    assert!(workflow_text.contains(hint), "{workflow_text}");
    assert!(!serde_json::to_string(&workflow).unwrap().contains("CANARY"));

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

    // Step 4: the agent records what it observed, still over MCP. The
    // session is hashed before persistence, exactly like the CLI.
    let recorded = call(
        &http,
        "record_annotation",
        json!({
            "session": "the-agent-session",
            "seq": 42,
            "kind": "instruction-divergence",
            "note": "instructions promised a paginated catalog; one page only"
        }),
    );
    let recorded_text = recorded["result"]["content"][0]["text"].as_str().unwrap();
    assert!(
        recorded_text.contains("recorded instruction-divergence"),
        "{recorded_text}"
    );

    // A false-success finding needs a semantic oracle even through MCP.
    let mut store = mcpeval::store::Store::open(Some(dir.clone())).unwrap();
    for session in ["first", "second"] {
        store
            .append(
                &serde_json::from_value(json!({
                    "ts":"2026-08-05T00:00:01Z", "session":session, "seq":1,
                    "server":"fixture", "method":"tools/call", "tool":"read_status",
                    "args":{}, "latency_ms":1, "outcome":"error", "shim_self_us":1,
                    "kind":"real", "error":{"code":"broken", "retryable":false,
                        "template_id":"aaaaaaaaaaaaaaaa"}
                }))
                .unwrap(),
            )
            .unwrap();
    }
    mcpeval::index::build(&dir).unwrap();
    mcpeval::promote::promote(
        &dir,
        mcpeval::promote::PromotionConfig {
            threshold: 0.0,
            now: chrono::Utc::now(),
        },
    )
    .unwrap();
    let db = rusqlite::Connection::open(dir.join("index.db")).unwrap();
    let finding: String = db
        .query_row(
            "SELECT f.finding_id FROM findings f JOIN issues i ON i.id=f.issue_id WHERE i.tool='read_status'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    db.execute(
        "UPDATE issues SET class='false-success' WHERE finding_id=?1",
        [&finding],
    )
    .unwrap();
    let marker = dir.join("unexpected-launch");
    let rejected = call(
        &http,
        "verify_finding",
        json!({
            "finding_id":finding, "case_id":"check",
            "manifest":{"version":1,"probes":[{
                "id":"check","probe":"instruction-fidelity","access":"read_only",
                "tool":"read_status","arguments":{},"expect":{"outcome":"ok"}
            }]},
            "command":["python3","-c","import pathlib,sys; pathlib.Path(sys.argv[1]).touch()",marker]
        }),
    );
    assert!(
        rejected["error"]["message"]
            .as_str()
            .unwrap()
            .contains("result assertion or an expected error"),
        "{rejected}"
    );
    assert!(
        !marker.exists(),
        "MCP verification launched a rejected case"
    );
    let evidence = rusqlite::Connection::open(dir.join("lifecycle.db")).unwrap();
    let history: i64 = evidence
        .query_row("SELECT COUNT(*) FROM probe_history", [], |row| row.get(0))
        .unwrap();
    assert_eq!(history, 0);
    let state: (String, i64) = evidence
        .query_row(
            "SELECT state,consecutive_passes FROM finding_lifecycle WHERE finding_id=?1",
            [&finding],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(state, ("open".into(), 0));

    let fixture =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/workflow_server.py");
    let verified = call(
        &http,
        "verify_finding",
        json!({
            "finding_id":finding, "case_id":"check",
            "manifest":{"version":1,"probes":[{
                "id":"check","probe":"instruction-fidelity","access":"read_only",
                "tool":"read_status","arguments":{},"expect":{"outcome":"ok",
                    "equals_paths":{"/structuredContent/status":"ready"}}
            }]}, "command":["python3",fixture,"clean"]
        }),
    );
    assert_eq!(
        verified["result"]["structuredContent"]["verified"], true,
        "{verified}"
    );
    assert_eq!(
        verified["result"]["structuredContent"]["lifecycle"]["consecutive_passes"],
        1
    );
    assert!(!verified.to_string().contains("CANARY"));

    drop(server);
    let mut stored = Vec::new();
    for entry in std::fs::read_dir(dir.join("store")).unwrap() {
        let path = entry.unwrap().path();
        let name = path.file_name().unwrap().to_str().unwrap().to_owned();
        if name.starts_with("annotations-") && name.ends_with(".jsonl") {
            for line in std::fs::read_to_string(&path).unwrap().lines() {
                stored.push(serde_json::from_str::<Value>(line).unwrap());
            }
        }
    }
    assert_eq!(stored.len(), 1);
    assert!(stored[0]["session"]
        .as_str()
        .unwrap()
        .starts_with("session:"));
    assert_eq!(stored[0]["kind"], "instruction-divergence");
    assert_eq!(stored[0]["seq"], 42);
}
