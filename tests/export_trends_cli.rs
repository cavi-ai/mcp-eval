use chrono::TimeZone;
use std::process::Command;

use mcpeval::index;
use mcpeval::promote::{promote, PromotionConfig};
use mcpeval::record::{CallRecord, ErrorInfo};
use mcpeval::store::Store;
use serde_json::json;

const CLEAN: &str = "tests/fixtures/probe_clean_server.py";
const MANIFEST: &str = "tests/fixtures/mcp-eval.manifest.json";

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_mcpeval")
}

fn home() -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!("mcpeval-export-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&path).unwrap();
    path
}

fn probe_run(home: &std::path::Path) -> std::process::Output {
    Command::new(bin())
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
        .env("MCPEVAL_HOME", home)
        .output()
        .unwrap()
}

#[test]
fn trends_record_full_battery_runs_and_render_deltas() {
    let dir = home();
    let first = probe_run(&dir);
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    let second = probe_run(&dir);
    assert!(second.status.success());

    let rendered = Command::new(bin())
        .args(["trends"])
        .env("MCPEVAL_HOME", &dir)
        .output()
        .unwrap();
    assert!(rendered.status.success());
    let stdout = String::from_utf8(rendered.stdout).unwrap();
    assert!(stdout.contains("fixture"));
    assert!(stdout.contains("score=100/100 cases=7/7"));
    assert!(stdout.contains(" +0"), "{stdout}");
}

#[test]
fn trends_reports_empty_history_gracefully() {
    let dir = home();
    let rendered = Command::new(bin())
        .args(["trends"])
        .env("MCPEVAL_HOME", &dir)
        .output()
        .unwrap();
    assert!(rendered.status.success());
    assert!(String::from_utf8(rendered.stdout)
        .unwrap()
        .contains("no trend history yet"));
}

fn failure(session: &str, seq: u64, tool: &str, template_id: &str) -> CallRecord {
    CallRecord {
        ts: format!("2026-08-05T00:00:{seq:02}Z"),
        session: session.into(),
        seq,
        server: "demo".into(),
        method: "tools/call".into(),
        tool: Some(tool.into()),
        args: Some(json!({"shape": {"target": "str<32"}})),
        latency_ms: Some(1),
        outcome: "error".into(),
        error: Some(ErrorInfo {
            code: Some(json!("blocked")),
            layer: None,
            retryable: Some(false),
            kind: None,
            template: Some("CANARY raw template /Users/private".into()),
            template_id: Some(template_id.into()),
        }),
        shim_self_us: 1,
        kind: "real".into(),
    }
}

fn promoted_home() -> std::path::PathBuf {
    let dir = home();
    let mut store = Store::open(Some(dir.clone())).unwrap();
    for record in [
        failure("raw-session-one", 1, "click", "aaaaaaaaaaaaaaaa"),
        failure("raw-session-two", 1, "click", "aaaaaaaaaaaaaaaa"),
    ] {
        store.append(&record).unwrap();
    }
    index::build(&dir).unwrap();
    let promotion = promote(
        &dir,
        PromotionConfig {
            threshold: 0.1,
            now: chrono::Utc.with_ymd_and_hms(2026, 8, 5, 1, 0, 0).unwrap(),
        },
    )
    .unwrap();
    assert_eq!(promotion.findings, 1);
    dir
}

#[test]
fn export_issues_writes_one_github_ready_file_per_finding() {
    let dir = promoted_home();
    let out_dir = dir.join("issues");
    let output = Command::new(bin())
        .args(["export-issues", "--dir"])
        .arg(&out_dir)
        .env("MCPEVAL_HOME", &dir)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.starts_with("wrote 1 issue files"));

    let files: Vec<_> = std::fs::read_dir(&out_dir)
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(files.len(), 1);
    let body = std::fs::read_to_string(files[0].path()).unwrap();
    assert!(body.contains("# mcpeval finding finding-"));
    assert!(body.contains("**Suggested labels:** `mcpeval`, `"));
    assert!(body.contains("mcpeval generate --finding "));
    assert!(!body.contains("CANARY"));
}

#[test]
fn export_issues_refuses_a_populated_directory_without_force() {
    let dir = promoted_home();
    let out_dir = dir.join("issues");
    std::fs::create_dir_all(&out_dir).unwrap();
    std::fs::write(out_dir.join("keep.md"), "existing").unwrap();

    let denied = Command::new(bin())
        .args(["export-issues", "--dir"])
        .arg(&out_dir)
        .env("MCPEVAL_HOME", &dir)
        .output()
        .unwrap();
    assert!(!denied.status.success());
    assert!(String::from_utf8_lossy(&denied.stderr).contains("--force"));

    let forced = Command::new(bin())
        .args(["export-issues", "--dir"])
        .arg(&out_dir)
        .arg("--force")
        .env("MCPEVAL_HOME", &dir)
        .output()
        .unwrap();
    assert!(
        forced.status.success(),
        "{}",
        String::from_utf8_lossy(&forced.stderr)
    );
}
