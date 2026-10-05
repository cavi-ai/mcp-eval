use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::process::Command;

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/evaluation_contract_server.py")
}

fn home() -> PathBuf {
    let path = std::env::temp_dir().join(format!("mcpeval-integrity-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&path).unwrap();
    path
}

fn score(mode: &str) -> Value {
    let dir = home();
    let output = Command::new(env!("CARGO_BIN_EXE_mcpeval"))
        .args([
            "score",
            "--server",
            "synthetic",
            "--format",
            "json",
            "--",
            "python3",
        ])
        .arg(fixture())
        .arg(mode)
        .env("MCPEVAL_HOME", &dir)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let document = serde_json::from_slice(&output.stdout).unwrap();
    std::fs::remove_dir_all(dir).unwrap();
    document
}

fn reliability(document: &Value) -> &Value {
    document["readiness"]["areas"]
        .as_array()
        .unwrap()
        .iter()
        .find(|area| area["name"] == "reliability")
        .unwrap()
}

#[test]
fn consistently_failing_calls_receive_no_reliability_credit() {
    let document = score("all-errors");
    assert_eq!(reliability(&document)["score"], 0);
    assert!(document["readiness"]["score"].as_u64().unwrap() < 100);
    assert_eq!(
        reliability(&document)["measurements"]["successful_calls"],
        0
    );
    assert_eq!(reliability(&document)["measurements"]["tool_errors"], 3);
}

#[test]
fn a_later_transport_failure_preserves_success_counts_and_coverage() {
    let document = score("partial-transport");
    assert_eq!(reliability(&document)["score"], 0);
    assert_eq!(
        reliability(&document)["measurements"]["successful_calls"],
        1
    );
    assert_eq!(
        reliability(&document)["measurements"]["transport_errors"],
        1
    );
    assert_eq!(document["readiness"]["surface"]["exercised"], 1);
}

#[test]
fn every_successful_output_must_conform_including_later_repeats() {
    for mode in ["wrong-output-type", "late-invalid-output"] {
        let document = score(mode);
        assert!(
            reliability(&document)["score"].as_u64().unwrap() < 100,
            "{document}"
        );
        assert!(reliability(&document)["checks"]
            .as_array()
            .unwrap()
            .iter()
            .any(|check| check["reason"] == "reliability-output-schema-broken"));
    }
    assert_eq!(score("clean")["readiness"]["score"], 100);
}

#[test]
fn manifest_output_schema_probe_rejects_wrong_types() {
    let dir = home();
    let path = dir.join("manifest.json");
    std::fs::write(&path, json!({"version":1,"probes":[{"id":"typed-output","probe":"output-schema","tool":"status","access":"read_only","arguments":{}}]}).to_string()).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_mcpeval"))
        .args([
            "probe",
            "--server",
            "synthetic",
            "--gate-only",
            "--format",
            "json",
            "--manifest",
        ])
        .arg(&path)
        .args(["--", "python3"])
        .arg(fixture())
        .arg("wrong-output-type")
        .env("MCPEVAL_HOME", &dir)
        .output()
        .unwrap();
    assert_eq!(
        output.status.code(),
        Some(1),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let document: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        document["cases"][0]["reason"],
        "output-schema-invalid-result"
    );
    std::fs::remove_dir_all(dir).unwrap();
}
