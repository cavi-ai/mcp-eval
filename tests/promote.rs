use chrono::{TimeZone, Utc};

use mcpeval::index;
use mcpeval::promote::{
    calibrate_seed, promote, resolve_threshold, score, wilson_lower_bound, PromotionConfig,
    ScoreInput,
};
use mcpeval::record::{AnnotationRecord, CallRecord, ErrorInfo};
use mcpeval::store::Store;
use serde_json::json;

fn close(actual: f64, expected: f64) {
    assert!((actual - expected).abs() < 1e-9, "{actual} != {expected}");
}

#[test]
fn scoring_wilson_uses_the_95_percent_lower_bound() {
    close(wilson_lower_bound(2, 2).unwrap(), 0.342_380_227_506_653_1);
    close(
        wilson_lower_bound(40, 100).unwrap(),
        0.309_401_286_432_458_9,
    );
    assert!(wilson_lower_bound(3, 3).unwrap() > wilson_lower_bound(2, 3).unwrap());
}

#[test]
fn scoring_rejects_impossible_counts() {
    assert!(wilson_lower_bound(1, 0).is_err());
    assert!(wilson_lower_bound(3, 2).is_err());
}

#[test]
fn scoring_recency_halves_after_fourteen_days_and_clamps_future_time() {
    let now = Utc.with_ymd_and_hms(2026, 8, 5, 0, 0, 0).unwrap();
    let recent = score(ScoreInput {
        failures: 4,
        calls: 5,
        last_seen: now,
        now,
        cost: 3.0,
        blast: 2,
    })
    .unwrap();
    let old = score(ScoreInput {
        last_seen: now - chrono::Duration::days(14),
        ..ScoreInput {
            failures: 4,
            calls: 5,
            last_seen: now,
            now,
            cost: 3.0,
            blast: 2,
        }
    })
    .unwrap();
    let future = score(ScoreInput {
        last_seen: now + chrono::Duration::days(1),
        ..ScoreInput {
            failures: 4,
            calls: 5,
            last_seen: now,
            now,
            cost: 3.0,
            blast: 2,
        }
    })
    .unwrap();

    close(old.recency, 0.5);
    close(old.score, recent.score / 2.0);
    close(future.recency, 1.0);
}

#[test]
fn scoring_is_monotonic_in_cost_and_blast_radius() {
    let now = Utc.with_ymd_and_hms(2026, 8, 5, 0, 0, 0).unwrap();
    let base = ScoreInput {
        failures: 3,
        calls: 5,
        last_seen: now,
        now,
        cost: 1.0,
        blast: 1,
    };
    let base_score = score(base).unwrap().score;
    assert!(score(ScoreInput { cost: 4.0, ..base }).unwrap().score > base_score);
    assert!(score(ScoreInput { blast: 3, ..base }).unwrap().score > base_score);
}

#[test]
fn scoring_seed_calibration_separates_blockers_from_annoyances() {
    let threshold = calibrate_seed().unwrap();
    assert!(threshold.is_finite() && threshold > 0.0);
    let empty_home = tempdir();
    close(resolve_threshold(&empty_home, None).unwrap(), threshold);

    let rows: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/phase2-seed.json")).unwrap();
    assert_eq!(
        rows.as_array()
            .unwrap()
            .iter()
            .map(|row| row["observations"].as_u64().unwrap())
            .sum::<u64>(),
        17
    );
    let now = Utc.with_ymd_and_hms(2026, 8, 5, 0, 0, 0).unwrap();
    for row in rows.as_array().unwrap() {
        let parts = score(ScoreInput {
            failures: row["failures"].as_u64().unwrap(),
            calls: row["calls"].as_u64().unwrap(),
            last_seen: now
                - chrono::Duration::seconds((row["age_days"].as_f64().unwrap() * 86_400.0) as i64),
            now,
            cost: row["cost"].as_f64().unwrap(),
            blast: row["blast"].as_u64().unwrap(),
        })
        .unwrap();
        match row["class"].as_str().unwrap() {
            "blocker" => assert!(parts.score >= threshold, "{}", row["name"]),
            "annoyance" => assert!(parts.score < threshold, "{}", row["name"]),
            other => panic!("unknown seed class {other}"),
        }
    }
}

