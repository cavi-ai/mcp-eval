use chrono::{TimeZone, Utc};
use mcpeval::index;
use mcpeval::promote::{promote, PromotionConfig};
use mcpeval::record::{CallRecord, ErrorInfo};
use mcpeval::store::Store;
use serde_json::json;
use std::process::Command;

const MANIFEST: &str = "tests/fixtures/mcp-eval.manifest.json";
const CLEAN: &str = "tests/fixtures/probe_clean_server.py";
const BROKEN: &str = "tests/fixtures/probe_broken_server.py";

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_mcpeval")
}

fn promoted_home() -> (std::path::PathBuf, String) {
    let dir = std::env::temp_dir().join(format!("mcpeval-lifecycle-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut store = Store::open(Some(dir.clone())).unwrap();
    for (session, seq) in [("first", 1), ("second", 1)] {
        store
            .append(&CallRecord {
                ts: format!("2026-08-05T00:00:{seq:02}Z"),
                session: session.into(),
                seq,
                server: "fixture".into(),
                method: "tools/call".into(),
                tool: Some("describe_status".into()),
                args: Some(json!({})),
                latency_ms: Some(1),
                outcome: "error".into(),
                error: Some(ErrorInfo {
                    code: Some(json!("broken")),
                    layer: None,
                    retryable: Some(false),
                    kind: None,
                    template: None,
                    template_id: Some("aaaaaaaaaaaaaaaa".into()),
                }),
                shim_self_us: 1,
                kind: "real".into(),
            })
            .unwrap();
    }
    index::build(&dir).unwrap();
    promote(
        &dir,
        PromotionConfig {
            threshold: 0.0,
            now: Utc.with_ymd_and_hms(2026, 8, 5, 1, 0, 0).unwrap(),
        },
    )
    .unwrap();
    let db = rusqlite::Connection::open(dir.join("index.db")).unwrap();
    let id = db
        .query_row("SELECT finding_id FROM findings", [], |row| row.get(0))
        .unwrap();
    (dir, id)
}

fn verify(home: &std::path::Path, id: &str, server: &str) -> std::process::Output {
    Command::new(bin())
        .args([
            "verify",
            "--finding",
            id,
            "--case",
            "literal-status",
            "--manifest",
            MANIFEST,
            "--",
            "python3",
            server,
        ])
        .env("MCPEVAL_HOME", home)
        .output()
        .unwrap()
}

#[test]
fn broken_and_clean_fixtures_drive_close_and_regression_with_history_intact() {
    let (home, id) = promoted_home();
    let broken = verify(&home, &id, BROKEN);
    assert!(!broken.status.success());
    assert!(String::from_utf8_lossy(&broken.stdout).contains("state=fix-claimed"));

    for expected in ["state=verifying", "state=verifying", "state=closed"] {
        let clean = verify(&home, &id, CLEAN);
        assert!(
            clean.status.success(),
            "{}",
            String::from_utf8_lossy(&clean.stderr)
        );
        assert!(String::from_utf8_lossy(&clean.stdout).contains(expected));
    }
    let regression = verify(&home, &id, BROKEN);
    assert!(!regression.status.success());
    assert!(String::from_utf8_lossy(&regression.stdout).contains("state=open"));

    let db = rusqlite::Connection::open(home.join("index.db")).unwrap();
    let row: (String, i64, i64) = db
        .query_row(
            "SELECT state,consecutive_passes,(SELECT COUNT(*) FROM probe_history WHERE finding_id=?1)
             FROM finding_lifecycle WHERE finding_id=?1",
            [&id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(row, ("open".into(), 0, 5));

    promote(
        &home,
        PromotionConfig {
            threshold: 0.0,
            now: Utc.with_ymd_and_hms(2026, 8, 6, 1, 0, 0).unwrap(),
        },
    )
    .unwrap();
    let retained: (String, i64) = db
        .query_row(
            "SELECT state,(SELECT COUNT(*) FROM probe_history WHERE finding_id=?1)
             FROM finding_lifecycle WHERE finding_id=?1",
            [&id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(retained, ("open".into(), 5));
}

#[test]
fn unknown_finding_and_tool_mismatch_fail_before_child_launch() {
    let (home, id) = promoted_home();
    let marker = home.join("launched");
    let output = Command::new(bin())
        .args([
            "verify",
            "--finding",
            "finding-0000000000000000",
            "--case",
            "literal-status",
            "--manifest",
            MANIFEST,
            "--",
            "sh",
            "-c",
        ])
        .arg(format!("touch {}", marker.display()))
        .env("MCPEVAL_HOME", &home)
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(!marker.exists());

    let output = Command::new(bin())
        .args([
            "verify",
            "--finding",
            &id,
            "--case",
            "repeat-read",
            "--manifest",
            MANIFEST,
            "--",
            "sh",
            "-c",
        ])
        .arg(format!("touch {}", marker.display()))
        .env("MCPEVAL_HOME", &home)
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(!marker.exists());
}
