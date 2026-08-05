use chrono::{TimeZone, Utc};

use mcpeval::index;
use mcpeval::promote::{
    calibrate_seed, promote, resolve_threshold, score, wilson_lower_bound, PromotionConfig,
    ScoreInput,
};
use mcpeval::record::{CallRecord, ErrorInfo};
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