fn tempdir() -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!("mcpeval-promote-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&path).unwrap();
    path
}

fn call(session: &str, seq: u64, tool: &str, outcome: &str, template_id: &str) -> CallRecord {
    CallRecord {
        ts: format!("2026-08-04T12:00:{seq:02}Z"),
        session: session.into(),
        seq,
        server: "demo".into(),
        method: "tools/call".into(),
        tool: Some(tool.into()),
        args: Some(json!({"shape": {"target": "str<32"}})),
        latency_ms: Some(5),
        outcome: outcome.into(),
        error: (outcome == "error").then(|| ErrorInfo {
            code: Some(json!("blocked")),
            layer: None,
            retryable: Some(false),
            kind: None,
            template: Some("private raw template".into()),
            template_id: Some(template_id.into()),
        }),
        shim_self_us: 1,
        kind: "real".into(),
    }
}

#[test]
fn aggregation_groups_complete_issue_keys_and_uses_server_tool_call_denominator() {
    let dir = tempdir();
    let mut store = Store::open(Some(dir.clone())).unwrap();
    for record in [
        call("s1", 1, "click", "error", "aaaaaaaaaaaaaaaa"),
        call("s1", 2, "click", "ok", "aaaaaaaaaaaaaaaa"),
        call("s2", 1, "click", "error", "aaaaaaaaaaaaaaaa"),
        call("s2", 2, "click", "error", "bbbbbbbbbbbbbbbb"),
    ] {
        store.append(&record).unwrap();
    }
    index::build(&dir).unwrap();

    let stats = promote(
        &dir,
        PromotionConfig {
            threshold: 0.0,
            now: Utc.with_ymd_and_hms(2026, 8, 5, 0, 0, 0).unwrap(),
        },
    )
    .unwrap();
    assert_eq!((stats.issues, stats.findings), (2, 1));

    let db = rusqlite::Connection::open(dir.join("index.db")).unwrap();
    let aggregate: (i64, i64, i64) = db
        .query_row(
            "SELECT failures, calls, sessions FROM issues WHERE err_template_id='aaaaaaaaaaaaaaaa'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(aggregate, (2, 4, 2));
}

#[test]
fn aggregation_uses_median_real_window_turns_and_distinct_tool_blast_radius() {
    let dir = tempdir();
    let mut store = Store::open(Some(dir.clone())).unwrap();
    for record in [
        call("s1", 1, "open", "ok", "aaaaaaaaaaaaaaaa"),
        call("s1", 2, "click", "error", "aaaaaaaaaaaaaaaa"),
        call("s1", 3, "type", "ok", "aaaaaaaaaaaaaaaa"),
        call("s2", 1, "open", "ok", "aaaaaaaaaaaaaaaa"),
        call("s2", 2, "click", "error", "aaaaaaaaaaaaaaaa"),
        call("s2", 3, "type", "ok", "aaaaaaaaaaaaaaaa"),
        call("s2", 4, "wait", "ok", "aaaaaaaaaaaaaaaa"),
    ] {
        store.append(&record).unwrap();
    }
    index::build(&dir).unwrap();
    promote(
        &dir,
        PromotionConfig {
            threshold: 0.0,
            now: Utc.with_ymd_and_hms(2026, 8, 5, 0, 0, 0).unwrap(),
        },
    )
    .unwrap();

    let db = rusqlite::Connection::open(dir.join("index.db")).unwrap();
    let aggregate: (f64, i64) = db
        .query_row("SELECT cost, blast FROM issues", [], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })
        .unwrap();
    assert_eq!(aggregate, (3.5, 4));
}

#[test]
fn two_session_rule_blocks_even_a_high_scoring_issue_at_zero_threshold() {
    let dir = tempdir();
    let mut store = Store::open(Some(dir.clone())).unwrap();
    for seq in 1..=8 {
        store
            .append(&call(
                "one-afternoon",
                seq,
                "click",
                "error",
                "aaaaaaaaaaaaaaaa",
            ))
            .unwrap();
    }
    index::build(&dir).unwrap();
    let stats = promote(
        &dir,
        PromotionConfig {
            threshold: 0.0,
            now: Utc.with_ymd_and_hms(2026, 8, 5, 0, 0, 0).unwrap(),
        },
    )
    .unwrap();

    assert_eq!(stats.issues, 1);
    assert_eq!(stats.findings, 0);
}

