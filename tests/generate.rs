use std::path::{Path, PathBuf};

use chrono::{TimeZone, Utc};
use mcpeval::generate;
use mcpeval::index;
use mcpeval::promote::{promote, PromotionConfig};
use mcpeval::record::{CallRecord, ErrorInfo};
use mcpeval::store::Store;
use serde_json::json;

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
