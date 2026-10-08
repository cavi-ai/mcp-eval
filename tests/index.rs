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
        identity: None,
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

fn identified(capture: uuid::Uuid, seq: u64, outcome: &str) -> CallRecord {
    let mut record = rec(seq, outcome);
    record.identity = Some(mcpeval::record::EventIdentity::new(capture));
    record
}

#[test]
fn independent_captures_with_the_same_session_and_sequence_have_isolated_windows() {
    let dir = tempdir();
    let mut store = Store::open(Some(dir.clone())).unwrap();
    let a = uuid::Uuid::new_v4();
    let b = uuid::Uuid::new_v4();
    for record in [
        identified(a, 1, "ok"),
        identified(b, 1, "ok"),
        identified(a, 2, "error"),
        identified(b, 2, "error"),
        identified(a, 3, "ok"),
    ] {
        store.append(&record).unwrap();
    }
    let mut prior = None;
    for _ in 0..2 {
        let stats = index::build(&dir).unwrap();
        assert_eq!(stats.calls, 5);
        let db = rusqlite::Connection::open(dir.join("index.db")).unwrap();
        let cross: i64 = db.query_row("SELECT COUNT(*) FROM windows w JOIN calls f ON f.id=w.failure_id JOIN calls n ON n.id=w.neighbour_id WHERE f.capture_id != n.capture_id", [], |row| row.get(0)).unwrap();
        assert_eq!(cross, 0);
        let links: Vec<(String, String, i64)> = db.prepare("SELECT f.event_id,n.event_id,w.offset FROM windows w JOIN calls f ON f.id=w.failure_id JOIN calls n ON n.id=w.neighbour_id ORDER BY f.event_id,n.event_id").unwrap()
            .query_map([], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?))).unwrap().map(Result::unwrap).collect();
        assert_eq!(links.len(), 3);
        if let Some(previous) = prior {
            assert_eq!(links, previous);
        }
        prior = Some(links);
    }
}

