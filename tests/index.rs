use std::io::Write;
use std::process::Command;

use mcpeval::index;
use mcpeval::record::{CallRecord, ErrorInfo};
use mcpeval::store::Store;
use rusqlite::params;
use serde_json::json;

fn rec(seq: u64, outcome: &str) -> CallRecord {
    rec_for("s1", "demo", seq, outcome, "2026-08-04T12:00:00Z")
}

fn rec_for(session: &str, server: &str, seq: u64, outcome: &str, ts: &str) -> CallRecord {
    CallRecord {
        ts: ts.into(),
        session: session.into(),
        seq,
        server: server.into(),
        method: "tools/call".into(),
        tool: Some(format!("tool{seq}")),
        args: None,
        latency_ms: Some(5),
        outcome: outcome.into(),
        error: None,
        shim_self_us: 10,
        kind: "real".into(),
    }
}

fn tempdir() -> std::path::PathBuf {
    let base = std::env::temp_dir().join(format!("mcpeval-index-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&base).unwrap();
    base
}

#[test]
fn loads_records_and_derives_a_failure_window() {
    let dir = tempdir();
    let mut store = Store::open(Some(dir.clone())).unwrap();
    for seq in 1..=10 {
        let outcome = if seq == 7 { "error" } else { "ok" };
        store.append(&rec(seq, outcome)).unwrap();
    }

    let stats = index::build(&dir).unwrap();
    assert_eq!(stats.calls, 10);
    assert_eq!(stats.failures, 1);

    let db = rusqlite::Connection::open(dir.join("index.db")).unwrap();
    let offsets: Vec<i64> = db
        .prepare("SELECT offset FROM windows ORDER BY offset")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert_eq!(offsets, vec![-5, -4, -3, -2, -1, 1, 2, 3]);
}

#[test]
fn a_failure_near_the_start_gets_a_short_window() {
    let dir = tempdir();
    let mut store = Store::open(Some(dir.clone())).unwrap();
    store.append(&rec(1, "error")).unwrap();
    store.append(&rec(2, "ok")).unwrap();

    index::build(&dir).unwrap();
    let db = rusqlite::Connection::open(dir.join("index.db")).unwrap();
    let count: i64 = db
        .query_row("SELECT COUNT(*) FROM windows", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 1, "only the one following call exists");
}

#[test]
fn rebuilding_is_idempotent_and_removes_calls_no_longer_in_the_journal() {
    let dir = tempdir();
    let mut store = Store::open(Some(dir.clone())).unwrap();
    store.append(&rec(1, "ok")).unwrap();
    store.append(&rec(2, "ok")).unwrap();

    index::build(&dir).unwrap();
    let journal = dir.join("store/calls-2026-08-04.jsonl");
    let first_line = std::fs::read_to_string(&journal)
        .unwrap()
        .lines()
        .next()
        .unwrap()
        .to_owned();
    std::fs::write(&journal, format!("{first_line}\n")).unwrap();

    let stats = index::build(&dir).unwrap();
    assert_eq!(stats.calls, 1);

    let db = rusqlite::Connection::open(dir.join("index.db")).unwrap();
    let count: i64 = db
        .query_row("SELECT COUNT(*) FROM calls", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 1, "rebuild must exactly replace prior rows");
}

#[test]
fn shared_session_sequence_values_from_different_servers_remain_distinct() {
    let dir = tempdir();
    let mut store = Store::open(Some(dir.clone())).unwrap();
    store
        .append(&rec_for(
            "shared",
            "alpha",
            1,
            "ok",
            "2026-08-04T12:00:00.001Z",
        ))
        .unwrap();
    store
        .append(&rec_for(
            "shared",
            "beta",
            1,
            "error",
            "2026-08-04T12:00:00.002Z",
        ))
        .unwrap();
    store
        .append(&rec_for(
            "shared",
            "alpha",
            2,
            "ok",
            "2026-08-04T12:00:00.003Z",
        ))
        .unwrap();

    let stats = index::build(&dir).unwrap();
    assert_eq!(stats.calls, 3);
    assert_eq!(stats.failures, 1);

    let db = rusqlite::Connection::open(dir.join("index.db")).unwrap();
    let neighbours: Vec<(String, i64, i64)> = db
        .prepare(
            "SELECT calls.server, calls.seq, windows.offset
             FROM windows JOIN calls ON calls.id = windows.neighbour_id
             ORDER BY windows.offset",
        )
        .unwrap()
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert_eq!(
        neighbours,
        vec![("alpha".into(), 1, -1), ("alpha".into(), 2, 1)]
    );
}

#[test]
fn duplicate_session_server_sequence_occurrences_are_preserved_on_every_rebuild() {
    let dir = tempdir();
    let mut store = Store::open(Some(dir.clone())).unwrap();
    store.append(&rec(1, "ok")).unwrap();
    store.append(&rec(1, "error")).unwrap();

    for _ in 0..2 {
        let stats = index::build(&dir).unwrap();
        assert_eq!(stats.calls, 2);
        assert_eq!(stats.failures, 1);

        let db = rusqlite::Connection::open(dir.join("index.db")).unwrap();
        let outcomes: Vec<String> = db
            .prepare("SELECT outcome FROM calls ORDER BY id")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert_eq!(outcomes, vec!["ok", "error"]);
        let links: Vec<(i64, i64, String, String)> = db
            .prepare("SELECT w.failure_id, w.neighbour_id, f.outcome, n.outcome FROM windows w JOIN calls f ON f.id=w.failure_id JOIN calls n ON n.id=w.neighbour_id")
            .unwrap()
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)))
            .unwrap().map(Result::unwrap).collect();
        assert_eq!(links.len(), 1);
        assert_ne!(links[0].0, links[0].1);
        assert_eq!(
            (&links[0].2, &links[0].3),
            (&"error".to_string(), &"ok".to_string())
        );
    }
}

