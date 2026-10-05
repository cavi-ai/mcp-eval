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
                identity: None,
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
    verify_manifest(home, id, server, std::path::Path::new(MANIFEST))
}

fn verify_manifest(
    home: &std::path::Path,
    id: &str,
    server: &str,
    manifest: &std::path::Path,
) -> std::process::Output {
    Command::new(bin())
        .args([
            "verify",
            "--finding",
            id,
            "--case",
            "literal-status",
            "--manifest",
            manifest.to_str().unwrap(),
            "--",
            "python3",
            server,
        ])
        .env("MCPEVAL_HOME", home)
        .output()
        .unwrap()
}

fn changed_manifest(
    home: &std::path::Path,
    change: impl FnOnce(&mut serde_json::Value),
) -> std::path::PathBuf {
    let mut manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(MANIFEST).unwrap()).unwrap();
    change(&mut manifest);
    let path = home.join("changed-manifest.json");
    std::fs::write(&path, serde_json::to_vec_pretty(&manifest).unwrap()).unwrap();
    path
}

#[test]
fn changing_a_definition_under_the_same_case_id_resets_the_pass_streak() {
    let (home, id) = promoted_home();
    for _ in 0..2 {
        assert!(verify(&home, &id, CLEAN).status.success());
    }
    let path = changed_manifest(&home, |manifest| {
        manifest["probes"][6]["expect"]["required_result_fields"] = json!(["status", "content"]);
    });
    // The clean server supplies both fields, so the changed case still passes.
    let output = verify_manifest(&home, &id, CLEAN, &path);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("consecutive_passes=1"));
}

#[test]
fn changed_timeout_resets_credit_but_unrelated_cases_do_not() {
    let (home, id) = promoted_home();
    assert!(verify(&home, &id, CLEAN).status.success());
    let unrelated = changed_manifest(&home, |manifest| {
        manifest["probes"][3]["max_total_tokens"] = json!(99999);
    });
    let second = verify_manifest(&home, &id, CLEAN, &unrelated);
    assert!(second.status.success());
    assert!(String::from_utf8_lossy(&second.stdout).contains("consecutive_passes=2"));
    let timeout = changed_manifest(&home, |manifest| {
        manifest["timeout_ms"] = json!(5000);
    });
    let fresh = verify_manifest(&home, &id, CLEAN, &timeout);
    assert!(fresh.status.success());
    assert!(String::from_utf8_lossy(&fresh.stdout).contains("consecutive_passes=1"));
}

#[test]
fn deleting_the_derived_index_preserves_verification_evidence() {
    let (home, id) = promoted_home();
    for _ in 0..2 {
        assert!(verify(&home, &id, CLEAN).status.success());
    }
    std::fs::remove_file(home.join("index.db")).unwrap();
    index::build(&home).unwrap();
    promote(
        &home,
        PromotionConfig {
            threshold: 0.0,
            now: Utc::now(),
        },
    )
    .unwrap();
    let output = verify(&home, &id, CLEAN);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("state=closed"));
    let db = rusqlite::Connection::open(home.join("index.db")).unwrap();
    let count: i64 = db
        .query_row(
            "SELECT COUNT(*) FROM probe_history WHERE finding_id=?1",
            [&id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 3);
}

#[test]
fn legacy_unbound_passes_do_not_close_a_new_verification() {
    let (home, id) = promoted_home();
    let db = rusqlite::Connection::open(home.join("index.db")).unwrap();
    db.execute_batch(
        "DROP TABLE probe_history; DROP TABLE finding_lifecycle;
        CREATE TABLE finding_lifecycle (finding_id TEXT PRIMARY KEY, server TEXT NOT NULL,
        tool TEXT, err_code TEXT, err_template_id TEXT, probe_id TEXT, state TEXT NOT NULL,
        consecutive_passes INTEGER NOT NULL, updated_at TEXT NOT NULL);
        CREATE TABLE probe_history (id INTEGER PRIMARY KEY, finding_id TEXT NOT NULL,
        probe_id TEXT NOT NULL, passed INTEGER NOT NULL, ts TEXT NOT NULL);",
    )
    .unwrap();
    db.execute(
        "INSERT INTO finding_lifecycle VALUES (?1,'fixture','describe_status',NULL,
        'aaaaaaaaaaaaaaaa','literal-status','verifying',2,'2026-08-05T00:00:00Z')",
        [&id],
    )
    .unwrap();
    db.execute("INSERT INTO probe_history(finding_id,probe_id,passed,ts) VALUES (?1,'literal-status',1,'2026-08-05T00:00:00Z')", [&id]).unwrap();
    // Simulate a pre-upgrade store: the cache is the only copy of its evidence.
    let evidence = home.join("lifecycle.db");
    if evidence.exists() {
        std::fs::remove_file(&evidence).unwrap();
    }
    db.execute("UPDATE finding_lifecycle SET probe_id='literal-status',state='verifying',consecutive_passes=2 WHERE finding_id=?1", [&id]).unwrap();
    let output = verify(&home, &id, CLEAN);
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("consecutive_passes=1"));
    let durable = rusqlite::Connection::open(evidence).unwrap();
    let rows: (i64, i64) = durable
        .query_row(
            "SELECT COUNT(*),SUM(definition_id IS NULL) FROM probe_history",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(rows, (2, 1));
}