#[test]
fn aggregation_orders_rfc3339_timestamps_by_instant_and_rejects_any_invalid_value() {
    let dir = tempdir();
    let mut store = Store::open(Some(dir.clone())).unwrap();
    let mut earlier = call("s1", 1, "click", "error", "aaaaaaaaaaaaaaaa");
    earlier.ts = "2026-08-05T01:00:00+02:00".into();
    let mut later = call("s2", 1, "click", "error", "aaaaaaaaaaaaaaaa");
    later.ts = "2026-08-05T00:30:00Z".into();
    store.append(&earlier).unwrap();
    store.append(&later).unwrap();
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
    let last_seen: String = db
        .query_row("SELECT last_seen FROM issues", [], |row| row.get(0))
        .unwrap();
    assert_eq!(last_seen, "2026-08-05T00:30:00Z");

    db.execute(
        "UPDATE calls SET ts='unknown' WHERE id=(SELECT MIN(id) FROM calls)",
        [],
    )
    .unwrap();
    assert!(promote(
        &dir,
        PromotionConfig {
            threshold: 0.0,
            now: Utc.with_ymd_and_hms(2026, 8, 5, 1, 0, 0).unwrap(),
        }
    )
    .is_err());
}

fn at(hour: u32) -> PromotionConfig {
    PromotionConfig {
        threshold: 0.0,
        now: Utc.with_ymd_and_hms(2026, 8, 5, hour, 0, 0).unwrap(),
    }
}

fn coded(
    session: &str,
    seq: u64,
    tool: &str,
    code: i64,
    retryable: bool,
    template_id: &str,
) -> CallRecord {
    let mut record = call(session, seq, tool, "error", template_id);
    let error = record.error.as_mut().unwrap();
    error.code = Some(json!(code));
    error.retryable = Some(retryable);
    record
}

#[test]
fn promotion_counts_only_real_calls_and_failures() {
    let dir = tempdir();
    let mut store = Store::open(Some(dir.clone())).unwrap();
    for session in ["probe-one", "probe-two"] {
        for seq in 1..=3 {
            let mut record = call(session, seq, "click", "error", "aaaaaaaaaaaaaaaa");
            record.kind = "synthetic".into();
            store.append(&record).unwrap();
        }
    }
    index::build(&dir).unwrap();
    let stats = promote(&dir, at(0)).unwrap();
    assert_eq!((stats.issues, stats.findings), (0, 0));

    for record in [
        call("s1", 1, "click", "error", "aaaaaaaaaaaaaaaa"),
        call("s1", 2, "click", "ok", "aaaaaaaaaaaaaaaa"),
        call("s2", 1, "click", "error", "aaaaaaaaaaaaaaaa"),
    ] {
        store.append(&record).unwrap();
    }
    index::build(&dir).unwrap();
    let stats = promote(&dir, at(0)).unwrap();
    assert_eq!((stats.issues, stats.findings), (1, 1));
    let db = rusqlite::Connection::open(dir.join("index.db")).unwrap();
    let counts: (i64, i64, i64) = db
        .query_row("SELECT failures, calls, sessions FROM issues", [], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })
        .unwrap();
    assert_eq!(counts, (2, 3, 2));
}