#[test]
fn journal_files_are_processed_in_deterministic_name_order() {
    let dir = tempdir();
    let mut store = Store::open(Some(dir.clone())).unwrap();
    store
        .append(&rec_for(
            "shared",
            "later",
            1,
            "error",
            "2026-08-05T00:00:00Z",
        ))
        .unwrap();
    store
        .append(&rec_for(
            "shared",
            "earlier",
            1,
            "ok",
            "2026-08-04T23:59:59Z",
        ))
        .unwrap();

    index::build(&dir).unwrap();

    let db = rusqlite::Connection::open(dir.join("index.db")).unwrap();
    let neighbour: (String, i64) = db
        .query_row(
            "SELECT calls.server, windows.offset
             FROM windows JOIN calls ON calls.id = windows.neighbour_id",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(neighbour, ("earlier".into(), -1));
}

#[test]
fn ignores_an_unterminated_trailing_record_from_an_active_writer() {
    let dir = tempdir();
    let mut store = Store::open(Some(dir.clone())).unwrap();
    store.append(&rec(1, "ok")).unwrap();
    let journal = dir.join("store/calls-2026-08-04.jsonl");
    std::fs::OpenOptions::new()
        .append(true)
        .open(journal)
        .unwrap()
        .write_all(br#"{"ts":"partial""#)
        .unwrap();

    let stats = index::build(&dir).unwrap();
    assert_eq!(stats.calls, 1);
}

#[test]
fn malformed_complete_record_preserves_the_previous_index() {
    let dir = tempdir();
    let mut store = Store::open(Some(dir.clone())).unwrap();
    store.append(&rec(1, "ok")).unwrap();
    index::build(&dir).unwrap();

    let journal = dir.join("store/calls-2026-08-04.jsonl");
    std::fs::OpenOptions::new()
        .append(true)
        .open(journal)
        .unwrap()
        .write_all(b"not-json\n")
        .unwrap();

    assert!(index::build(&dir).is_err());
    let db = rusqlite::Connection::open(dir.join("index.db")).unwrap();
    let count: i64 = db
        .query_row("SELECT COUNT(*) FROM calls", [], |row| row.get(0))
        .unwrap();
    assert_eq!(count, 1, "a failed rebuild must retain the prior index");
}

#[test]
fn ignores_non_call_files_and_directories() {
    let dir = tempdir();
    let mut store = Store::open(Some(dir.clone())).unwrap();
    store.append(&rec(1, "ok")).unwrap();
    std::fs::write(dir.join("store/notes.jsonl"), b"not-json\n").unwrap();
    std::fs::create_dir(dir.join("store/calls-directory.jsonl")).unwrap();

    let stats = index::build(&dir).unwrap();
    assert_eq!(stats.calls, 1);
}

#[cfg(unix)]
#[test]
fn ignores_symlinked_call_files() {
    let dir = tempdir();
    let mut store = Store::open(Some(dir.clone())).unwrap();
    store.append(&rec(1, "ok")).unwrap();
    let outside = dir.join("outside.jsonl");
    std::fs::write(&outside, b"not-json\n").unwrap();
    std::os::unix::fs::symlink(&outside, dir.join("store/calls-linked.jsonl")).unwrap();

    let stats = index::build(&dir).unwrap();
    assert_eq!(stats.calls, 1);
}

#[test]
fn preserves_privacy_safe_serialized_values() {
    let dir = tempdir();
    let mut store = Store::open(Some(dir.clone())).unwrap();
    let mut record = rec(1, "error");
    record.args = Some(json!({"shape": {"name": "str<32", "retries": "int"}}));
    record.error = Some(ErrorInfo {
        code: Some(json!({"object": 2})),
        layer: Some("str<8".into()),
        retryable: Some(false),
        kind: Some("str<32".into()),
        template: Some("{message}".into()),
        template_id: None,
    });
    store.append(&record).unwrap();

    index::build(&dir).unwrap();

    let db = rusqlite::Connection::open(dir.join("index.db")).unwrap();
    let stored: (String, String, String, i64) = db
        .query_row(
            "SELECT args, err_code, err_template, err_retryable FROM calls",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .unwrap();
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&stored.0).unwrap(),
        json!({"shape": {"name": "str<32", "retries": "int"}})
    );
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&stored.1).unwrap(),
        json!({"object": 2})
    );
    assert_eq!(stored.2, "{message}");
    assert_eq!(stored.3, 0);
}