#[test]
fn identical_modern_event_replays_count_once_without_deduplicating_legacy_occurrences() {
    let dir = tempdir();
    let mut store = Store::open(Some(dir.clone())).unwrap();
    let record = identified(uuid::Uuid::new_v4(), 1, "error");
    for _ in 0..2 {
        store.append(&record).unwrap();
        store.append(&rec(1, "ok")).unwrap();
    }
    for _ in 0..2 {
        let stats = index::build(&dir).unwrap();
        assert_eq!(stats.calls, 3);
        assert_eq!(stats.failures, 1);
        assert_eq!(stats.replayed_events, 1);
        let db = rusqlite::Connection::open(dir.join("index.db")).unwrap();
        let legacy: i64 = db
            .query_row(
                "SELECT COUNT(*) FROM calls WHERE capture_id IS NULL AND event_id IS NULL",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(legacy, 2, "no IDs may be manufactured for legacy records");
    }
}

#[test]
fn conflicting_event_identity_preserves_the_prior_index() {
    for conflict in ["event", "sequence", "capture", "partial"] {
        let dir = tempdir();
        let mut store = Store::open(Some(dir.clone())).unwrap();
        let record = identified(uuid::Uuid::new_v4(), 1, "error");
        store.append(&record).unwrap();
        index::build(&dir).unwrap();
        let mut other = record.clone();
        match conflict {
            "event" => other.outcome = "ok".into(),
            "sequence" => other.identity.as_mut().unwrap().event_id = uuid::Uuid::new_v4(),
            "capture" => {
                other.seq = 2;
                other.server = "another-server".into();
                other.identity.as_mut().unwrap().event_id = uuid::Uuid::new_v4();
            }
            "partial" => {
                let mut value = serde_json::to_value(&record).unwrap();
                value["identity"]
                    .as_object_mut()
                    .unwrap()
                    .remove("event_id");
                let path = dir.join("store/calls-2026-08-04.jsonl");
                writeln!(
                    std::fs::OpenOptions::new().append(true).open(path).unwrap(),
                    "{value}"
                )
                .unwrap();
            }
            _ => unreachable!(),
        }
        if conflict != "partial" {
            store.append(&other).unwrap();
        }
        assert!(index::build(&dir).is_err(), "{conflict}");
        let db = rusqlite::Connection::open(dir.join("index.db")).unwrap();
        let rows: Vec<String> = db
            .prepare("SELECT outcome FROM calls")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert_eq!(
            rows,
            vec!["error"],
            "failed rebuild must retain prior evidence"
        );
    }
}

#[test]
fn annotations_resolve_only_explicit_events_or_unambiguous_legacy_coordinates() {
    use mcpeval::record::AnnotationRecord;
    let dir = tempdir();
    let mut store = Store::open(Some(dir.clone())).unwrap();
    let a = identified(uuid::Uuid::new_v4(), 1, "error");
    let b = identified(uuid::Uuid::new_v4(), 1, "error");
    store.append(&a).unwrap();
    store.append(&b).unwrap();
    store.append(&rec(2, "ok")).unwrap();
    for (event_id, session, seq, note) in [
        (
            Some(a.identity.as_ref().unwrap().event_id),
            None,
            None,
            "explicit",
        ),
        (None, Some("s1".into()), Some(1), "ambiguous"),
        (None, Some("s1".into()), Some(2), "legacy"),
        (Some(uuid::Uuid::new_v4()), None, None, "unknown"),
    ] {
        store
            .append_annotation(&AnnotationRecord {
                ts: "2026-08-04T12:00:01Z".into(),
                event_id,
                session,
                seq,
                kind: "false-success".into(),
                note: note.into(),
            })
            .unwrap();
    }
    for _ in 0..2 {
        let stats = index::build(&dir).unwrap();
        assert_eq!(stats.annotations, 4);
        let db = rusqlite::Connection::open(dir.join("index.db")).unwrap();
        let resolved: (Option<String>, String) = db.query_row("SELECT c.event_id,a.note FROM annotations a JOIN calls c ON c.id=a.call_id WHERE a.note='explicit'", [], |row| Ok((row.get(0)?,row.get(1)?))).unwrap();
        assert_eq!(
            resolved.0,
            Some(a.identity.as_ref().unwrap().event_id.to_string())
        );
        let unresolved: i64 = db
            .query_row(
                "SELECT COUNT(*) FROM annotations WHERE call_id IS NULL",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(unresolved, 2);
    }
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
    assert!(
        neighbours.is_empty(),
        "legacy context cannot cross recorder server boundaries"
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
        assert!(
            links.is_empty(),
            "duplicate legacy coordinates cannot establish call order"
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
    let servers: Vec<String> = db
        .prepare("SELECT server FROM calls ORDER BY id")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert_eq!(servers, vec!["earlier", "later"]);
    let windows: i64 = db
        .query_row("SELECT COUNT(*) FROM windows", [], |row| row.get(0))
        .unwrap();
    assert_eq!(windows, 0);
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
        json!({"shape": {"name": "str<32", "retries": "str<8"}})
    );
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&stored.1).unwrap(),
        json!({"object": 2})
    );
    assert_eq!(stored.2, "{message}");
    assert_eq!(stored.3, 0);
}

#[test]
fn index_resanitizes_an_externally_constructed_journal() {
    let dir = tempdir();
    std::fs::create_dir_all(dir.join("store")).unwrap();
    let canary = "CANARY /Users/private?token=secret";
    let record = json!({
        "ts": canary,
        "session": canary,
        "seq": 1,
        "server": canary,
        "method": canary,
        "tool": canary,
        "args": {"path": canary, "header": canary, "count": 42},
        "latency_ms": 5,
        "outcome": canary,
        "error": {
            "code": canary,
            "layer": canary,
            "retryable": false,
            "kind": canary,
            "template": canary,
            "template_id": canary
        },
        "shim_self_us": 10,
        "kind": canary
    });
    std::fs::write(
        dir.join("store/calls-external.jsonl"),
        format!("{}\n", serde_json::to_string(&record).unwrap()),
    )
    .unwrap();

    index::build(&dir).unwrap();
    let db = rusqlite::Connection::open(dir.join("index.db")).unwrap();
    let stored: (
        String,
        String,
        String,
        String,
        Option<String>,
        String,
        String,
        String,
    ) = db
        .query_row(
            "SELECT ts, session, server, method, tool, args, outcome, kind FROM calls",
            [],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                    row.get(7)?,
                ))
            },
        )
        .unwrap();
    let persisted = format!("{stored:?}");
    assert!(!persisted.contains("CANARY"));
    assert!(!persisted.contains("/Users/private"));
    assert!(!persisted.contains("token=secret"));
    assert_eq!(stored.0, "unknown");
    assert!(stored.1.starts_with("session:"));
    assert_eq!(stored.2, "invalid");
    assert_eq!(stored.3, "unparsed/metadata");
    assert_eq!(stored.4, None);
    assert_eq!(stored.6, "unknown");
    assert_eq!(stored.7, "unparsed");
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
        "indexed 1 calls, 1 failures, 0 annotations\n"
    );
    assert!(output.stderr.is_empty());
}

