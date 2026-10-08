use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{mpsc, Arc, Barrier};
use std::time::Duration;

use mcpeval::trends::{self, TrendPoint};

struct Home(PathBuf);

impl Home {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("mcpeval-trends-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(root.join("store/probes")).unwrap();
        Self(root)
    }

    fn journal(&self) -> PathBuf {
        self.0.join("store/probes/history.jsonl")
    }

    fn append(&self, server: &str, ts: &str, score: u64) {
        let record = serde_json::json!({"server":server,"ts":ts,"score":score,
            "passed":true,"cases_total":1,"cases_passed":1});
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.journal())
            .unwrap();
        writeln!(file, "{record}").unwrap();
    }
}

impl Drop for Home {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn values(points: Vec<TrendPoint>) -> Vec<(String, String, u64)> {
    points
        .into_iter()
        .map(|p| (p.server, p.ts, p.score))
        .collect()
}

fn fresh(root: &Path, last: usize) -> Vec<(String, String, u64)> {
    let copy = Home::new();
    std::fs::copy(root.join("store/probes/history.jsonl"), copy.journal()).unwrap();
    values(trends::load(&copy.0, last).unwrap())
}

#[test]
fn append_refresh_reuses_history_and_preserves_ties_and_changing_limits() {
    let home = Home::new();
    home.append("b", "03", 30);
    home.append("a", "02", 20);
    home.append("a", "03", 31);
    assert_eq!(
        values(trends::load(&home.0, 1).unwrap()),
        vec![("a".into(), "03".into(), 31), ("b".into(), "03".into(), 30)]
    );
    assert!(
        home.0.join("trends.db").is_file(),
        "refresh must retain a private cache"
    );
    let db = rusqlite::Connection::open(home.0.join("trends.db")).unwrap();
    db.execute_batch("CREATE TRIGGER reject_old BEFORE INSERT ON trend_points WHEN json_extract(NEW.point,'$.score')=20 BEGIN SELECT RAISE(ABORT,'historical row inserted again'); END;").unwrap();
    home.append("a", "01", 10);
    home.append("a", "03", 32);
    home.append("b", "02", 21);
    assert_eq!(
        values(trends::load(&home.0, 2).unwrap()),
        vec![
            ("a".into(), "03".into(), 31),
            ("a".into(), "03".into(), 32),
            ("b".into(), "02".into(), 21),
            ("b".into(), "03".into(), 30)
        ]
    );
    assert_eq!(
        values(trends::load(&home.0, 10).unwrap()),
        fresh(&home.0, 10)
    );
    assert_eq!(
        db.query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE name='reject_old'",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        1
    );
    assert_eq!(
        db.query_row("SELECT offset FROM trend_checkpoint", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        std::fs::metadata(home.journal()).unwrap().len() as i64
    );
    assert!(trends::load(&home.0, 0).unwrap().is_empty());
}

#[test]
fn changed_schema_rebuilds_and_corrupt_cache_uses_validated_journal() {
    let home = Home::new();
    home.append("demo", "01", 10);
    assert_eq!(
        values(trends::load(&home.0, 1).unwrap()),
        vec![("demo".into(), "01".into(), 10)]
    );
    {
        let db = rusqlite::Connection::open(home.0.join("trends.db")).unwrap();
        db.execute_batch("DROP INDEX trend_recent").unwrap();
    }
    home.append("demo", "02", 20);
    assert_eq!(
        values(trends::load(&home.0, 10).unwrap()),
        fresh(&home.0, 10)
    );
    {
        let db = rusqlite::Connection::open(home.0.join("trends.db")).unwrap();
        assert_eq!(
            db.query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE name='trend_recent'",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            1
        );
    }
    std::fs::write(home.0.join("trends.db"), b"broken cache").unwrap();
    home.append("demo", "03", u64::MAX);
    assert_eq!(
        values(trends::load(&home.0, 1).unwrap()),
        vec![("demo".into(), "03".into(), u64::MAX)]
    );
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(home.journal())
        .unwrap();
    file.write_all(b"{bad}\n").unwrap();
    assert!(
        trends::load(&home.0, 1).is_err(),
        "cache failure must not mask journal corruption"
    );
}

#[test]
fn oversized_and_invalid_profile_appends_preserve_prior_cache() {
    for oversized in [true, false] {
        let home = Home::new();
        home.append("demo", "01", 10);
        trends::load(&home.0, 1).unwrap();
        let before = std::fs::read(home.journal()).unwrap();
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(home.journal())
            .unwrap();
        if oversized {
            file.write_all(&vec![b' '; 4 * 1024 * 1024 + 1]).unwrap();
            file.write_all(b"\n").unwrap();
        } else {
            let mut profile = mcpeval::measurement::MeasurementProfile::current(false, &[]);
            profile.schema = "mcpeval.measurement-profile/v999".into();
            let record = serde_json::json!({"server":"demo","ts":"02","score":20,
                "passed":true,"cases_total":1,"cases_passed":1,"measurement_profile":profile});
            writeln!(file, "{record}").unwrap();
        }
        assert!(trends::load(&home.0, 1).is_err());
        let db = rusqlite::Connection::open(home.0.join("trends.db")).unwrap();
        assert_eq!(
            db.query_row("SELECT COUNT(*) FROM trend_points", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            1
        );
        assert_eq!(
            db.query_row("SELECT offset FROM trend_checkpoint", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            before.len() as i64
        );
        drop(file);
        std::fs::write(home.journal(), before).unwrap();
        home.append("demo", "02", 20);
        assert_eq!(
            values(trends::load(&home.0, 10).unwrap()),
            fresh(&home.0, 10)
        );
    }
}

#[test]
fn locked_journal_blocks_both_recording_and_loading_until_released() {
    for writing in [true, false] {
        let home = Home::new();
        home.append("demo", "01", 10);
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(home.journal())
            .unwrap();
        file.lock().unwrap();
        let root = home.0.clone();
        let (started_tx, started_rx) = mpsc::channel();
        let (done_tx, done_rx) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            started_tx.send(()).unwrap();
            if writing {
                let report = mcpeval::probe::ProbeReport {
                    readiness: Some(mcpeval::score::fold(&Default::default())),
                    ..Default::default()
                };
                trends::record(&root, "writer", &report).unwrap();
            } else {
                assert_eq!(trends::load(&root, 1).unwrap().len(), 1);
            }
            done_tx.send(()).unwrap();
        });
        started_rx.recv().unwrap();
        let blocked = matches!(
            done_rx.recv_timeout(Duration::from_millis(150)),
            Err(mpsc::RecvTimeoutError::Timeout)
        );
        file.unlock().unwrap();
        if blocked {
            done_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        }
        worker.join().unwrap();
        assert!(
            blocked,
            "{} ignored the journal lock",
            if writing { "writer" } else { "reader" }
        );
    }
}

#[test]
fn changed_truncated_removed_and_replaced_journals_match_clean_reconstruction() {
    for change in ["edit", "truncate", "replace", "remove"] {
        let home = Home::new();
        home.append("demo", "01", 10);
        home.append("demo", "02", 20);
        trends::load(&home.0, 1).unwrap();
        match change {
            "edit" => {
                let text = std::fs::read_to_string(home.journal()).unwrap();
                std::fs::write(home.journal(), text.replace("20", "30")).unwrap();
            }
            "truncate" => std::fs::write(home.journal(), b"").unwrap(),
            "replace" => {
                std::fs::remove_file(home.journal()).unwrap();
                home.append("other", "00", 5);
            }
            "remove" => {
                std::fs::remove_file(home.journal()).unwrap();
                assert!(trends::load(&home.0, 1).unwrap().is_empty());
                home.append("new", "04", 40);
            }
            _ => unreachable!(),
        }
        assert_eq!(
            values(trends::load(&home.0, 10).unwrap()),
            fresh(&home.0, 10)
        );
    }
}

#[test]
fn unfinished_tail_is_retried_and_bad_complete_records_do_not_advance_cache() {
    let home = Home::new();
    home.append("demo", "01", 10);
    trends::load(&home.0, 10).unwrap();
    let valid = std::fs::read(home.journal()).unwrap();
    let tail =
        br#"{"server":"demo","ts":"02","score":20,"passed":true,"cases_total":1,"cases_passed":1}"#;
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(home.journal())
        .unwrap();
    file.write_all(&tail[..30]).unwrap();
    assert_eq!(
        values(trends::load(&home.0, 10).unwrap()),
        vec![("demo".into(), "01".into(), 10)]
    );
    file.write_all(&tail[30..]).unwrap();
    file.write_all(b"\n").unwrap();
    assert_eq!(
        values(trends::load(&home.0, 10).unwrap()),
        vec![
            ("demo".into(), "01".into(), 10),
            ("demo".into(), "02".into(), 20)
        ]
    );
    let before = std::fs::read(home.journal()).unwrap();
    file.write_all(b"{bad}\n").unwrap();
    assert!(
        trends::load(&home.0, 0).is_err(),
        "zero limit must still reject corrupt history"
    );
    let db = rusqlite::Connection::open(home.0.join("trends.db")).unwrap();
    assert_eq!(
        db.query_row("SELECT COUNT(*) FROM trend_points", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        2
    );
    drop(file);
    std::fs::write(home.journal(), before).unwrap();
    home.append("demo", "03", 30);
    assert_eq!(
        values(trends::load(&home.0, 10).unwrap()),
        fresh(&home.0, 10)
    );
    std::fs::write(home.journal(), valid).unwrap();
    assert_eq!(
        values(trends::load(&home.0, 10).unwrap()),
        fresh(&home.0, 10)
    );
}

#[test]
fn unavailable_cache_falls_back_without_changing_journal_bytes() {
    let home = Home::new();
    home.append("demo", "01", 10);
    std::fs::create_dir(home.0.join("trends.db")).unwrap();
    let before = std::fs::read(home.journal()).unwrap();
    assert_eq!(
        values(trends::load(&home.0, 1).unwrap()),
        vec![("demo".into(), "01".into(), 10)]
    );
    assert_eq!(std::fs::read(home.journal()).unwrap(), before);
    home.append("demo", "02", 20);
    assert_eq!(
        values(trends::load(&home.0, 1).unwrap()),
        vec![("demo".into(), "02".into(), 20)]
    );
}

#[test]
fn concurrent_recorders_and_refreshers_keep_every_complete_point_once() {
    let home = Home::new();
    let barrier = Arc::new(Barrier::new(5));
    let mut workers = Vec::new();
    for number in 0..4 {
        let root = home.0.clone();
        let barrier = barrier.clone();
        workers.push(std::thread::spawn(move || {
            let report = mcpeval::probe::ProbeReport {
                readiness: Some(mcpeval::score::fold(&Default::default())),
                ..Default::default()
            };
            barrier.wait();
            for _ in 0..12 {
                trends::record(&root, &format!("writer-{number}"), &report).unwrap();
                trends::load(&root, 2).unwrap();
            }
        }));
    }
    barrier.wait();
    for worker in workers {
        worker.join().unwrap();
    }
    let points = trends::load(&home.0, 100).unwrap();
    assert_eq!(points.len(), 48);
    for number in 0..4 {
        assert_eq!(
            points
                .iter()
                .filter(|p| p.server == format!("writer-{number}"))
                .count(),
            12
        );
    }
    assert_eq!(values(points), fresh(&home.0, 100));
}

#[test]
fn native_share_includes_journal_and_excludes_private_trend_cache() {
    let home = Home::new();
    home.append("demo", "2026-10-08T00:00:00.000Z", 10);
    trends::load(&home.0, 1).unwrap();
    assert!(home.0.join("trends.db").is_file());
    let output_home = Home::new();
    let output = output_home.0.join("envelope");
    let result = std::process::Command::new(env!("CARGO_BIN_EXE_mcpeval"))
        .args(["share", "--include-probe-history", "--dir"])
        .arg(&output)
        .env("MCPEVAL_HOME", &home.0)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(!output.join("trends.db").exists());
    let files: Vec<_> = std::fs::read_dir(output.join("store/probes"))
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert_eq!(files, vec![std::ffi::OsString::from("history.jsonl")]);
    assert_eq!(
        std::fs::read(output.join("store/probes/history.jsonl")).unwrap(),
        std::fs::read(home.journal()).unwrap()
    );
}