#[test]
fn one_template_with_several_codes_is_one_issue_keyed_without_the_code() {
    let dir = tempdir();
    let mut store = Store::open(Some(dir.clone())).unwrap();
    for record in [
        coded("s1", 1, "click", -32002, false, "aaaaaaaaaaaaaaaa"),
        coded("s2", 1, "click", -32001, false, "aaaaaaaaaaaaaaaa"),
        coded("s2", 2, "click", -32002, false, "aaaaaaaaaaaaaaaa"),
        coded("s1", 2, "type", -32005, false, "bbbbbbbbbbbbbbbb"),
        coded("s2", 3, "type", -32003, false, "bbbbbbbbbbbbbbbb"),
    ] {
        store.append(&record).unwrap();
    }
    index::build(&dir).unwrap();
    let stats = promote(&dir, at(0)).unwrap();
    assert_eq!((stats.issues, stats.findings), (2, 2));

    let db = rusqlite::Connection::open(dir.join("index.db")).unwrap();
    let rows: Vec<(String, String, String, String, i64)> = db
        .prepare(
            "SELECT finding_id, err_code, err_codes, class, failures FROM issues
             ORDER BY err_template_id",
        )
        .unwrap()
        .query_map([], |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
            ))
        })
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(
        rows,
        vec![
            (
                mcpeval::lifecycle::finding_id(
                    "demo",
                    Some("click"),
                    None,
                    Some("aaaaaaaaaaaaaaaa")
                ),
                "-32002".into(),
                "[-32001,-32002]".into(),
                "unstable-error-code".into(),
                3,
            ),
            (
                mcpeval::lifecycle::finding_id(
                    "demo",
                    Some("type"),
                    None,
                    Some("bbbbbbbbbbbbbbbb")
                ),
                "-32003".into(),
                "[-32003,-32005]".into(),
                "unstable-error-code".into(),
                2,
            ),
        ]
    );
}

#[test]
fn rekeyed_findings_keep_the_latest_lifecycle_state_and_its_probe_history() {
    let dir = tempdir();
    let mut store = Store::open(Some(dir.clone())).unwrap();
    for record in [
        coded("s1", 1, "click", -32001, false, "aaaaaaaaaaaaaaaa"),
        coded("s2", 1, "click", -32001, false, "aaaaaaaaaaaaaaaa"),
    ] {
        store.append(&record).unwrap();
    }
    index::build(&dir).unwrap();
    let template = Some("aaaaaaaaaaaaaaaa");
    let kept = mcpeval::lifecycle::finding_id("demo", Some("click"), Some("-32001"), template);
    let stale = mcpeval::lifecycle::finding_id("demo", Some("click"), Some("-32002"), template);
    let rekeyed = mcpeval::lifecycle::finding_id("demo", Some("click"), None, template);
    let db = rusqlite::Connection::open(dir.join("index.db")).unwrap();
    db.execute_batch(mcpeval::lifecycle::SCHEMA).unwrap();
    for (id, code, probe, state, passes, updated_at) in [
        (
            &kept,
            "-32001",
            "probe-kept",
            "verifying",
            2,
            "2026-08-04T12:00:00.000Z",
        ),
        (
            &stale,
            "-32002",
            "probe-stale",
            "open",
            0,
            "2026-08-01T00:00:00.000Z",
        ),
    ] {
        db.execute(
            "INSERT INTO finding_lifecycle
             (finding_id,server,tool,err_code,err_template_id,probe_id,state,consecutive_passes,updated_at)
             VALUES (?1,'demo','click',?2,'aaaaaaaaaaaaaaaa',?3,?4,?5,?6)",
            rusqlite::params![id, code, probe, state, passes, updated_at],
        )
        .unwrap();
        db.execute(
            "INSERT INTO probe_history(finding_id,probe_id,passed,ts) VALUES (?1,?2,1,?3)",
            rusqlite::params![id, probe, updated_at],
        )
        .unwrap();
    }

    let stats = promote(&dir, at(0)).unwrap();
    assert_eq!(stats.findings, 1);
    let lifecycle: Vec<(String, String, Option<String>, i64)> = db
        .prepare("SELECT finding_id,state,probe_id,consecutive_passes FROM finding_lifecycle")
        .unwrap()
        .query_map([], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
        })
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(
        lifecycle,
        vec![(
            rekeyed.clone(),
            "verifying".into(),
            Some("probe-kept".into()),
            2
        )]
    );
    let history: Vec<(String, String)> = db
        .prepare("SELECT finding_id,probe_id FROM probe_history")
        .unwrap()
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(history, vec![(rekeyed.clone(), "probe-kept".into())]);
    let finding: String = db
        .query_row("SELECT finding_id FROM findings", [], |row| row.get(0))
        .unwrap();
    assert_eq!(finding, rekeyed);
}

