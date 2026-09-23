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
    assert!(
        body.contains("**Readiness: 100/100** ![mcpeval]("),
        "{body}"
    );
    assert!(
        body.contains("\n*Corpus battery (discovery-cost, token-cost, pagination, surface-listing): 100/100 — above "),
        "{body}"
    );
    assert!(body.contains("https://img.shields.io/badge/mcpeval-100%2F100-brightgreen"));
    assert!(body.contains("| literal-status | instruction-fidelity | pass | 1 |"));
    assert!(body.contains("| discovery | 2/2 |"));
    assert!(!body.contains("CANARY"));
}

#[test]
fn markdown_report_places_the_battery_and_catalog_against_the_corpus() {
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
    assert!(
        body.contains(
            "**Readiness: 100/100** ![mcpeval](https://img.shields.io/badge/mcpeval-100%2F100-brightgreen)\n\n\
             *Corpus battery (discovery-cost, token-cost, pagination, surface-listing): 100/100 — above 1, tied with 2, below 0 of 3 observed servers.*\n\n"
        ),
        "{body}"
    );
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
    assert!(
        body.contains("above 0, tied with 1, below 0 of 1 observed servers.*"),
        "{body}"
    );
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
