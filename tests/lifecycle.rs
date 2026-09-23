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

#[test]
fn a_verification_that_loses_its_server_leaves_the_lifecycle_untouched() {
    let (home, id) = promoted_home();
    let output = Command::new(bin())
        .args([
            "verify",
            "--finding",
            &id,
            "--case",
            "literal-status",
            "--manifest",
            MANIFEST,
            "--",
            "python3",
            "tests/fixtures/transport_fault_server.py",
            "describe_status",
        ])
        .env("MCPEVAL_HOME", &home)
        .output()
        .unwrap();
    assert_eq!(
        output.status.code(),
        Some(3),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("not verified") && stdout.contains("transport-closed"),
        "{stdout}"
    );
    let db = rusqlite::Connection::open(home.join("index.db")).unwrap();
    let history: i64 = db
        .query_row(
            "SELECT COUNT(*) FROM probe_history WHERE finding_id=?1",
            [&id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        history, 0,
        "an unevaluated case is no verification evidence"
    );
}

fn shim_demo_session(home: &std::path::Path, session: &str) {
    use std::io::{BufRead, BufReader, Write};
    let mut child = Command::new(bin())
        .args(["shim", "--server", "demo", "--"])
        .arg(env!("CARGO_BIN_EXE_mcpeval-demo"))
        .env("MCPEVAL_HOME", home)
        .env("MCPEVAL_SESSION", session)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap());
    let mut frames = vec![
        (
            json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"lifecycle-test","version":"0"}}}),
            true,
        ),
        (
            json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
            false,
        ),
        (json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}), true),
    ];
    for id in 3..=8 {
        frames.push((
            json!({"jsonrpc":"2.0","id":id,"method":"tools/call","params":{"name":"flaky_read","arguments":{}}}),
            true,
        ));
    }
    for (frame, answered) in frames {
        writeln!(stdin, "{frame}").unwrap();
        stdin.flush().unwrap();
        if answered {
            let mut line = String::new();
            stdout.read_line(&mut line).unwrap();
            assert!(line.contains("\"jsonrpc\""), "{line}");
        }
    }
    drop(stdin);
    assert!(child.wait().unwrap().success());
}

fn describe_status_call(session: &str, seq: u64, failed: bool) -> CallRecord {
    CallRecord {
        ts: format!("2026-08-05T00:00:{seq:02}Z"),
        session: session.into(),
        seq,
        server: "demo".into(),
        method: "tools/call".into(),
        tool: Some("describe_status".into()),
        args: Some(json!({})),
        latency_ms: Some(1),
        outcome: if failed { "error" } else { "ok" }.into(),
        error: failed.then(|| ErrorInfo {
            code: Some(json!(-32000)),
            layer: None,
            retryable: Some(false),
            kind: None,
            template: None,
            template_id: Some("bbbbbbbbbbbbbbbb".into()),
        }),
        shim_self_us: 1,
        kind: "real".into(),
    }
}

#[test]
fn a_captured_finding_generates_a_probe_sized_to_its_rate_that_verify_explains_and_closes() {
    let home = std::env::temp_dir().join(format!("mcpeval-lifecycle-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&home).unwrap();
    for session in ["one", "two", "three"] {
        shim_demo_session(&home, session);
    }
    let mut store = Store::open(Some(home.clone())).unwrap();
    for session in ["hand-one", "hand-two"] {
        for (seq, failed) in [(1, true), (2, false), (3, false)] {
            store
                .append(&describe_status_call(session, seq, failed))
                .unwrap();
        }
    }
    let run = |args: &[&str]| {
        let out = Command::new(bin())
            .args(args)
            .current_dir(&home)
            .env("MCPEVAL_HOME", &home)
            .output()
            .unwrap();
        (out.status.code(), String::from_utf8(out.stdout).unwrap())
    };
    assert_eq!(run(&["index"]).0, Some(0));
    assert_eq!(run(&["promote", "--threshold", "0"]).0, Some(0));
    let findings: Vec<serde_json::Value> =
        serde_json::from_str(&run(&["findings", "--format", "json"]).1).unwrap();
    let finding = |tool: &str| {
        findings
            .iter()
            .find(|finding| finding["tool"] == tool)
            .unwrap()
            .clone()
    };
    let flaky = finding("flaky_read");
    assert_eq!(
        (&flaky["class"], &flaky["retryable"], &flaky["rate"]),
        (&json!("recovers-on-retry"), &json!(true), &json!(1.0 / 3.0))
    );

    let flaky_id = flaky["finding_id"].as_str().unwrap();
    assert_eq!(
        run(&[
            "generate",
            "--finding",
            flaky_id,
            "--confirm-read-only",
            "--output",
            "flaky.json",
        ]),
        (
            Some(0),
            format!("{flaky_id}\nprobe=degradation-over-n max_attempts=8\n")
        )
    );
    let demo = env!("CARGO_BIN_EXE_mcpeval-demo");
    let verify_generated = |id: &str, manifest: &str| {
        run(&[
            "verify",
            "--finding",
            id,
            "--case",
            id,
            "--manifest",
            manifest,
            "--",
            demo,
        ])
    };
    assert_eq!(
        verify_generated(flaky_id, "flaky.json"),
        (
            Some(1),
            format!(
                "{flaky_id} state=fix-claimed probe={flaky_id} consecutive_passes=0 \
                 reason=unexpected-outcome\n  hint: {}\n",
                mcpeval::remediation::hint(mcpeval::probe::FailureReason::UnexpectedOutcome)
            )
        )
    );

    let status_id = finding("describe_status")["finding_id"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(
        run(&[
            "generate",
            "--finding",
            &status_id,
            "--confirm-read-only",
            "--output",
            "status.json",
        ]),
        (
            Some(0),
            format!("{status_id}\nprobe=degradation-over-n max_attempts=8\n")
        )
    );
    for (state, passes) in [("verifying", 1), ("verifying", 2), ("closed", 3)] {
        assert_eq!(
            verify_generated(&status_id, "status.json"),
            (
                Some(0),
                format!(
                    "{status_id} state={state} probe={status_id} consecutive_passes={passes}\n"
                )
            )
        );
    }
}