#[test]
fn index_command_prints_the_indexed_counts() {
    let dir = tempdir();
    let mut store = Store::open(Some(dir.clone())).unwrap();
    store.append(&rec(1, "error")).unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_mcpeval"))
        .arg("index")
        .env("MCPEVAL_HOME", &dir)
        .output()
        .unwrap();

    assert!(output.status.success());
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        "indexed 1 calls, 1 failures\n"
    );
    assert!(output.stderr.is_empty());
}

#[test]
fn distinct_error_fingerprints_stay_distinct_rows() {
    // Build two failures on the same server and tool with different fingerprints,
    // index them, and assert the issue key separates them.
    let dir = tempdir();
    let mut store = Store::open(Some(dir.clone())).unwrap();

    let mut first = rec(1, "error");
    first.error = Some(mcpeval::record::ErrorInfo {
        code: Some(serde_json::json!("browserCommandFailed")),
        layer: None,
        retryable: Some(false),
        kind: None,
        template: Some("{message}".into()),
        template_id: Some("aaaaaaaaaaaaaaaa".into()),
    });
    let mut second = rec(2, "error");
    second.error = Some(mcpeval::record::ErrorInfo {
        code: Some(serde_json::json!("browserCommandFailed")),
        layer: None,
        retryable: Some(false),
        kind: None,
        template: Some("{message}".into()),
        template_id: Some("bbbbbbbbbbbbbbbb".into()),
    });
    store.append(&first).unwrap();
    store.append(&second).unwrap();

    index::build(&dir).unwrap();
    let db = rusqlite::Connection::open(dir.join("index.db")).unwrap();
    let distinct: i64 = db
        .query_row(
            "SELECT COUNT(*) FROM (SELECT DISTINCT server, tool, err_code, err_template_id FROM calls WHERE outcome = 'error')",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(distinct, 2, "two causes must not collapse into one issue");
}

#[test]
fn windows_never_cross_session_boundaries() {
    let dir = tempdir();
    let mut store = Store::open(Some(dir.clone())).unwrap();
    store
        .append(&rec_for("first", "demo", 1, "ok", "2026-08-04T12:00:00Z"))
        .unwrap();
    store
        .append(&rec_for(
            "second",
            "demo",
            1,
            "error",
            "2026-08-04T12:00:01Z",
        ))
        .unwrap();

    index::build(&dir).unwrap();
    let db = rusqlite::Connection::open(dir.join("index.db")).unwrap();
    let count: i64 = db
        .query_row("SELECT COUNT(*) FROM windows", params![], |row| row.get(0))
        .unwrap();
    assert_eq!(count, 0);
}
