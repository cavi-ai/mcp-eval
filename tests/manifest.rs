use std::io::Write;

use mcpeval::manifest::Manifest;
use serde_json::{json, Value};

fn valid_manifest() -> Value {
    json!({
        "version": 1,
        "sandboxes": {"fixture": {"description": "disposable state"}},
        "probes": [
            {
                "id": "repeat-read",
                "probe": "degradation-over-n",
                "tool": "read_counter",
                "access": "read_only",
                "arguments": {},
                "max_attempts": 5
            },
            {
                "id": "reset-fixture",
                "probe": "instruction-fidelity",
                "tool": "reset_counter",
                "access": "mutating",
                "sandbox": "fixture",
                "arguments": {},
                "expect": {
                    "outcome": "ok",
                    "required_result_fields": ["reset"],
                    "equals": {"reset": true}
                }
            }
        ]
    })
}

fn parse(value: &Value) -> anyhow::Result<Manifest> {
    let mut file = tempfile();
    serde_json::to_writer(&mut file, value).unwrap();
    file.flush().unwrap();
    Manifest::load(&file.path)
}

struct TempFile {
    path: std::path::PathBuf,
}

fn tempfile() -> TempFile {
    let path = std::env::temp_dir().join(format!("mcpeval-manifest-{}.json", uuid::Uuid::new_v4()));
    TempFile { path }
}

impl Write for TempFile {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?
            .write(buf)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

#[test]
fn accepts_a_strict_version_one_manifest() {
    let manifest = parse(&valid_manifest()).unwrap();
    assert_eq!(manifest.probes.len(), 2);
}

#[test]
fn rejects_unknown_fields_and_unsupported_versions() {
    let mut unknown = valid_manifest();
    unknown["mutation_allowed"] = json!(true);
    assert!(parse(&unknown).is_err());

    let mut version = valid_manifest();
    version["version"] = json!(2);
    assert!(parse(&version).is_err());
}

#[test]
fn rejects_duplicate_or_content_bearing_identifiers() {
    let mut duplicate = valid_manifest();
    duplicate["probes"][1]["id"] = json!("repeat-read");
    assert!(parse(&duplicate).is_err());

    let mut invalid = valid_manifest();
    invalid["probes"][0]["id"] = json!("CANARY invalid id");
    let error = parse(&invalid).unwrap_err().to_string();
    assert!(!error.contains("CANARY"));

    let mut tool = valid_manifest();
    tool["probes"][0]["tool"] = json!("invalid/tool");
    assert!(parse(&tool).is_err());
}

#[test]
fn mutating_cases_require_a_declared_sandbox_and_read_only_cases_forbid_one() {
    let mut missing = valid_manifest();
    missing["probes"][1]
        .as_object_mut()
        .unwrap()
        .remove("sandbox");
    assert!(parse(&missing).is_err());

    let mut undefined = valid_manifest();
    undefined["probes"][1]["sandbox"] = json!("elsewhere");
    assert!(parse(&undefined).is_err());

    let mut read_only = valid_manifest();
    read_only["probes"][0]["sandbox"] = json!("fixture");
    assert!(parse(&read_only).is_err());
}

#[test]
fn degradation_attempts_are_bounded() {
    for attempts in [1, 101] {
        let mut value = valid_manifest();
        value["probes"][0]["max_attempts"] = json!(attempts);
        assert!(parse(&value).is_err());
    }
}

#[test]
fn fidelity_expectations_are_structural_and_outcome_specific() {
    let mut prose = valid_manifest();
    prose["probes"][1]["expect"]["equals"]["reset"] = json!("CANARY prose value");
    let error = parse(&prose).unwrap_err().to_string();
    assert!(!error.contains("CANARY"));

    let mut error_fields = valid_manifest();
    error_fields["probes"][1]["expect"] = json!({
        "outcome": "error",
        "required_result_fields": ["reset"],
        "error_code": -32000
    });
    assert!(parse(&error_fields).is_err());

    let mut success_code = valid_manifest();
    success_code["probes"][1]["expect"]["error_code"] = json!(-32000);
    assert!(parse(&success_code).is_err());
}

#[test]
fn discovery_cost_is_read_only_and_has_bounded_limits() {
    let mut value = valid_manifest();
    value["probes"] = json!([{
        "id": "bounded-discovery",
        "probe": "discovery-cost",
        "access": "read_only",
        "max_tools": 10,
        "max_schema_bytes": 1000
    }]);
    assert!(parse(&value).is_ok());

    for (field, invalid) in [("max_tools", 0), ("max_schema_bytes", 10_000_001)] {
        let mut invalid_value = value.clone();
        invalid_value["probes"][0][field] = json!(invalid);
        assert!(parse(&invalid_value).is_err());
    }

    value["probes"][0]["access"] = json!("mutating");
    assert!(parse(&value).is_err());
}