#[test]
fn formatting_and_object_key_order_do_not_invalidate_the_binding() {
    let (home, id) = promoted_home();
    let path = changed_manifest(&home, |manifest| {
        manifest["probes"][6]["arguments"] = json!({"a":1,"b":2});
    });
    assert!(verify_manifest(&home, &id, CLEAN, &path).status.success());
    let value: serde_json::Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    let body = serde_json::to_string(&value)
        .unwrap()
        .replace("\"a\":1,\"b\":2", "\"b\":2,\"a\":1");
    std::fs::write(&path, body).unwrap();
    let output = verify_manifest(&home, &id, CLEAN, &path);
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("consecutive_passes=2"));
}

#[test]
fn target_configuration_changes_cannot_reuse_prior_passes() {
    let (home, id) = promoted_home();
    for _ in 0..2 {
        assert!(verify(&home, &id, CLEAN).status.success());
    }
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
            "-u",
            CLEAN,
        ])
        .env("MCPEVAL_HOME", &home)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("consecutive_passes=1"));
}

#[test]
fn verification_identity_does_not_persist_raw_arguments_or_target_paths() {
    let (home, id) = promoted_home();
    let path = changed_manifest(&home, |manifest| {
        manifest["probes"][6]["arguments"] = json!({"secret":"PRIVATE-EVIDENCE-ARGUMENT-81"});
    });
    assert!(verify_manifest(&home, &id, CLEAN, &path).status.success());
    let bytes = std::fs::read(home.join("lifecycle.db")).unwrap();
    let body = String::from_utf8_lossy(&bytes);
    assert!(!body.contains("PRIVATE-EVIDENCE-ARGUMENT-81"));
    assert!(!body.contains(CLEAN));
}

#[test]
fn replaying_a_verification_run_is_idempotent_and_conflicts_fail_closed() {
    let (home, id) = promoted_home();
    assert!(verify(&home, &id, CLEAN).status.success());
    let db = rusqlite::Connection::open(home.join("lifecycle.db")).unwrap();
    let (definition, run): (String, String) = db
        .query_row(
            "SELECT definition_id,run_id FROM probe_history",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    let replay = mcpeval::lifecycle::record(
        &home,
        &id,
        "literal-status",
        &definition,
        &run,
        true,
        Utc::now(),
    )
    .unwrap();
    assert_eq!(replay.consecutive_passes, 1);
    assert!(mcpeval::lifecycle::record(
        &home,
        &id,
        "literal-status",
        &definition,
        &run,
        false,
        Utc::now()
    )
    .is_err());
    let count: i64 = db
        .query_row("SELECT COUNT(*) FROM probe_history", [], |row| row.get(0))
        .unwrap();
    assert_eq!(count, 1);
}

#[test]
fn concurrent_verifications_record_each_result_once() {
    let (home, id) = promoted_home();
    let workers: Vec<_> = (0..3)
        .map(|_| {
            let home = home.clone();
            let id = id.clone();
            std::thread::spawn(move || verify(&home, &id, CLEAN))
        })
        .collect();
    for worker in workers {
        let output = worker.join().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let db = rusqlite::Connection::open(home.join("lifecycle.db")).unwrap();
    let row: (String, i64, i64, i64) = db.query_row(
        "SELECT state,consecutive_passes,(SELECT COUNT(*) FROM probe_history),
         (SELECT COUNT(DISTINCT run_id) FROM probe_history) FROM finding_lifecycle WHERE finding_id=?1",
        [&id], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?))
    ).unwrap();
    assert_eq!(row, ("closed".into(), 3, 3, 3));
}

#[test]
fn modified_index_cache_cannot_supply_verification_credit() {
    let (home, id) = promoted_home();
    assert!(verify(&home, &id, CLEAN).status.success());
    let cache = rusqlite::Connection::open(home.join("index.db")).unwrap();
    cache.execute("UPDATE finding_lifecycle SET state='closed',consecutive_passes=999 WHERE finding_id=?1", [&id]).unwrap();
    let output = verify(&home, &id, CLEAN);
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("consecutive_passes=2"));
}

#[test]
fn a_newer_durable_schema_is_refused_without_overwriting_it() {
    let (home, id) = promoted_home();
    let db = rusqlite::Connection::open(home.join("lifecycle.db")).unwrap();
    db.pragma_update(None, "user_version", 2).unwrap();
    let output = verify(&home, &id, CLEAN);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("schema is newer"));
    let version: i64 = db
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .unwrap();
    assert_eq!(version, 2);
    let count: i64 = db
        .query_row("SELECT COUNT(*) FROM probe_history", [], |row| row.get(0))
        .unwrap();
    assert_eq!(count, 0);
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
        identity: None,
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