#[test]
fn promotion_classifies_each_issue_from_codes_annotations_and_retries() {
    let dir = tempdir();
    let mut store = Store::open(Some(dir.clone())).unwrap();
    for record in [
        coded("c1", 1, "click", -32000, true, "cccccccccccccccc"),
        call("c1", 2, "click", "ok", "cccccccccccccccc"),
        coded("c2", 1, "click", -32000, true, "cccccccccccccccc"),
        coded("d1", 1, "type", -32000, true, "dddddddddddddddd"),
        call("d1", 2, "click", "ok", "dddddddddddddddd"),
        coded("d2", 1, "type", -32000, true, "dddddddddddddddd"),
        coded("e1", 1, "open", -32000, false, "eeeeeeeeeeeeeeee"),
        coded("e2", 1, "open", -32000, false, "eeeeeeeeeeeeeeee"),
        coded("f1", 1, "scroll", -32000, false, "ffffffffffffffff"),
        coded("f2", 1, "scroll", -32000, false, "ffffffffffffffff"),
        coded("g1", 1, "wait", -32000, true, "gggggggggggggggg"),
        call("g1", 2, "wait", "ok", "gggggggggggggggg"),
        coded("g2", 1, "wait", -32000, false, "gggggggggggggggg"),
    ] {
        store.append(&record).unwrap();
    }
    for (session, kind) in [("e1", "false-success"), ("f1", "blocked-optimal-path")] {
        store
            .append_annotation(&AnnotationRecord {
                ts: "2026-08-04T12:00:01Z".into(),
                session: mcpeval::privacy::opaque_session(session),
                seq: 1,
                kind: kind.into(),
                note: "observed".into(),
            })
            .unwrap();
    }
    index::build(&dir).unwrap();
    promote(&dir, at(0)).unwrap();

    let db = rusqlite::Connection::open(dir.join("index.db")).unwrap();
    let classes: Vec<(String, String, String)> = db
        .prepare("SELECT tool, class, err_codes FROM issues ORDER BY tool")
        .unwrap()
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    let expected = [
        ("click", "recovers-on-retry"),
        ("open", "false-success"),
        ("scroll", "blocked-optimal-path"),
        ("type", "retry-did-not-recover"),
        ("wait", "recurring-error"),
    ];
    assert_eq!(
        classes,
        expected
            .iter()
            .map(|(tool, class)| (tool.to_string(), class.to_string(), "[-32000]".to_string()))
            .collect::<Vec<_>>()
    );
}

#[test]
fn promotion_records_whether_every_failure_was_retryable_and_findings_json_carries_it() {
    let dir = tempdir();
    let mut store = Store::open(Some(dir.clone())).unwrap();
    let mut unknown = coded("u2", 1, "unknown", -32000, true, "uuuuuuuuuuuuuuuu");
    unknown.error.as_mut().unwrap().retryable = None;
    for record in [
        coded("r1", 1, "retry", -32000, true, "rrrrrrrrrrrrrrrr"),
        coded("r2", 1, "retry", -32000, true, "rrrrrrrrrrrrrrrr"),
        coded("n1", 1, "never", -32000, false, "nnnnnnnnnnnnnnnn"),
        coded("n2", 1, "never", -32000, false, "nnnnnnnnnnnnnnnn"),
        coded("m1", 1, "mixed", -32000, true, "mmmmmmmmmmmmmmmm"),
        coded("m2", 1, "mixed", -32000, false, "mmmmmmmmmmmmmmmm"),
        coded("u1", 1, "unknown", -32000, true, "uuuuuuuuuuuuuuuu"),
        unknown,
    ] {
        store.append(&record).unwrap();
    }
    index::build(&dir).unwrap();
    promote(&dir, at(0)).unwrap();

    let db = rusqlite::Connection::open(dir.join("index.db")).unwrap();
    let stored: Vec<(String, Option<i64>)> = db
        .prepare("SELECT tool, retryable FROM issues ORDER BY tool")
        .unwrap()
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(
        stored,
        [
            ("mixed", None),
            ("never", Some(0)),
            ("retry", Some(1)),
            ("unknown", None)
        ]
        .map(|(tool, flag)| (tool.to_owned(), flag))
    );

    let findings = mcpeval::report::render(
        &mcpeval::report::load_findings(&dir).unwrap(),
        mcpeval::report::ReportFormat::Json,
    )
    .unwrap();
    let mut exposed: Vec<(String, serde_json::Value)> =
        serde_json::from_str::<Vec<serde_json::Value>>(&findings)
            .unwrap()
            .into_iter()
            .map(|row| {
                (
                    row["tool"].as_str().unwrap().to_owned(),
                    row["retryable"].clone(),
                )
            })
            .collect();
    exposed.sort_by(|left, right| left.0.cmp(&right.0));
    assert_eq!(
        exposed,
        [
            ("mixed", json!(null)),
            ("never", json!(false)),
            ("retry", json!(true)),
            ("unknown", json!(null))
        ]
        .map(|(tool, flag)| (tool.to_owned(), flag))
    );
}

