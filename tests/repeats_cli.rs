use std::path::Path;
use std::process::Command;

use serde_json::Value;

fn score(extra: &[&str]) -> std::process::Output {
    let home = std::env::temp_dir().join(format!("mcpeval-repeats-{}", uuid::Uuid::new_v4()));
    Command::new(env!("CARGO_BIN_EXE_mcpeval"))
        .args(["score", "--server", "status", "--format", "json"])
        .args(extra)
        .args(["--", "python3"])
        .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/flaky_server.py"))
        .env("MCPEVAL_HOME", home)
        .output()
        .unwrap()
}

/// The lost reliability checks on `read_status`, and the profile's count.
fn reliability(output: &std::process::Output) -> (Vec<String>, u64) {
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    let readiness = &report["readiness"];
    let lost = readiness["areas"]
        .as_array()
        .unwrap()
        .iter()
        .find(|area| area["name"] == "reliability")
        .unwrap()["checks"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|check| check["tool"] == "read_status")
        .map(|check| check["id"].as_str().unwrap().to_owned())
        .collect();
    (
        lost,
        readiness["measurement_profile"]["repeats"]
            .as_u64()
            .unwrap(),
    )
}

#[test]
fn repeats_calls_each_tool_more_often_and_catches_a_failure_three_calls_miss() {
    // Every fourth call fails: three calls never see it.
    assert_eq!(reliability(&score(&[])), (vec![], 3));
    assert_eq!(
        reliability(&score(&["--repeats", "5"])),
        (
            vec![
                "reliability.success".to_owned(),
                "reliability.consistent".to_owned()
            ],
            5
        )
    );

    let refused = score(&["--repeats", "2"]);
    assert_eq!(refused.status.code(), Some(2), "{refused:?}");
    assert!(
        String::from_utf8_lossy(&refused.stderr).contains("expected 3 to 100"),
        "{refused:?}"
    );
}
