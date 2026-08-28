use std::process::Command;

const CLEAN: &str = "tests/fixtures/probe_clean_server.py";
const BROKEN: &str = "tests/fixtures/probe_broken_server.py";
const MANIFEST: &str = "tests/fixtures/mcp-eval.manifest.json";

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_mcpeval")
}

fn home() -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!("mcpeval-md-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&path).unwrap();
    path
}

fn probe(fixture: &str, format: &str) -> std::process::Output {
    Command::new(bin())
        .args([
            "probe",
            "--server",
            "fixture",
            "--manifest",
            MANIFEST,
            "--format",
            format,
        ])
        .args(["--", "python3", fixture])
        .env("MCPEVAL_HOME", home())
        .output()
        .unwrap()
}

#[test]
fn markdown_report_is_pull_request_ready_and_scored() {
    let output = probe(CLEAN, "markdown");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let body = String::from_utf8(output.stdout).unwrap();
    assert!(body.contains("## mcp-eval report — fixture"));
    assert!(body.contains("**Readiness: 100/100 —"), "{body}");
    assert!(body.contains("corpus median 100"), "{body}");
    assert!(body.contains("https://img.shields.io/badge/mcpeval-100%2F100-brightgreen"));
    assert!(body.contains("| literal-status | instruction-fidelity | pass | 1 |"));
    assert!(body.contains("| discovery | 2/2 |"));
    assert!(!body.contains("CANARY"));
}

#[test]
fn markdown_report_reports_failures_without_prose() {
    let output = probe(BROKEN, "markdown");
    assert!(!output.status.success());
    let body = String::from_utf8(output.stdout).unwrap();
    assert!(body.contains("**Readiness:"), "{body}");
    assert!(!body.contains("brightgreen"));
    assert!(body.contains("value-mismatch"));
    assert!(!body.contains("wrong"));
    assert!(!body.contains("CANARY"));
}

#[test]
fn json_report_carries_the_readiness_object() {
    let output = probe(CLEAN, "json");
    assert!(output.status.success());
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["schema"], "mcpeval.probe-report/v1");
    assert_eq!(report["readiness"]["score"], 100);
    assert_eq!(
        report["readiness"]["badge"],
        "https://img.shields.io/badge/mcpeval-100%2F100-brightgreen"
    );
    let categories = report["readiness"]["categories"].as_array().unwrap();
    assert_eq!(categories.len(), 4);
    let names: Vec<&str> = categories
        .iter()
        .map(|category| category["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        ["discovery", "reliability", "contract", "concurrency"]
    );
}

#[test]
fn text_summary_gains_a_readiness_line() {
    let output = probe(CLEAN, "text");
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    let readiness = stdout
        .lines()
        .find(|line| line.contains("readiness"))
        .expect("readiness summary line");
    assert!(readiness.starts_with("fixture readiness 100/100 "));
    assert!(readiness.contains("discovery=2/2"));
}
