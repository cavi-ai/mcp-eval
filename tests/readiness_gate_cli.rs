use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::Value;

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_mcpeval")
}

fn demo() -> &'static str {
    env!("CARGO_BIN_EXE_mcpeval-demo")
}

fn home() -> PathBuf {
    let path = std::env::temp_dir().join(format!("mcpeval-floor-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&path).unwrap();
    path
}

fn run(dir: &Path, args: &[&str]) -> (Output, Value) {
    let output = Command::new(bin())
        .args(args)
        .env("MCPEVAL_HOME", dir)
        .output()
        .unwrap();
    let document = serde_json::from_slice(&output.stdout).unwrap_or(Value::Null);
    (output, document)
}

/// `score --format json` against the demo, with `flags` before `--`.
fn score(dir: &Path, flags: &[&str], server: &[&str]) -> (Output, Value) {
    let mut args = vec!["score", "--server", "demo", "--format", "json"];
    args.extend_from_slice(flags);
    args.push("--");
    args.push(demo());
    args.extend_from_slice(server);
    run(dir, &args)
}

fn write(dir: &Path, name: &str, document: &Value) -> String {
    let path = dir.join(name);
    std::fs::write(&path, serde_json::to_vec_pretty(document).unwrap()).unwrap();
    path.display().to_string()
}

#[test]
fn a_readiness_floor_fails_a_run_the_battery_completed() {
    let dir = home();
    let (output, document) = score(&dir, &[], &["--broken", "lying-errors"]);
    assert_eq!(output.status.code(), Some(0), "{document:#}");
    assert_eq!(document["readiness"]["score"], 76);
    assert!(document["readiness_gate"].is_null(), "{document:#}");

    let (output, document) = score(
        &dir,
        &["--min-readiness", "85"],
        &["--broken", "lying-errors"],
    );
    assert_eq!(output.status.code(), Some(1), "{document:#}");
    let gate = &document["readiness_gate"];
    assert_eq!(gate["min_score"], 85);
    assert_eq!(gate["passed"], false);
    assert_eq!(
        gate["shortfalls"],
        serde_json::json!([{"subject": "score", "observed": 76, "floor": 85}])
    );

    let (output, document) = score(
        &dir,
        &["--min-readiness", "76"],
        &["--broken", "lying-errors"],
    );
    assert_eq!(output.status.code(), Some(0), "{document:#}");
    assert_eq!(document["readiness_gate"]["passed"], true);
    assert_eq!(
        document["readiness_gate"]["shortfalls"],
        serde_json::json!([])
    );
}

#[test]
fn an_area_floor_names_the_area_under_it() {
    let dir = home();
    let (output, document) = score(
        &dir,
        &[
            "--min-area",
            "error-honesty=90",
            "--min-area",
            "protocol=100",
        ],
        &["--broken", "lying-errors"],
    );
    assert_eq!(output.status.code(), Some(1), "{document:#}");
    let shortfalls = document["readiness_gate"]["shortfalls"].as_array().unwrap();
    assert_eq!(shortfalls.len(), 1, "{document:#}");
    assert_eq!(shortfalls[0]["subject"], "error-honesty");
    assert_eq!(shortfalls[0]["floor"], 90);
    assert_eq!(
        document["readiness_gate"]["min_areas"],
        serde_json::json!({"error-honesty": 90, "protocol": 100})
    );

    for bad in ["honesty=90", "error-honesty", "error-honesty=101"] {
        let (output, _) = score(&dir, &["--min-area", bad], &[]);
        assert_eq!(output.status.code(), Some(2), "--min-area {bad}");
    }
}

#[test]
fn excluding_latency_compares_scores_with_latency_at_full_marks() {
    let dir = home();
    let (output, document) = score(&dir, &[], &[]);
    assert_eq!(output.status.code(), Some(0), "{document:#}");
    let readiness = &document["readiness"];
    let with = readiness["score"].as_u64().unwrap();
    let without = readiness["score_without_latency"].as_u64().unwrap();
    assert!(without >= with, "{readiness:#}");
    // The clean demo's slow_read loses latency points.
    let reliability = readiness["areas"]
        .as_array()
        .unwrap()
        .iter()
        .find(|area| area["name"] == "reliability")
        .unwrap();
    let reliability_without = reliability["measurements"]["score_without_latency"]
        .as_u64()
        .unwrap();
    assert!(reliability_without > reliability["score"].as_u64().unwrap());

    let floor = without.to_string();
    let (output, document) = score(&dir, &["--min-readiness", &floor, "--exclude-latency"], &[]);
    assert_eq!(output.status.code(), Some(0), "{document:#}");
    assert_eq!(document["readiness_gate"]["exclude_latency"], true);

    let area_floor = format!("reliability={reliability_without}");
    let (output, _) = score(&dir, &["--min-area", &area_floor], &[]);
    assert_eq!(output.status.code(), Some(1));
    let (output, _) = score(&dir, &["--min-area", &area_floor, "--exclude-latency"], &[]);
    assert_eq!(output.status.code(), Some(0));

    let (output, _) = score(&dir, &["--exclude-latency"], &[]);
    assert_eq!(
        output.status.code(),
        Some(2),
        "--exclude-latency needs a floor"
    );
}

#[test]
fn a_floor_needs_the_standard_battery() {
    let dir = home();
    let manifest = dir.join("demo.manifest.json");
    let (output, _) = run(
        &dir,
        &[
            "init",
            "--server",
            "demo",
            "--confirm-read-only",
            "--output",
            manifest.to_str().unwrap(),
            "--",
            demo(),
        ],
    );
    assert_eq!(output.status.code(), Some(0));
    let manifest = manifest.to_str().unwrap();
    for extra in [&["--gate-only"][..], &["--probe", "token-cost"][..]] {
        let mut args = vec![
            "probe",
            "--server",
            "demo",
            "--manifest",
            manifest,
            "--min-readiness",
            "50",
        ];
        args.extend_from_slice(extra);
        args.extend_from_slice(&["--", demo()]);
        let (output, _) = run(&dir, &args);
        assert_eq!(output.status.code(), Some(2), "{extra:?}");
    }
    let (output, document) = run(
        &dir,
        &[
            "probe",
            "--server",
            "demo",
            "--manifest",
            manifest,
            "--format",
            "json",
            "--min-readiness",
            "85",
            "--",
            demo(),
            "--broken",
            "lying-errors",
        ],
    );
    assert_eq!(output.status.code(), Some(1), "{document:#}");
    assert_eq!(document["passed"], true, "the manifest cases still pass");
    assert_eq!(document["readiness_gate"]["passed"], false);

    // A re-rendered document keeps its verdict.
    let path = write(&dir, "floored.json", &document);
    let (output, _) = run(&dir, &["report", &path]);
    assert_eq!(output.status.code(), Some(1));
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(
        text.contains("readiness floor failed: score 76 < 85"),
        "{text}"
    );
}

#[test]
fn diff_fails_a_readiness_drop_over_the_allowance() {
    let dir = home();
    let (_, clean) = score(&dir, &[], &[]);
    let (_, lying) = score(&dir, &[], &["--broken", "lying-errors"]);
    let baseline = write(&dir, "baseline.json", &clean);
    let current = write(&dir, "current.json", &lying);
    let drop = clean["readiness"]["score"].as_u64().unwrap() - 76;

    let (output, _) = run(
        &dir,
        &["diff", &baseline, &current, "--max-readiness-drop", "5"],
    );
    assert_eq!(output.status.code(), Some(1));
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(
        text.contains(&format!("readiness drop {drop} exceeds 5")),
        "{text}"
    );
    let allowance = drop.to_string();
    let (output, _) = run(
        &dir,
        &[
            "diff",
            &baseline,
            &current,
            "--max-readiness-drop",
            &allowance,
        ],
    );
    assert_eq!(output.status.code(), Some(0));

    let (output, document) = run(
        &dir,
        &[
            "diff",
            &baseline,
            &current,
            "--format",
            "json",
            "--max-readiness-drop",
            "5",
            "--exclude-latency",
        ],
    );
    assert_eq!(output.status.code(), Some(1), "{document:#}");
    let gate = &document["readiness_gate"];
    assert_eq!(gate["max_drop"], 5);
    assert_eq!(gate["exclude_latency"], true);
    assert_eq!(gate["passed"], false);
    assert_eq!(
        gate["drop"].as_u64().unwrap(),
        clean["readiness"]["score_without_latency"]
            .as_u64()
            .unwrap()
            - lying["readiness"]["score_without_latency"]
                .as_u64()
                .unwrap()
    );

    // A side without the latency-free score cannot be compared that way.
    let mut legacy = clean.clone();
    legacy["readiness"]
        .as_object_mut()
        .unwrap()
        .remove("score_without_latency");
    let legacy = write(&dir, "legacy.json", &legacy);
    let (output, _) = run(
        &dir,
        &[
            "diff",
            &legacy,
            &current,
            "--max-readiness-drop",
            "5",
            "--exclude-latency",
        ],
    );
    assert_eq!(output.status.code(), Some(2));

    // Incomparable readiness cannot be gated.
    let mut other = clean.clone();
    other["readiness"]["standard"] = "mcpeval-standard/1".into();
    let other = write(&dir, "other.json", &other);
    let (output, _) = run(
        &dir,
        &["diff", &other, &current, "--max-readiness-drop", "5"],
    );
    assert_eq!(output.status.code(), Some(2));
}
