use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::{json, Value};
use sha2::{Digest, Sha256};

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_mcpeval")
}

fn demo() -> &'static str {
    env!("CARGO_BIN_EXE_mcpeval-demo")
}

fn home() -> PathBuf {
    let path = std::env::temp_dir().join(format!("mcpeval-schema-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&path).unwrap();
    path
}

fn schema(name: &str) -> Value {
    let path = format!("docs/mcp-eval.{name}.schema.json");
    serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap()
}

fn validator(name: &str) -> jsonschema::Validator {
    let schema = schema(name);
    assert!(
        jsonschema::meta::is_valid(&schema),
        "{name} schema is not a valid schema"
    );
    jsonschema::validator_for(&schema).unwrap()
}

fn assert_valid(validator: &jsonschema::Validator, document: &Value) {
    let errors: Vec<String> = validator
        .iter_errors(document)
        .map(|error| format!("{} at {}", error, error.instance_path()))
        .collect();
    assert!(errors.is_empty(), "{errors:#?}\n{document:#}");
}

fn write_manifest(dir: &Path, manifest: &Value) -> PathBuf {
    let path = dir.join("m.json");
    std::fs::write(&path, serde_json::to_vec_pretty(manifest).unwrap()).unwrap();
    path
}

fn probe_json(dir: &Path, manifest: &Path, server_args: &[&str]) -> (Output, Value) {
    let output = Command::new(bin())
        .args(["probe", "--server", "demo", "--manifest"])
        .arg(manifest)
        .args(["--format", "json", "--"])
        .args(server_args)
        .env("MCPEVAL_HOME", dir)
        .output()
        .unwrap();
    let report = serde_json::from_slice(&output.stdout).unwrap_or_else(|_| {
        panic!(
            "no JSON report: {}",
            String::from_utf8_lossy(&output.stderr)
        )
    });
    (output, report)
}

#[test]
fn a_probe_report_carries_provenance_tool_hint_and_bound_detail() {
    let dir = home();
    let manifest = write_manifest(
        &dir,
        &json!({"version": 1, "probes": [
            {"id": "catalog", "probe": "discovery-cost", "access": "read_only",
             "max_tools": 3, "max_schema_bytes": 200000},
            {"id": "guess", "probe": "schema-guessability", "tool": "describe_status",
             "access": "read_only", "arguments": {}}
        ]}),
    );
    let (output, report) = probe_json(&dir, &manifest, &[demo()]);
    assert_eq!(output.status.code(), Some(1));

    assert_eq!(
        report["generator"],
        json!({"name": "mcpeval", "version": env!("CARGO_PKG_VERSION")})
    );
    let digest = Sha256::digest(std::fs::read(&manifest).unwrap());
    let hex: String = digest.iter().map(|byte| format!("{byte:02x}")).collect();
    assert_eq!(report["manifest_sha256"], hex);

    let catalog = &report["cases"][0];
    assert_eq!(catalog["tool"], Value::Null);
    assert_eq!(
        catalog["detail"],
        json!({"bound": "max_tools", "limit": 3, "observed": catalog["measurements"]["tool_count"]})
    );
    let explained = Command::new(bin())
        .args(["explain", "discovery-limit-exceeded"])
        .output()
        .unwrap();
    assert_eq!(
        catalog["hint"].as_str().unwrap(),
        String::from_utf8(explained.stdout).unwrap().trim_end()
    );

    let guess = &report["cases"][1];
    assert_eq!(guess["tool"], "describe_status");
    assert_eq!(guess["hint"], Value::Null);
    assert_eq!(guess["detail"], Value::Null);

    assert_valid(&validator("probe-report"), &report);
}

#[test]
fn every_bound_detail_names_its_limit_and_validates() {
    let report_schema = validator("probe-report");
    let dir = home();
    let manifest = write_manifest(
        &dir,
        &json!({"version": 1, "probes": [
            {"id": "tokens", "probe": "token-cost", "access": "read_only",
             "max_total_tokens": 100000, "max_tool_tokens": 1},
            {"id": "slow", "probe": "latency-budget", "tool": "slow_read", "access": "read_only",
             "arguments": {}, "attempts": 2, "max_latency_ms": 100},
            {"id": "pages", "probe": "pagination", "access": "read_only", "max_pages": 2}
        ]}),
    );
    let (_, report) = probe_json(&dir, &manifest, &[demo(), "--broken", "stalled-cursor"]);
    assert_valid(&report_schema, &report);
    let cases = report["cases"].as_array().unwrap();

    let tokens = &cases[0];
    assert_eq!(tokens["detail"]["bound"], "max_tool_tokens");
    assert_eq!(tokens["detail"]["limit"], 1);
    assert_eq!(
        tokens["detail"]["observed"],
        tokens["measurements"]["per_tool"][0]["tokens"]
    );

    let slow = &cases[1];
    assert_eq!(slow["detail"]["bound"], "max_latency_ms");
    assert_eq!(slow["detail"]["limit"], 100);
    assert_eq!(
        slow["detail"]["observed"],
        slow["measurements"]["latency_ms"]
    );

    let pages = &cases[2];
    assert_eq!(pages["reason"], "pagination-stalled-cursor");
    assert_eq!(
        pages["detail"],
        json!({"bound": "max_pages", "limit": 2, "observed": pages["measurements"]["pages"]})
    );
}

#[test]
fn errored_reports_and_diffs_validate_and_bogus_documents_do_not() {
    let report_schema = validator("probe-report");
    let diff_schema = validator("probe-diff");
    let dir = home();
    let manifest = write_manifest(
        &dir,
        &json!({"version": 1, "probes": [
            {"id": "pages", "probe": "pagination", "access": "read_only", "max_pages": 5}
        ]}),
    );
    let (_, duplicate) = probe_json(&dir, &manifest, &[demo(), "--broken", "duplicate-page"]);
    let (_, stalled) = probe_json(&dir, &manifest, &[demo(), "--broken", "stalled-cursor"]);
    assert_valid(&report_schema, &duplicate);
    std::fs::write(dir.join("a.json"), duplicate.to_string()).unwrap();
    std::fs::write(dir.join("b.json"), stalled.to_string()).unwrap();
    let diff = Command::new(bin())
        .args(["diff", "a.json", "b.json", "--format", "json"])
        .current_dir(&dir)
        .output()
        .unwrap();
    let diff: Value = serde_json::from_slice(&diff.stdout).unwrap();
    assert_eq!(diff["cases"][0]["verdict"], "changed");
    assert_valid(&diff_schema, &diff);

    let crash = write_manifest(
        &dir,
        &json!({"version": 1, "probes": [
            {"id": "crashes", "probe": "latency-budget", "tool": "crash", "access": "read_only",
             "arguments": {}, "attempts": 2, "max_latency_ms": 1000}
        ]}),
    );
    let (output, errored) = probe_json(
        &dir,
        &crash,
        &["python3", "tests/fixtures/transport_fault_server.py"],
    );
    assert_eq!(output.status.code(), Some(3));
    assert_eq!(errored["cases"][0]["reason"], "transport-closed");
    assert_valid(&report_schema, &errored);

    let mut bogus = duplicate.clone();
    bogus["cases"][0]["reason"] = json!("not-a-reason");
    assert!(!report_schema.is_valid(&bogus));
    let mut bogus = diff.clone();
    bogus["cases"][0]["verdict"] = json!("sideways");
    assert!(!diff_schema.is_valid(&bogus));
}

#[test]
fn schema_command_prints_each_published_schema() {
    for (argument, name) in [
        (None, "manifest"),
        (Some("manifest"), "manifest"),
        (Some("report"), "probe-report"),
        (Some("diff"), "probe-diff"),
    ] {
        let mut command = Command::new(bin());
        command.arg("schema");
        if let Some(argument) = argument {
            command.arg(argument);
        }
        let output = command.output().unwrap();
        assert!(output.status.success(), "{argument:?}");
        let printed: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(printed, schema(name), "{argument:?}");
    }
}

#[test]
fn the_manifest_schema_accepts_every_fixture_manifest() {
    let manifest_schema = validator("manifest");
    for path in [
        "tests/fixtures/mcp-eval.manifest.json",
        "tests/fixtures/action-sarif.manifest.json",
    ] {
        let manifest: Value =
            serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        assert_valid(&manifest_schema, &manifest);
    }
    let timed = json!({"version": 1, "timeout_ms": 1000, "probes": [
        {"id": "a", "probe": "pagination", "access": "read_only", "max_pages": 2}
    ]});
    assert_valid(&manifest_schema, &timed);
    let unknown = json!({"version": 1, "timeout_ms": 1000, "probes": [
        {"id": "a", "probe": "pagination", "access": "read_only", "max_pages": 2, "extra": 1}
    ]});
    assert!(!manifest_schema.is_valid(&unknown));
}
