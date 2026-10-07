use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};

use serde_json::{json, Value};

struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("mcpeval-workflow-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
struct Server(Child);
impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/workflow_server.py")
}
fn workflow() -> Value {
    json!({"id":"read-sequence","probe":"workflow","access":"read_only","repetitions":3,
    "steps":[
        {"tool":"read_status","arguments":{"key":"CANARY private argument"},"expect":{"outcome":"ok","equals":{"status":"ready"}}},
        {"tool":"read_other","arguments":{},"expect":{"outcome":"ok"}},
        {"tool":"read_status","arguments":{},"expect":{"outcome":"ok","equals":{"status":"ready"}}}
    ]})
}
fn run(root: &Path, case: Value, mode: &str, url: Option<&str>) -> Output {
    let manifest = root.join("manifest.json");
    std::fs::write(&manifest, json!({"version":1,"probes":[case]}).to_string()).unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_mcpeval"));
    command
        .args([
            "probe",
            "--server",
            "fixture",
            "--manifest",
            manifest.to_str().unwrap(),
            "--format",
            "json",
            "--probe",
            "workflow",
        ])
        .env("MCPEVAL_HOME", root)
        .env("WORKFLOW_CALL_LOG", root.join("calls.jsonl"));
    if let Some(url) = url {
        command.args(["--url", url]);
    } else {
        command.args(["--", "python3", fixture().to_str().unwrap(), mode]);
    }
    command.output().unwrap()
}
fn calls(root: &Path) -> Vec<Value> {
    std::fs::read_to_string(root.join("calls.jsonl"))
        .unwrap_or_default()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .filter(|event: &Value| event["method"] == "tools/call")
        .collect()
}
fn report(output: &Output) -> Value {
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    mcpeval::probe::ProbeReport::from_json_document(&value).unwrap();
    jsonschema::validator_for(
        &serde_json::from_str::<Value>(include_str!("../docs/mcp-eval.probe-report.schema.json"))
            .unwrap(),
    )
    .unwrap()
    .validate(&value)
    .unwrap();
    value
}

#[test]
fn workflow_repeats_in_one_session_and_does_not_export_payloads() {
    let root = Temp::new();
    let output = run(&root.0, workflow(), "clean", None);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report = report(&output);
    let manifest: Value =
        serde_json::from_slice(&std::fs::read(root.0.join("manifest.json")).unwrap()).unwrap();
    jsonschema::validator_for(
        &serde_json::from_str::<Value>(include_str!("../docs/mcp-eval.manifest.schema.json"))
            .unwrap(),
    )
    .unwrap()
    .validate(&manifest)
    .unwrap();
    assert_eq!(report["cases"][0]["attempts"], 9);
    assert_eq!(report["cases"][0]["first_failure"], Value::Null);
    let calls = calls(&root.0);
    assert_eq!(calls.len(), 9);
    assert!(calls
        .iter()
        .all(|call| call["session"] == calls[0]["session"]));
    let stored = std::fs::read_dir(root.0.join("store"))
        .unwrap()
        .filter_map(|entry| {
            let path = entry.unwrap().path();
            path.is_file()
                .then(|| std::fs::read_to_string(path).unwrap())
        })
        .collect::<String>();
    assert!(stored
        .lines()
        .all(|line| line.contains("\"kind\":\"synthetic\"")));
    assert!(!stored.contains("CANARY"));
    assert!(!String::from_utf8_lossy(&output.stdout).contains("CANARY"));
    assert!(!String::from_utf8_lossy(&output.stderr).contains("CANARY"));
}