#[test]
fn index_command_prints_the_annotation_count() {
    use mcpeval::record::AnnotationRecord;

    let dir = tempdir();
    let mut store = Store::open(Some(dir.clone())).unwrap();
    store.append(&rec(1, "ok")).unwrap();
    store
        .append_annotation(&AnnotationRecord {
            ts: "2026-08-04T12:00:00Z".into(),
            event_id: None,
            session: Some("s1".into()),
            seq: Some(1),
            kind: "workaround".into(),
            note: "found a way around it".into(),
        })
        .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_mcpeval"))
        .arg("index")
        .env("MCPEVAL_HOME", &dir)
        .output()
        .unwrap();

    assert!(output.status.success());
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        "indexed 1 calls, 0 failures, 1 annotations\n"
    );
}

#[test]
fn distinct_error_fingerprints_stay_distinct_rows() {
    // Build two failures on the same server and tool with different fingerprints,
    // index them, and assert the issue key separates them.
    let dir = tempdir();
    let mut store = Store::open(Some(dir.clone())).unwrap();

    let mut first = rec(1, "error");
    first.tool = Some("click".into());
    first.error = Some(mcpeval::record::ErrorInfo {
        code: Some(serde_json::json!("browserCommandFailed")),
        layer: None,
        retryable: Some(false),
        kind: None,
        template: Some("{message}".into()),
        template_id: Some("aaaaaaaaaaaaaaaa".into()),
    });
    let mut second = rec(2, "error");
    second.tool = Some("click".into());
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
fn same_tool_and_fingerprint_collapse_to_one_issue() {
    // Same server, tool, code, and fingerprint: this is the same issue twice,
    // and must count as a single distinct row.
    let dir = tempdir();
    let mut store = Store::open(Some(dir.clone())).unwrap();

    let mut first = rec(1, "error");
    first.tool = Some("click".into());
    first.error = Some(mcpeval::record::ErrorInfo {
        code: Some(serde_json::json!("browserCommandFailed")),
        layer: None,
        retryable: Some(false),
        kind: None,
        template: Some("{message}".into()),
        template_id: Some("aaaaaaaaaaaaaaaa".into()),
    });
    let mut second = rec(2, "error");
    second.tool = Some("click".into());
    second.error = Some(mcpeval::record::ErrorInfo {
        code: Some(serde_json::json!("browserCommandFailed")),
        layer: None,
        retryable: Some(false),
        kind: None,
        template: Some("{message}".into()),
        template_id: Some("aaaaaaaaaaaaaaaa".into()),
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
    assert_eq!(
        distinct, 1,
        "the same cause twice must collapse into one issue"
    );
}

#[test]
fn windows_follow_sequence_not_file_order() {
    let dir = tempdir();
    let mut store = Store::open(Some(dir.clone())).unwrap();
    // Append the error (seq 2) first, so file order and logical seq order
    // disagree. Under the old session-only sort the file order [2,3,1] is
    // preserved, the failure sits at index 0 with no backward neighbours,
    // and the (wrong) forward-only window yields [3, 1]. Sorting by
    // (session, seq) reorders to [1,2,3], giving the failure one neighbour
    // on each side: [1, 3].
    for seq in [2u64, 3, 1] {
        let outcome = if seq == 2 { "error" } else { "ok" };
        store.append(&rec(seq, outcome)).unwrap();
    }

    index::build(&dir).unwrap();
    let db = rusqlite::Connection::open(dir.join("index.db")).unwrap();
    let neighbours: Vec<i64> = db
        .prepare(
            "SELECT c.seq FROM windows w JOIN calls c ON c.id = w.neighbour_id ORDER BY w.offset",
        )
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert_eq!(
        neighbours,
        vec![1, 3],
        "the failure at seq 2 has seq 1 before it and seq 3 after it"
    );
}

#[test]
fn build_migrates_a_hand_patched_phase_1_index_db() {
    // C2: a Phase 1 `index.db` has a `calls` table with no `err_template_id`
    // column, no `annotations` table, and `calls_issue` keyed on
    // `(server, tool, err_template)`. `CREATE TABLE IF NOT EXISTS` is a
    // no-op against it, so `build` must drop and recreate rather than
    // assume the schema already matches.
    let dir = tempdir();
    let mut store = Store::open(Some(dir.clone())).unwrap();
    store.append(&rec(1, "error")).unwrap();

    {
        let db = rusqlite::Connection::open(dir.join("index.db")).unwrap();
        db.execute_batch(
            "CREATE TABLE calls (
              id INTEGER PRIMARY KEY,
              ts TEXT NOT NULL, session TEXT NOT NULL, seq INTEGER NOT NULL,
              server TEXT NOT NULL, method TEXT NOT NULL, tool TEXT,
              latency_ms INTEGER, outcome TEXT NOT NULL,
              err_code TEXT, err_template TEXT, err_retryable INTEGER,
              args TEXT, kind TEXT NOT NULL
            );
            CREATE TABLE windows (
              failure_id INTEGER NOT NULL REFERENCES calls(id),
              neighbour_id INTEGER NOT NULL REFERENCES calls(id),
              offset INTEGER NOT NULL,
              PRIMARY KEY (failure_id, neighbour_id)
            );
            CREATE INDEX calls_issue ON calls (server, tool, err_template);",
        )
        .unwrap();
    }

    let stats = index::build(&dir).expect("build must migrate a Phase 1 index.db, not fail");
    assert_eq!(stats.calls, 1);
    assert_eq!(stats.failures, 1);

    let db = rusqlite::Connection::open(dir.join("index.db")).unwrap();
    let index_sql: String = db
        .query_row(
            "SELECT sql FROM sqlite_master WHERE type = 'index' AND name = 'calls_issue'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(
        index_sql.contains("err_template_id"),
        "calls_issue must be rebuilt to cover err_template_id, got: {index_sql}"
    );

    let annotation_count: i64 = db
        .query_row("SELECT COUNT(*) FROM annotations", [], |row| row.get(0))
        .unwrap();
    assert_eq!(annotation_count, 0, "annotations table must now exist");
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
#[test]
fn oversized_complete_journal_records_refuse_rebuild_without_replacing_index() {
    let dir = std::env::temp_dir().join(format!("mcpeval-index-bound-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(dir.join("store")).unwrap();
    mcpeval::index::build(&dir).unwrap();
    let mut record = vec![b' '; 4 * 1024 * 1024 + 1];
    record.push(b'\n');
    std::fs::write(dir.join("store/calls-overflow.jsonl"), record).unwrap();
    assert!(mcpeval::index::build(&dir).is_err());
    let db = rusqlite::Connection::open(dir.join("index.db")).unwrap();
    let calls: i64 = db
        .query_row("SELECT COUNT(*) FROM calls", [], |row| row.get(0))
        .unwrap();
    assert_eq!(calls, 0);
}

fn index_contents(root: &std::path::Path) -> Vec<Vec<Vec<String>>> {
    let db = rusqlite::Connection::open(root.join("index.db")).unwrap();
    ["calls", "windows", "annotations"]
        .iter()
        .map(|table| {
            let mut query = db
                .prepare(&format!("SELECT * FROM {table} ORDER BY rowid"))
                .unwrap();
            let columns = query.column_count();
            query
                .query_map([], |row| {
                    Ok((0..columns)
                        .map(|i| format!("{:?}", row.get_ref(i).unwrap()))
                        .collect::<Vec<_>>())
                })
                .unwrap()
                .map(Result::unwrap)
                .collect()
        })
        .collect()
}

fn assert_matches_fresh_index(root: &std::path::Path) {
    let fresh = tempdir();
    std::fs::create_dir_all(fresh.join("store")).unwrap();
    for entry in std::fs::read_dir(root.join("store")).unwrap() {
        let entry = entry.unwrap();
        std::fs::copy(entry.path(), fresh.join("store").join(entry.file_name())).unwrap();
    }
    assert_eq!(index::build(root).unwrap(), index::build(&fresh).unwrap());
    assert_eq!(index_contents(root), index_contents(&fresh));
    std::fs::remove_dir_all(fresh).unwrap();
}

#[test]
fn append_refresh_reuses_cached_rows_and_matches_clean_reconstruction() {
    let dir = tempdir();
    let mut store = Store::open(Some(dir.clone())).unwrap();
    let capture = uuid::Uuid::new_v4();
    let first = identified(capture, 2, "error");
    store.append(&first).unwrap();
    index::build(&dir).unwrap();
    let db = rusqlite::Connection::open(dir.join("index.db")).unwrap();
    db.execute_batch("CREATE TRIGGER retained_calls BEFORE INSERT ON calls WHEN NEW.seq=2 BEGIN SELECT RAISE(ABORT,'historical row reinserted'); END;").unwrap();
    for seq in [1, 3, 4] {
        store.append(&identified(capture, seq, "ok")).unwrap();
    }
    assert_matches_fresh_index(&dir);
    assert_eq!(
        db.query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='trigger' AND name='retained_calls'",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        1
    );
    store.append(&first).unwrap();
    assert_eq!(index::build(&dir).unwrap().replayed_events, 1);
    assert_matches_fresh_index(&dir);
}

#[test]
fn refreshed_calls_relink_annotations_and_remove_ambiguous_legacy_windows() {
    use mcpeval::record::AnnotationRecord;
    let dir = tempdir();
    let mut store = Store::open(Some(dir.clone())).unwrap();
    store.append(&rec(1, "error")).unwrap();
    store
        .append_annotation(&AnnotationRecord {
            ts: "2026-08-04T12:00:01Z".into(),
            event_id: None,
            session: Some("s1".into()),
            seq: Some(1),
            kind: "false-success".into(),
            note: "legacy".into(),
        })
        .unwrap();
    index::build(&dir).unwrap();
    store.append(&rec(2, "ok")).unwrap();
    assert_matches_fresh_index(&dir);
    store.append(&rec(1, "ok")).unwrap();
    assert_matches_fresh_index(&dir);
    let db = rusqlite::Connection::open(dir.join("index.db")).unwrap();
    assert_eq!(
        db.query_row(
            "SELECT COUNT(*) FROM annotations WHERE call_id IS NULL",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        1
    );
    assert_eq!(
        db.query_row("SELECT COUNT(*) FROM windows", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        0
    );
}

#[test]
fn edited_removed_replaced_and_older_journals_match_clean_reconstruction() {
    for mutation in [
        "edit",
        "truncate",
        "remove",
        "replace",
        "older",
        "append-older",
    ] {
        let dir = tempdir();
        let mut store = Store::open(Some(dir.clone())).unwrap();
        store.append(&rec(1, "error")).unwrap();
        let path = dir.join("store/calls-2026-08-04.jsonl");
        index::build(&dir).unwrap();
        match mutation {
            "edit" => {
                let bytes = std::fs::read_to_string(&path).unwrap();
                std::fs::write(&path, bytes.replace("error", "other")).unwrap();
            }
            "truncate" => {
                std::fs::write(&path, b"").unwrap();
            }
            "remove" => {
                std::fs::remove_file(&path).unwrap();
            }
            "replace" => {
                std::fs::remove_file(&path).unwrap();
                store.append(&rec(2, "ok")).unwrap();
            }
            "older" => {
                store
                    .append(&rec_for("s1", "demo", 2, "ok", "2026-08-03T12:00:00Z"))
                    .unwrap();
            }
            "append-older" => {
                store
                    .append(&rec_for("s1", "demo", 3, "ok", "2026-08-05T12:00:00Z"))
                    .unwrap();
                index::build(&dir).unwrap();
                store.append(&rec(2, "ok")).unwrap();
            }
            _ => unreachable!(),
        }
        assert_matches_fresh_index(&dir);
    }
}

#[test]
fn interrupted_appends_and_failed_refreshes_preserve_checkpoint_evidence() {
    let dir = tempdir();
    let mut store = Store::open(Some(dir.clone())).unwrap();
    let first = identified(uuid::Uuid::new_v4(), 1, "error");
    store.append(&first).unwrap();
    index::build(&dir).unwrap();
    let path = dir.join("store/calls-2026-08-04.jsonl");
    let before = std::fs::read(&path).unwrap();
    let next = serde_json::to_vec(&identified(
        first.identity.as_ref().unwrap().capture_id,
        2,
        "ok",
    ))
    .unwrap();
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap();
    file.write_all(&next[..next.len() / 2]).unwrap();
    assert_matches_fresh_index(&dir);
    file.write_all(&next[next.len() / 2..]).unwrap();
    file.write_all(b"\n").unwrap();
    assert_matches_fresh_index(&dir);
    let valid = std::fs::read(&path).unwrap();
    let snapshot = index_contents(&dir);
    file.write_all(b"{bad}\n").unwrap();
    assert!(index::build(&dir).is_err());
    assert_eq!(index_contents(&dir), snapshot);
    drop(file);
    std::fs::write(&path, valid).unwrap();
    assert_matches_fresh_index(&dir);
    let mut conflicting = first;
    conflicting.outcome = "ok".into();
    store.append(&conflicting).unwrap();
    assert!(index::build(&dir).is_err());
    assert_eq!(index_contents(&dir), snapshot);
    std::fs::write(&path, before).unwrap();
    assert_matches_fresh_index(&dir);
}

#[test]
fn index_rebuild_command_discards_the_cache_without_touching_journals() {
    let dir = tempdir();
    let mut store = Store::open(Some(dir.clone())).unwrap();
    store.append(&rec(1, "error")).unwrap();
    index::build(&dir).unwrap();
    let path = dir.join("store/calls-2026-08-04.jsonl");
    let original = std::fs::read(&path).unwrap();
    let db = rusqlite::Connection::open(dir.join("index.db")).unwrap();
    db.execute("UPDATE calls SET outcome='ok'", []).unwrap();
    let result = Command::new(env!("CARGO_BIN_EXE_mcpeval"))
        .env("MCPEVAL_HOME", &dir)
        .args(["index", "--rebuild"])
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(
        db.query_row("SELECT outcome FROM calls", [], |row| row
            .get::<_, String>(0))
            .unwrap(),
        "error"
    );
    assert_eq!(std::fs::read(path).unwrap(), original);
}

#[test]
fn trailing_journals_and_late_events_refresh_retained_annotations() {
    use mcpeval::record::AnnotationRecord;
    let dir = tempdir();
    let mut store = Store::open(Some(dir.clone())).unwrap();
    let event = identified(uuid::Uuid::new_v4(), 1, "error");
    store
        .append_annotation(&AnnotationRecord {
            ts: "2026-08-04T12:00:01Z".into(),
            event_id: Some(event.identity.as_ref().unwrap().event_id),
            session: None,
            seq: None,
            kind: "false-success".into(),
            note: "late".into(),
        })
        .unwrap();
    index::build(&dir).unwrap();
    store.append(&event).unwrap();
    assert_matches_fresh_index(&dir);
    let db = rusqlite::Connection::open(dir.join("index.db")).unwrap();
    assert_eq!(
        db.query_row(
            "SELECT COUNT(*) FROM annotations WHERE call_id IS NOT NULL",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        1
    );
    store
        .append(&rec_for("s1", "demo", 2, "ok", "2026-08-05T12:00:00Z"))
        .unwrap();
    assert_matches_fresh_index(&dir);
    store
        .append_annotation(&AnnotationRecord {
            ts: "2026-08-03T12:00:01Z".into(),
            event_id: Some(event.identity.as_ref().unwrap().event_id),
            session: None,
            seq: None,
            kind: "false-success".into(),
            note: "older".into(),
        })
        .unwrap();
    assert_matches_fresh_index(&dir);
}

#[test]
fn changed_cached_schema_is_reconstructed_and_concurrent_refreshes_do_not_duplicate_calls() {
    let dir = tempdir();
    let mut store = Store::open(Some(dir.clone())).unwrap();
    store.append(&rec(1, "error")).unwrap();
    index::build(&dir).unwrap();
    let db = rusqlite::Connection::open(dir.join("index.db")).unwrap();
    db.execute_batch("DROP INDEX calls_issue").unwrap();
    assert_matches_fresh_index(&dir);
    assert_eq!(
        db.query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='index' AND name='calls_issue'",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        1
    );
    store.append(&rec(2, "ok")).unwrap();
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
    let workers: Vec<_> = (0..2)
        .map(|_| {
            let barrier = barrier.clone();
            let dir = dir.clone();
            std::thread::spawn(move || {
                barrier.wait();
                index::build(&dir).unwrap()
            })
        })
        .collect();
    for worker in workers {
        assert_eq!(worker.join().unwrap().calls, 2);
    }
    assert_matches_fresh_index(&dir);
}
