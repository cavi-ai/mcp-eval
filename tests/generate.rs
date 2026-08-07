use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use chrono::{TimeZone, Utc};
use mcpeval::generate;
use mcpeval::index;
use mcpeval::promote::{promote, PromotionConfig};
use mcpeval::record::{CallRecord, ErrorInfo};
use mcpeval::store::Store;
use serde_json::json;

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_mcpeval")
}

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("mcpeval-generate-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&path).unwrap();
        Self { path }
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

fn call(session: &str, arguments: serde_json::Value) -> CallRecord {
    CallRecord {
        ts: "2026-08-05T12:00:00Z".into(),
        session: session.into(),
        seq: 1,
        server: "synthetic-canary-server".into(),
        method: "tools/call".into(),
        tool: Some("read_status".into()),
        args: Some(arguments),
        latency_ms: Some(5),
        outcome: "error".into(),
        error: Some(ErrorInfo {
            code: Some(json!("blocked")),
            layer: None,
            retryable: Some(false),
            kind: None,
            template: Some("synthetic-canary-error".into()),
            template_id: Some("aaaaaaaaaaaaaaaa".into()),
        }),
        shim_self_us: 1,
        kind: "real".into(),
    }
}

fn promoted_finding(root: &Path, arguments: serde_json::Value) -> String {
    let mut store = Store::open(Some(root.to_owned())).unwrap();
    store
        .append(&call("first-session", arguments.clone()))
        .unwrap();
    store.append(&call("second-session", arguments)).unwrap();
    index::build(root).unwrap();
    promote(
        root,
        PromotionConfig {
            threshold: 0.0,
            now: Utc.with_ymd_and_hms(2026, 8, 6, 0, 0, 0).unwrap(),
        },
    )
    .unwrap();

    rusqlite::Connection::open(root.join("index.db"))
        .unwrap()
        .query_row("SELECT finding_id FROM findings", [], |row| row.get(0))
        .unwrap()
}

fn generate_cli(root: &Path, finding_id: &str, output: &Path, force: bool) -> Output {
    let mut command = Command::new(bin());
    command
        .args([
            "generate",
            "--finding",
            finding_id,
            "--output",
            output.to_str().unwrap(),
        ])
        .env("MCPEVAL_HOME", root);
    if force {
        command.arg("--force");
    }
    command.output().unwrap()
}

#[test]
fn generates_a_deterministic_read_only_manifest_for_empty_arguments() {
    let root = TempDir::new();
    let finding_id = promoted_finding(&root.path, json!({}));
    let output = root.path.join("generated.json");

    let probe_id = generate::run(&root.path, &finding_id, &output, false).unwrap();
    let body = std::fs::read_to_string(&output).unwrap();
    let expected = format!(
        "{{\n  \"version\": 1,\n  \"sandboxes\": {{}},\n  \"probes\": [\n    {{\n      \"probe\": \"degradation-over-n\",\n      \"id\": \"{finding_id}\",\n      \"tool\": \"read_status\",\n      \"access\": \"read_only\",\n      \"sandbox\": null,\n      \"arguments\": {{}},\n      \"max_attempts\": 3\n    }}\n  ]\n}}\n"
    );

    assert_eq!(probe_id, finding_id);
    assert_eq!(body, expected);
    assert!(!body.contains("synthetic-canary"));
    mcpeval::manifest::Manifest::load(&output).unwrap();
}

#[test]
fn creates_new_files_and_replaces_existing_files_only_when_forced() {
    let root = TempDir::new();
    let finding_id = promoted_finding(&root.path, json!({}));
    let output = root.path.join("generated.json");
    std::fs::write(&output, "existing manifest\n").unwrap();

    assert!(generate::run(&root.path, &finding_id, &output, false).is_err());
    assert_eq!(
        std::fs::read_to_string(&output).unwrap(),
        "existing manifest\n"
    );

    generate::run(&root.path, &finding_id, &output, true).unwrap();
    assert_ne!(
        std::fs::read_to_string(&output).unwrap(),
        "existing manifest\n"
    );
}

#[test]
fn rejects_promoted_findings_without_a_tool() {
    let root = TempDir::new();
    let finding_id = promoted_finding(&root.path, json!({}));
    rusqlite::Connection::open(root.path.join("index.db"))
        .unwrap()
        .execute(
            "UPDATE issues SET tool=NULL WHERE finding_id=?1",
            [&finding_id],
        )
        .unwrap();

    assert!(generate::run(
        &root.path,
        &finding_id,
        &root.path.join("generated.json"),
        false
    )
    .is_err());
}

#[test]
fn rejects_promoted_findings_with_non_empty_arguments_without_leaking_them() {
    let root = TempDir::new();
    let finding_id = promoted_finding(&root.path, json!({"target": "synthetic-canary-value"}));
    let error = generate::run(
        &root.path,
        &finding_id,
        &root.path.join("generated.json"),
        false,
    )
    .unwrap_err()
    .to_string();

    assert!(!error.contains("synthetic-canary-value"));
}

#[test]
fn generate_cli_prints_only_the_generated_probe_for_an_eligible_finding() {
    let root = TempDir::new();
    let finding_id = promoted_finding(&root.path, json!({}));
    let output = root.path.join("generated.json");

    let result = generate_cli(&root.path, &finding_id, &output, false);

    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(
        String::from_utf8(result.stdout).unwrap(),
        format!("generated probe={finding_id}\n")
    );
    assert!(result.stderr.is_empty());
    mcpeval::manifest::Manifest::load(&output).unwrap();
}

#[test]
fn generate_cli_writes_the_requested_manifest_without_creating_a_store_directory() {
    let root = TempDir::new();
    let finding_id = promoted_finding(&root.path, json!({}));
    std::fs::remove_dir_all(root.path.join("store")).unwrap();
    let output = root.path.join("generated.json");

    let result = generate_cli(&root.path, &finding_id, &output, false);

    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(output.is_file());
    assert!(!root.path.join("store").exists());
}

#[test]
fn generate_cli_rejects_ineligible_findings_without_canaries_or_absolute_paths() {
    let root = TempDir::new();
    let finding_id = promoted_finding(&root.path, json!({"target": "synthetic-canary-value"}));
    let output = root.path.join("generated.json");

    let result = generate_cli(&root.path, &finding_id, &output, false);
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );

    assert!(!result.status.success());
    assert!(text.contains("finding arguments must be exactly {}"));
    assert!(!text.contains("synthetic-canary"));
    assert!(!text.contains(root.path.to_str().unwrap()));
}

#[test]
fn generate_cli_requires_force_to_replace_an_existing_output() {
    let root = TempDir::new();
    let finding_id = promoted_finding(&root.path, json!({}));
    let output = root.path.join("generated.json");
    std::fs::write(&output, "existing manifest\n").unwrap();

    let denied = generate_cli(&root.path, &finding_id, &output, false);

    assert!(!denied.status.success());
    assert_eq!(
        std::fs::read_to_string(&output).unwrap(),
        "existing manifest\n"
    );

    let forced = generate_cli(&root.path, &finding_id, &output, true);

    assert!(
        forced.status.success(),
        "{}",
        String::from_utf8_lossy(&forced.stderr)
    );
    assert_eq!(
        String::from_utf8(forced.stdout).unwrap(),
        format!("generated probe={finding_id}\n")
    );
}