#[test]
fn workflow_fails_at_the_cross_tool_corruption_and_stops() {
    let root = Temp::new();
    let output = run(&root.0, workflow(), "poison", None);
    assert_eq!(
        output.status.code(),
        Some(1),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let case = &report(&output)["cases"][0];
    assert_eq!(case["reason"], "value-mismatch");
    assert_eq!(case["first_failure"], 3);
    assert_eq!(case["attempts"], 3);
    assert_eq!(case["tool"], "read_status");
    assert_eq!(calls(&root.0).len(), 3);
}

#[test]
fn workflow_remediation_identifies_the_failed_call_across_output_formats() {
    let root = Temp::new();
    let output = run(&root.0, workflow(), "poison", None);
    let document = report(&output);
    assert_eq!(document["cases"][0]["reason"], "value-mismatch");
    let hint = document["cases"][0]["hint"].as_str().unwrap();
    assert!(hint.contains("Workflow call 3"), "{hint}");
    assert!(hint.contains("fresh session"), "{hint}");
    assert!(hint.contains("equals_paths"), "{hint}");
    let parsed = mcpeval::probe::ProbeReport::from_json_document(&document).unwrap();
    assert_eq!(parsed.to_json("fixture")["cases"][0]["hint"], hint);
    for format in ["text", "markdown", "sarif"] {
        let output = Command::new(env!("CARGO_BIN_EXE_mcpeval"))
            .args([
                "probe",
                "--server",
                "fixture",
                "--manifest",
                root.0.join("manifest.json").to_str().unwrap(),
                "--format",
                format,
                "--probe",
                "workflow",
                "--",
                "python3",
                fixture().to_str().unwrap(),
                "poison",
            ])
            .env("MCPEVAL_HOME", &root.0)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(1));
        let stdout = String::from_utf8_lossy(&output.stdout);
        let text = if format == "sarif" {
            serde_json::from_str::<Value>(&stdout).unwrap()["runs"][0]["results"][0]["message"]
                ["text"]
                .as_str()
                .unwrap()
                .to_owned()
        } else {
            stdout.to_string()
        };
        assert!(text.contains(hint), "{format}: {text}");
        assert!(!text.contains("CANARY"));
    }
    let clean = run(&root.0, workflow(), "clean", None);
    assert_eq!(report(&clean)["cases"][0]["hint"], Value::Null);
}

