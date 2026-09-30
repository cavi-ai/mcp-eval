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
    probe_in(&home(), fixture, format)
}

fn probe_in(home: &std::path::Path, fixture: &str, format: &str) -> std::process::Output {
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
        .env("MCPEVAL_HOME", home)
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
    assert!(body.contains("**Readiness: "), "{body}");
    assert!(
        body.contains("/100** ![mcpeval](https://img.shields.io/badge/mcpeval-"),
        "{body}"
    );
    assert!(body.contains("`mcpeval-standard/"), "{body}");
    assert!(body.contains("| Area | Weight | Score |"), "{body}");
    assert!(body.contains("**Gate:** 7/7 cases passed"), "{body}");
    assert!(!body.contains("Corpus battery"), "{body}");
    assert!(body.contains("| literal-status | instruction-fidelity | pass | 1 |"));
    assert!(!body.contains("CANARY"));
}

#[test]
fn markdown_report_places_the_catalog_against_the_corpus() {
    let dir = home();
    std::fs::write(
        dir.join("corpus.json"),
        r#"{"schema":"mcpeval.readiness-corpus/v1","source":"test corpus",
            "battery":["discovery-cost","token-cost","pagination","surface-listing"],
            "observations":[
                {"server":"a","score":100,"tool_count":3,"catalog_tokens":1},
                {"server":"b","score":100,"tool_count":90,"catalog_tokens":1000000},
                {"server":"c","score":75,"tool_count":200,"catalog_tokens":2000000}
        ]}"#,
    )
    .unwrap();
    let json = probe_in(&dir, CLEAN, "json");
    let report: serde_json::Value = serde_json::from_slice(&json.stdout).unwrap();
    let token_case = report["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|case| case["probe"] == "token-cost")
        .unwrap();
    let tokens = token_case["measurements"]["total_tokens"].as_u64().unwrap();
    let tools = token_case["measurements"]["tool_count"].as_u64().unwrap();

    let output = probe_in(&dir, CLEAN, "markdown");
    assert!(output.status.success());
    let body = String::from_utf8(output.stdout).unwrap();
    // Battery placement waits for a corpus collected under the standard.
    assert!(!body.contains("Corpus battery"), "{body}");
    assert!(
        body.contains(&format!(
            "\n*Catalog: {tokens} tokens over {tools} tools — lighter than 2 of 3 observed servers (median 1000000 tokens).*\n"
        )),
        "{body}"
    );

    // Without measurements in the corpus, the catalog line is omitted.
    std::fs::write(
        dir.join("corpus.json"),
        r#"{"schema":"mcpeval.readiness-corpus/v1","source":"test corpus","observations":[
            {"server":"a","score":100}
        ]}"#,
    )
    .unwrap();
    let body = String::from_utf8(probe_in(&dir, CLEAN, "markdown").stdout).unwrap();
    assert!(!body.contains("*Catalog:"), "{body}");
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
    assert_eq!(report["schema"], "mcpeval.probe-report/v2");
    assert_eq!(report["gate"], serde_json::json!({"passed": 7, "total": 7}));
    let readiness = &report["readiness"];
    assert!(readiness["standard"]
        .as_str()
        .unwrap()
        .starts_with("mcpeval-standard/"));
    assert!(readiness["badge"]
        .as_str()
        .unwrap()
        .starts_with("https://img.shields.io/badge/mcpeval-"));
    let names: Vec<&str> = readiness["areas"]
        .as_array()
        .unwrap()
        .iter()
        .map(|area| area["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["context", "reliability", "coverage"]);
}

#[test]
fn text_summary_gains_a_readiness_line() {
    let output = probe(CLEAN, "text");
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("\nfixture gate 7/7 passed\n"), "{stdout}");
    let readiness = stdout
        .lines()
        .find(|line| line.starts_with("fixture readiness "))
        .expect("readiness summary line");
    assert!(
        readiness.contains(" standard=mcpeval-standard/"),
        "{readiness}"
    );
}
