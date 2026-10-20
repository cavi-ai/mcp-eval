use serde_json::{json, Value};
use std::process::Command;

fn probe(case: Value, mode: &str) -> (std::process::Output, Value) {
    let home = std::env::temp_dir().join(format!("mcpeval-contract-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&home).unwrap();
    let manifest = home.join("manifest.json");
    let document = json!({"version":1,"timeout_ms":2000,"probes":[case]});
    let schema: Value =
        serde_json::from_str(include_str!("../docs/mcp-eval.manifest.schema.json")).unwrap();
    assert!(jsonschema::validator_for(&schema)
        .unwrap()
        .is_valid(&document));
    std::fs::write(&manifest, document.to_string()).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_mcpeval"))
        .args([
            "probe",
            "--server",
            "fixture",
            "--gate-only",
            "--format",
            "json",
            "--manifest",
        ])
        .arg(&manifest)
        .args([
            "--",
            "python3",
            "tests/fixtures/probe_contract_server.py",
            mode,
        ])
        .env("MCPEVAL_HOME", &home)
        .output()
        .unwrap();
    let report = serde_json::from_slice(&output.stdout).unwrap_or(Value::Null);
    if !report.is_null() {
        let schema: Value =
            serde_json::from_str(include_str!("../docs/mcp-eval.probe-report.schema.json"))
                .unwrap();
        assert!(jsonschema::validator_for(&schema)
            .unwrap()
            .is_valid(&report));
    }
    std::fs::remove_dir_all(home).unwrap();
    (output, report)
}

#[test]
fn bounded_null_id_rejections_require_valid_errors_and_session_recovery() {
    let case = json!({"id":"bound","probe":"payload-bounds","access":"read_only","tool":"read","arguments":{},"field":"blob","size_bytes":1000000,"expect_handled":false});
    for mode in ["reject", "parse-reject", "reject-meta"] {
        let (output, _) = probe(case.clone(), mode);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stdout)
        );
    }
    for mode in [
        "wrong-id",
        "null-application-error",
        "null-malformed-error",
        "reject-exit",
    ] {
        let (output, _) = probe(case.clone(), mode);
        assert!(!output.status.success(), "{mode}");
    }
    let mut required = case;
    required["expect_handled"] = json!(true);
    assert!(!probe(required, "reject").0.status.success());
}

#[test]
fn retryability_uses_error_data_and_rejects_missing_or_conflicting_hints() {
    let case = json!({"id":"permanent","probe":"error-honesty","access":"read_only","tool":"read","arguments":{},"max_attempts":2,"expect_retryable":false});
    assert!(probe(case.clone(), "retry-data").0.status.success());
    for mode in ["retry-missing", "retry-conflict"] {
        let (output, report) = probe(case.clone(), mode);
        assert!(!output.status.success());
        assert_eq!(report["cases"][0]["reason"], "retryability-mismatch");
    }
}

#[test]
fn optional_cancellation_reports_completion_and_tolerates_late_replies() {
    let case = json!({"id":"optional","probe":"cancellation","access":"read_only","tool":"read","arguments":{},"grace_seconds":1,"reason":"probe","require_honored":false});
    for (mode, expected) in [
        ("cancel-completed", "completed"),
        ("cancel-late", "no-result"),
    ] {
        let (output, report) = probe(case.clone(), mode);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            report["cases"][0]["measurements"]["cancellation_outcome"],
            expected
        );
        assert_eq!(report["cases"][0]["attempts"], 3);
        let parsed = mcpeval::probe::ProbeReport::from_json_document(&report).unwrap();
        assert_eq!(
            parsed.to_json("fixture")["cases"][0]["measurements"]["cancellation_outcome"],
            expected
        );
    }
    for mode in [
        "cancel-error",
        "cancel-tool-error",
        "cancel-recovery-error",
        "cancel-exit",
    ] {
        assert!(!probe(case.clone(), mode).0.status.success(), "{mode}");
    }
}
