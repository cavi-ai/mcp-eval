use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::{json, Value};

const SERVER: &[&str] = &["python3", "tests/fixtures/record_server.py"];

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_mcpeval")
}

fn home() -> PathBuf {
    let path = std::env::temp_dir().join(format!("mcpeval-arguments-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&path).unwrap();
    path
}

fn run(dir: &Path, args: &[&str]) -> Output {
    Command::new(bin())
        .args(args)
        .args(["--"])
        .args(SERVER)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .env("MCPEVAL_HOME", dir)
        .output()
        .unwrap()
}

fn write(dir: &Path, name: &str, document: &Value) -> String {
    let path = dir.join(name);
    std::fs::write(&path, serde_json::to_vec_pretty(document).unwrap()).unwrap();
    path.display().to_string()
}

fn lost_for(document: &Value, tool: &str) -> Vec<(String, String)> {
    document["readiness"]["areas"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|area| area["checks"].as_array().unwrap().iter())
        .filter(|check| {
            check["tool"] == tool && !check["id"].as_str().unwrap().starts_with("catalog.")
        })
        .map(|check| {
            (
                check["id"].as_str().unwrap().to_owned(),
                check["reason"].as_str().unwrap().to_owned(),
            )
        })
        .collect()
}

#[test]
fn supplied_arguments_exercise_a_tool_the_schema_cannot_feed() {
    let dir = home();
    let output = run(&dir, &["score", "--server", "records", "--format", "json"]);
    assert_eq!(output.status.code(), Some(0));
    let synthesized: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        lost_for(&synthesized, "get_record"),
        vec![
            (
                "reliability.success".into(),
                "reliability-call-failed".into()
            ),
            ("coverage.exercised".into(), "coverage-call-failed".into()),
        ]
    );
    assert!(synthesized["readiness"]["measurement_profile"]
        .get("arguments_sha256")
        .is_none());

    let arguments = write(
        &dir,
        "arguments.json",
        &json!({"get_record": {"id": "rec-1"}}),
    );
    let output = run(
        &dir,
        &[
            "score",
            "--server",
            "records",
            "--format",
            "json",
            "--arguments",
            &arguments,
        ],
    );
    assert_eq!(output.status.code(), Some(0));
    let supplied: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(lost_for(&supplied, "get_record"), vec![]);
    assert!(
        supplied["readiness"]["score"].as_u64() > synthesized["readiness"]["score"].as_u64(),
        "{supplied:#}"
    );
    let digest = supplied["readiness"]["measurement_profile"]["arguments_sha256"]
        .as_str()
        .unwrap();
    assert_eq!(digest.len(), 64);
    // The values never reach the report.
    assert!(!String::from_utf8_lossy(&output.stdout).contains("rec-1"));

    // Runs with different arguments measure different things.
    let baseline = write(&dir, "synthesized.json", &synthesized);
    let current = write(&dir, "supplied.json", &supplied);
    let diff = Command::new(bin())
        .args(["diff", &baseline, &current])
        .env("MCPEVAL_HOME", &dir)
        .output()
        .unwrap();
    let text = String::from_utf8(diff.stdout).unwrap();
    assert!(
        text.contains("readiness  not comparable (measurement profile changed or unavailable)"),
        "{text}"
    );
}

#[test]
fn arguments_must_name_listed_read_only_tools_and_fit_their_schema() {
    let dir = home();
    for (name, document) in [
        ("unknown", json!({"no_such_tool": {}})),
        ("writer", json!({"delete_record": {"id": "rec-1"}})),
        ("mistyped", json!({"get_record": {"id": 5}})),
        ("not-an-object", json!({"get_record": "rec-1"})),
        ("array", json!([{"get_record": {"id": "rec-1"}}])),
    ] {
        let arguments = write(&dir, &format!("{name}.json"), &document);
        let output = run(
            &dir,
            &["score", "--server", "records", "--arguments", &arguments],
        );
        assert_eq!(output.status.code(), Some(2), "{name}: {output:?}");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(!stderr.contains("rec-1"), "{name}: {stderr}");
    }
}

#[test]
fn init_scaffolds_every_tool_it_can_call_and_names_the_rest() {
    let dir = home();
    let manifest = dir.join("records.manifest.json");
    let manifest = manifest.to_str().unwrap();
    let output = run(&dir, &["init", "--server", "records", "--output", manifest]);
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(
        text.contains("  skipped get_record: the call with arguments built from its schema failed; pass working ones with --arguments\n"),
        "{text}"
    );
    assert!(
        text.contains("  skipped delete_record: destructiveHint\n"),
        "{text}"
    );
    let scaffolded: Value = serde_json::from_slice(&std::fs::read(manifest).unwrap()).unwrap();
    assert!(!scaffolded["probes"]
        .as_array()
        .unwrap()
        .iter()
        .any(|case| case["tool"] == "get_record"));

    let arguments = write(
        &dir,
        "arguments.json",
        &json!({"get_record": {"id": "rec-1"}}),
    );
    let output = run(
        &dir,
        &[
            "init",
            "--server",
            "records",
            "--output",
            manifest,
            "--force",
            "--arguments",
            &arguments,
        ],
    );
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(!text.contains("skipped get_record"), "{text}");
    let scaffolded: Value = serde_json::from_slice(&std::fs::read(manifest).unwrap()).unwrap();
    let guessable = scaffolded["probes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|case| case["id"] == "get_record-guessable")
        .unwrap();
    assert_eq!(guessable["arguments"], json!({"id": "rec-1"}));

    let output = run(
        &dir,
        &[
            "probe",
            "--server",
            "records",
            "--manifest",
            manifest,
            "--gate-only",
        ],
    );
    assert_eq!(output.status.code(), Some(0), "{output:?}");
}