#[test]
fn workflow_checks_state_after_an_expected_error() {
    for (mode, code) in [("clean", 0), ("error-poison", 1)] {
        let root = Temp::new();
        let mut case = workflow();
        case["steps"][1] = json!({"tool":"invalid_read","arguments":{},"expect":{"outcome":"error","error_code":-32602}});
        let output = run(&root.0, case, mode, None);
        assert_eq!(
            output.status.code(),
            Some(code),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        if code == 1 {
            assert_eq!(report(&output)["cases"][0]["first_failure"], 3);
        }
    }
}

#[test]
fn nested_assertions_check_structured_content_arrays_nulls_and_escaped_keys() {
    for (mode, code) in [("clean", 0), ("poison", 1)] {
        let root = Temp::new();
        let mut case = workflow();
        case["steps"][2]["expect"] = json!({"outcome":"ok",
            "equals":{"structuredContent.status":"literal"},
            "required_result_paths":["/structuredContent/items/0/ready","/structuredContent/nullable"],
            "equals_paths":{"/structuredContent/status":"ready","/structuredContent/items/0/ready":true,
                "/structuredContent/nullable":null,"/structuredContent/a~1b/~0state":"ready"}});
        let output = run(&root.0, case, mode, None);
        let manifest: Value =
            serde_json::from_slice(&std::fs::read(root.0.join("manifest.json")).unwrap()).unwrap();
        jsonschema::validator_for(
            &serde_json::from_str::<Value>(include_str!("../docs/mcp-eval.manifest.schema.json"))
                .unwrap(),
        )
        .unwrap()
        .validate(&manifest)
        .unwrap();
        assert_eq!(
            output.status.code(),
            Some(code),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let document = report(&output);
        if code == 1 {
            assert_eq!(document["cases"][0]["reason"], "value-mismatch");
        }
        assert!(!String::from_utf8_lossy(&output.stdout).contains("structuredContent"));
        assert!(!String::from_utf8_lossy(&output.stdout).contains("CANARY"));
    }
}

#[test]
fn missing_paths_are_distinct_from_existing_null_values() {
    for (expect, reason) in [
        (
            json!({"outcome":"ok","required_result_paths":["/structuredContent/missing"]}),
            "missing-field",
        ),
        (
            json!({"outcome":"ok","equals_paths":{"/structuredContent/missing":null}}),
            "value-mismatch",
        ),
    ] {
        let root = Temp::new();
        let mut case = workflow();
        case["steps"][0]["expect"] = expect;
        let output = run(&root.0, case, "clean", None);
        assert_eq!(
            output.status.code(),
            Some(1),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let document = report(&output);
        assert_eq!(document["cases"][0]["reason"], reason);
        assert!(document["cases"][0]["hint"]
            .as_str()
            .unwrap()
            .contains("Workflow call 1"));
        assert_eq!(calls(&root.0).len(), 1);
    }
}

#[test]
fn invalid_nested_assertions_fail_before_startup_without_echoing_values() {
    let invalid = [
        json!({"outcome":"ok","required_result_paths":[""]}),
        json!({"outcome":"ok","required_result_paths":["structuredContent/status"]}),
        json!({"outcome":"ok","required_result_paths":["/bad~2escape"]}),
        json!({"outcome":"ok","required_result_paths":[format!("/{}","x".repeat(512))]}),
        json!({"outcome":"ok","required_result_paths":["/structuredContent/status","/structuredContent/status"]}),
        json!({"outcome":"error","equals_paths":{"/structuredContent/status":"ready"}}),
        json!({"outcome":"ok","equals_paths":{"/structuredContent/status":{"raw":"CANARY private content"}}}),
        json!({"outcome":"ok","equals_paths":{"/CANARY private path":"ready"}}),
    ];
    for expect in invalid {
        let root = Temp::new();
        let mut case = workflow();
        case["steps"][0]["expect"] = expect;
        let output = run(&root.0, case, "clean", None);
        assert_eq!(output.status.code(), Some(2));
        assert!(!root.0.join("calls.jsonl").exists());
        assert!(!String::from_utf8_lossy(&output.stderr).contains("CANARY"));
    }
}

#[test]
fn workflow_refuses_annotated_writers_before_any_step() {
    let root = Temp::new();
    let output = run(&root.0, workflow(), "writer", None);
    assert_eq!(output.status.code(), Some(2));
    assert!(calls(&root.0).is_empty());
}

#[test]
fn invalid_workflows_never_start_the_server() {
    for (field, value) in [
        ("steps", json!([])),
        ("steps", json!(vec![workflow()["steps"][0].clone(); 33])),
        ("repetitions", json!(0)),
        ("repetitions", json!(21)),
        ("access", json!("mutating")),
    ] {
        let root = Temp::new();
        let mut case = workflow();
        case[field] = value;
        let output = run(&root.0, case, "clean", None);
        assert_eq!(output.status.code(), Some(2));
        assert!(!root.0.join("calls.jsonl").exists());
    }
}

#[test]
fn workflow_transport_failure_has_no_pass_credit_or_partial_verdict() {
    let root = Temp::new();
    let output = run(&root.0, workflow(), "crash", None);
    assert_eq!(output.status.code(), Some(3));
    let case = &report(&output)["cases"][0];
    assert_eq!(case["passed"], false);
    assert_eq!(case["reason"], "transport-closed");
    assert_eq!(case["first_failure"], Value::Null);
    assert_eq!(calls(&root.0).len(), 2);
}

#[test]
fn workflow_http_uses_one_fresh_session_for_all_steps() {
    for (mode, code) in [("clean", 0), ("poison", 1)] {
        let root = Temp::new();
        let mut server = Server(
            Command::new("python3")
                .args([fixture().to_str().unwrap(), mode, "http"])
                .env("WORKFLOW_CALL_LOG", root.0.join("calls.jsonl"))
                .stdout(Stdio::piped())
                .spawn()
                .unwrap(),
        );
        let mut url = String::new();
        BufReader::new(server.0.stdout.take().unwrap())
            .read_line(&mut url)
            .unwrap();
        let mut case = workflow();
        case["steps"][2]["expect"] =
            json!({"outcome":"ok","equals_paths":{"/structuredContent/status":"ready"}});
        let output = run(&root.0, case, mode, Some(url.trim()));
        assert_eq!(
            output.status.code(),
            Some(code),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let calls = calls(&root.0);
        assert!(!calls.is_empty());
        assert!(calls
            .iter()
            .all(|call| call["session"] == calls[0]["session"]));
        report(&output);
    }
}

#[test]
fn malformed_tool_results_receive_no_workflow_credit_over_stdio_or_http() {
    for mode in [
        "malformed-error-flag",
        "malformed-result-null",
        "malformed-result-bool",
        "malformed-result-number",
        "malformed-result-string",
        "malformed-result-array",
    ] {
        for http in [false, true] {
            let root = Temp::new();
            let mut server = http.then(|| {
                Server(
                    Command::new("python3")
                        .args([fixture().to_str().unwrap(), mode, "http"])
                        .env("WORKFLOW_CALL_LOG", root.0.join("calls.jsonl"))
                        .stdout(Stdio::piped())
                        .spawn()
                        .unwrap(),
                )
            });
            let mut url = String::new();
            if let Some(server) = server.as_mut() {
                BufReader::new(server.0.stdout.take().unwrap())
                    .read_line(&mut url)
                    .unwrap();
            }
            let mut case = workflow();
            for step in case["steps"].as_array_mut().unwrap() {
                step["expect"] = json!({"outcome":"ok"});
            }
            let output = run(&root.0, case, mode, http.then(|| url.trim()));
            assert_eq!(
                output.status.code(),
                Some(3),
                "{mode} http={http}: {} {}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            let document = report(&output);
            assert_eq!(document["gate"]["passed"], 0);
            assert_eq!(document["cases"][0]["reason"], "transport-error");
            assert_eq!(document["cases"][0]["first_failure"], Value::Null);
            assert!(!document["cases"][0]["hint"]
                .as_str()
                .unwrap()
                .contains("Workflow call"));
            assert_eq!(calls(&root.0).len(), 1);
            assert!(!String::from_utf8_lossy(&output.stdout).contains("CANARY"));
            assert!(!String::from_utf8_lossy(&output.stderr).contains("CANARY"));
        }
    }
}

#[test]
fn workflow_reports_can_gate_a_regression_through_the_native_diff() {
    let root = Temp::new();
    for (mode, name) in [("clean", "baseline.json"), ("poison", "current.json")] {
        let output = run(&root.0, workflow(), mode, None);
        report(&output);
        std::fs::write(root.0.join(name), output.stdout).unwrap();
    }
    let output = Command::new(env!("CARGO_BIN_EXE_mcpeval"))
        .args([
            "diff",
            root.0.join("baseline.json").to_str().unwrap(),
            root.0.join("current.json").to_str().unwrap(),
            "--format",
            "json",
            "--fail-on-regression",
        ])
        .env("MCPEVAL_HOME", &root.0)
        .output()
        .unwrap();
    assert_eq!(
        output.status.code(),
        Some(1),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let document: Value = serde_json::from_slice(&output.stdout).unwrap();
    jsonschema::validator_for(
        &serde_json::from_str::<Value>(include_str!("../docs/mcp-eval.probe-diff.schema.json"))
            .unwrap(),
    )
    .unwrap()
    .validate(&document)
    .unwrap();
    assert_eq!(document["summary"]["regressed"], 1);
}

#[test]
fn workflow_isolated_session_does_not_hide_or_inherit_other_case_state() {
    let root = Temp::new();
    let mut case = workflow();
    case["steps"] = json!([case["steps"][0].clone(), case["steps"][2].clone()]);
    case["repetitions"] = json!(1);
    let manifest = root.0.join("mixed.json");
    std::fs::write(&manifest,json!({"version":1,"probes":[
        {"id":"prelude","probe":"instruction-fidelity","tool":"read_other","access":"read_only","arguments":{},"expect":{"outcome":"ok"}},
        case,
        {"id":"after","probe":"instruction-fidelity","tool":"read_status","access":"read_only","arguments":{},"expect":{"outcome":"ok","equals":{"status":"ready"}}}
    ]}).to_string()).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_mcpeval"))
        .args([
            "probe",
            "--server",
            "fixture",
            "--manifest",
            manifest.to_str().unwrap(),
            "--format",
            "json",
            "--",
            "python3",
            fixture().to_str().unwrap(),
            "poison",
        ])
        .env("MCPEVAL_HOME", &root.0)
        .env("WORKFLOW_CALL_LOG", root.0.join("calls.jsonl"))
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    let report = report(&output);
    assert_eq!(report["cases"][0]["passed"], true);
    assert_eq!(report["cases"][1]["passed"], true);
    assert_eq!(report["cases"][2]["passed"], false);
    let calls = calls(&root.0);
    assert_ne!(calls[0]["session"], calls[1]["session"]);
    assert_eq!(calls[0]["session"], calls[3]["session"]);
}

#[test]
fn workflow_verification_closes_only_after_complete_passes_and_reopens_on_a_mismatch() {
    use chrono::{TimeZone, Utc};
    use mcpeval::record::{CallRecord, ErrorInfo};
    let root = Temp::new();
    let mut store = mcpeval::store::Store::open(Some(root.0.clone())).unwrap();
    for session in ["first", "second"] {
        store
            .append(&CallRecord {
                identity: None,
                ts: "2026-08-05T12:00:00Z".into(),
                session: session.into(),
                seq: 1,
                server: "fixture".into(),
                method: "tools/call".into(),
                tool: Some("read_status".into()),
                args: Some(json!({})),
                latency_ms: Some(5),
                outcome: "error".into(),
                error: Some(ErrorInfo {
                    code: Some(json!(-32000)),
                    layer: None,
                    retryable: Some(false),
                    kind: None,
                    template: Some("fixture failure".into()),
                    template_id: Some("aaaaaaaaaaaaaaaa".into()),
                }),
                shim_self_us: 0,
                kind: "real".into(),
            })
            .unwrap();
    }
    mcpeval::index::build(&root.0).unwrap();
    mcpeval::promote::promote(
        &root.0,
        mcpeval::promote::PromotionConfig {
            threshold: 0.0,
            now: Utc.with_ymd_and_hms(2026, 8, 6, 0, 0, 0).unwrap(),
        },
    )
    .unwrap();
    let finding: String = rusqlite::Connection::open(root.0.join("index.db"))
        .unwrap()
        .query_row("SELECT finding_id FROM findings", [], |row| row.get(0))
        .unwrap();
    let manifest = root.0.join("verify.json");
    std::fs::write(
        &manifest,
        json!({"version":1,"probes":[workflow()]}).to_string(),
    )
    .unwrap();
    for (mode, state) in [
        ("clean", "verifying"),
        ("clean", "verifying"),
        ("clean", "closed"),
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_mcpeval"))
            .args([
                "verify",
                "--finding",
                &finding,
                "--case",
                "read-sequence",
                "--manifest",
                manifest.to_str().unwrap(),
                "--",
                "python3",
                fixture().to_str().unwrap(),
                mode,
            ])
            .env("MCPEVAL_HOME", &root.0)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(String::from_utf8_lossy(&output.stdout).contains(state));
    }
    // A malformed outcome cannot add pass evidence or alter a closed finding.
    for mode in [
        "malformed-error-flag",
        "malformed-result-null",
        "malformed-result-bool",
        "malformed-result-number",
        "malformed-result-string",
        "malformed-result-array",
    ] {
        let malformed = Command::new(env!("CARGO_BIN_EXE_mcpeval"))
            .args([
                "verify",
                "--finding",
                &finding,
                "--case",
                "read-sequence",
                "--manifest",
                manifest.to_str().unwrap(),
                "--",
                "python3",
                fixture().to_str().unwrap(),
                mode,
            ])
            .env("MCPEVAL_HOME", &root.0)
            .output()
            .unwrap();
        assert_eq!(malformed.status.code(), Some(3), "{mode}");
        assert!(!String::from_utf8_lossy(&malformed.stdout).contains("CANARY"));
        assert!(!String::from_utf8_lossy(&malformed.stderr).contains("CANARY"));
        let evidence = rusqlite::Connection::open(root.0.join("lifecycle.db")).unwrap();
        let state: String = evidence
            .query_row(
                "SELECT state FROM finding_lifecycle WHERE finding_id=?1",
                [&finding],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(state, "closed");
        let history: i64 = evidence
            .query_row(
                "SELECT COUNT(*) FROM probe_history WHERE finding_id=?1",
                [&finding],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(history, 3);
    }
    // The same target command is required by lifecycle binding. Altering the
    // oracle instead proves a mismatch reopens an already closed workflow.
    let mut case = workflow();
    case["steps"][2]["expect"]["equals"]["status"] = json!("other");
    std::fs::write(&manifest, json!({"version":1,"probes":[case]}).to_string()).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_mcpeval"))
        .args([
            "verify",
            "--finding",
            &finding,
            "--case",
            "read-sequence",
            "--manifest",
            manifest.to_str().unwrap(),
            "--",
            "python3",
            fixture().to_str().unwrap(),
            "clean",
        ])
        .env("MCPEVAL_HOME", &root.0)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stdout).contains("open"));
    assert!(String::from_utf8_lossy(&output.stdout).contains("Workflow call 3"));
    assert!(String::from_utf8_lossy(&output.stdout).contains("fresh session"));
}