#[test]
fn promotion_counts_why_issues_stayed_below_findings() {
    let dir = tempdir();
    let mut store = Store::open(Some(dir.clone())).unwrap();
    for record in [
        call("s1", 1, "click", "error", "aaaaaaaaaaaaaaaa"),
        call("s1", 2, "type", "error", "bbbbbbbbbbbbbbbb"),
        call("s2", 1, "type", "error", "bbbbbbbbbbbbbbbb"),
    ] {
        store.append(&record).unwrap();
    }
    index::build(&dir).unwrap();

    let blocked = promote(
        &dir,
        PromotionConfig {
            threshold: 1000.0,
            ..at(0)
        },
    )
    .unwrap();
    assert_eq!(
        (
            blocked.issues,
            blocked.findings,
            blocked.single_session,
            blocked.below_threshold
        ),
        (2, 0, 1, 1)
    );
    assert_eq!(
        blocked.summary(1000.0),
        "promoted 0 of 2 issues (1 seen in one session only, 1 below threshold 1000.000000)"
    );

    let promoted = promote(&dir, at(0)).unwrap();
    assert_eq!(
        promoted.summary(0.0),
        "promoted 1 of 2 issues (1 seen in one session only)"
    );
    let below_only = mcpeval::promote::PromotionStats {
        single_session: 0,
        ..blocked
    };
    assert_eq!(
        below_only.summary(2.5),
        "promoted 0 of 2 issues (1 below threshold 2.500000)"
    );
    let clean = mcpeval::promote::PromotionStats {
        below_threshold: 0,
        ..below_only
    };
    assert_eq!(clean.summary(2.5), "promoted 0 of 2 issues");
}

fn shim_session(home: &std::path::Path, session: &str) {
    use std::io::{BufRead, BufReader, Write};
    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_mcpeval"))
        .args(["shim", "--server", "demo", "--"])
        .arg(env!("CARGO_BIN_EXE_mcpeval-demo"))
        .args(["--broken", "unstable-errors"])
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
            json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"promote-test","version":"0"}}}),
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

#[test]
fn shim_captured_unstable_error_codes_promote_to_one_classified_finding() {
    let home = tempdir();
    for session in ["one", "two", "three"] {
        shim_session(&home, session);
    }
    let run = |args: &[&str]| {
        let out = std::process::Command::new(env!("CARGO_BIN_EXE_mcpeval"))
            .args(args)
            .env("MCPEVAL_HOME", &home)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap()
    };
    run(&["index"]);
    assert_eq!(
        run(&["promote", "--threshold", "0"]),
        "promoted 1 of 1 issues\n"
    );
    let findings: serde_json::Value =
        serde_json::from_str(&run(&["findings", "--format", "json"])).unwrap();
    let findings = findings.as_array().unwrap();
    assert_eq!(findings.len(), 1, "{findings:?}");
    let finding = &findings[0];
    assert_eq!(
        (&finding["server"], &finding["tool"]),
        (&json!("demo"), &json!("flaky_read"))
    );
    assert_eq!(finding["err_codes"], json!([-32001, -32002]));
    assert_eq!(finding["class"], "unstable-error-code");
    assert_eq!(
        finding["hint"],
        mcpeval::remediation::hint(mcpeval::probe::FailureReason::UnstableErrorCode)
    );
    assert_eq!(finding["state"], "open");
    assert_eq!(
        (&finding["failures"], &finding["sessions"]),
        (&json!(6), &json!(3))
    );
}
