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
    let path = std::env::temp_dir().join(format!("mcpeval-contract-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&path).unwrap();
    path
}

/// The text output's lines, each split into words.
fn lines(output: &Output) -> Vec<Vec<String>> {
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(|line| line.split_whitespace().map(str::to_owned).collect())
        .collect()
}

fn has_line(output: &Output, words: &[&str]) -> bool {
    lines(output).iter().any(|line| line == words)
}

fn run(dir: &Path, args: &[&str]) -> Output {
    Command::new(bin())
        .args(args)
        .env("MCPEVAL_HOME", dir)
        .output()
        .unwrap()
}

/// A gate-only probe of the demo against a manifest that never calls
/// `describe_status`, written to `name`.
fn report(dir: &Path, name: &str, broken: &[&str]) -> (String, Value) {
    let manifest = dir.join("weather.manifest.json");
    if !manifest.exists() {
        let output = run(
            dir,
            &[
                "init",
                "--server",
                "demo",
                "--tool",
                "report_weather",
                "--output",
                manifest.to_str().unwrap(),
                "--",
                demo(),
            ],
        );
        assert_eq!(output.status.code(), Some(0));
    }
    let mut args = vec![
        "probe",
        "--server",
        "demo",
        "--manifest",
        manifest.to_str().unwrap(),
        "--gate-only",
        "--format",
        "json",
        "--",
        demo(),
    ];
    args.extend_from_slice(broken);
    let output = run(dir, &args);
    // A red gate still records the catalog; an incomplete run may not.
    assert!(
        matches!(output.status.code(), Some(0 | 1)),
        "{broken:?}: {output:?}"
    );
    let path = dir.join(name);
    std::fs::write(&path, &output.stdout).unwrap();
    (
        path.display().to_string(),
        serde_json::from_slice(&output.stdout).unwrap(),
    )
}

#[test]
fn a_new_required_parameter_on_an_unexercised_tool_breaks_the_diff() {
    let dir = home();
    let (before, document) = report(&dir, "before.json", &[]);
    let (after, _) = report(&dir, "after.json", &["--broken", "schema"]);

    let tools = document["catalog"]["tools"].as_array().unwrap();
    assert_eq!(tools.len(), 12);
    let weather = tools
        .iter()
        .find(|tool| tool["name"] == "report_weather")
        .unwrap();
    assert_eq!(weather["class"], "read-only");
    assert_eq!(weather["parameters"].as_object().unwrap().len(), 1);
    // Digests only: no parameter name or schema keyword leaves the server.
    let catalog = document["catalog"].to_string();
    assert!(
        !catalog.contains("city") && !catalog.contains("string"),
        "{catalog}"
    );

    let output = run(&dir, &["diff", &before, &after, "--fail-on-regression"]);
    assert_eq!(output.status.code(), Some(0), "the gate cases all pass");
    let output = run(&dir, &["diff", &before, &after, "--fail-on-breaking"]);
    assert_eq!(output.status.code(), Some(1));
    assert!(
        has_line(
            &output,
            &[
                "describe_status",
                "catalog",
                "BREAKING",
                "parameter-required"
            ]
        ),
        "{output:?}"
    );
    assert!(
        has_line(
            &output,
            &[
                "catalog",
                "1",
                "breaking,",
                "0",
                "to",
                "review,",
                "0",
                "compatible"
            ]
        ),
        "{output:?}"
    );

    let output = run(&dir, &["diff", &before, &after, "--format", "json"]);
    let diff: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        diff["catalog"],
        serde_json::json!({
            "comparable": true,
            "changes": [{
                "tool": "describe_status",
                "change": "parameter-required",
                "severity": "breaking",
                "count": 1
            }]
        })
    );
    let output = run(&dir, &["diff", &before, &after, "--format", "markdown"]);
    let markdown = String::from_utf8(output.stdout).unwrap();
    assert!(
        markdown.contains("| describe_status | `parameter-required` | **breaking** |"),
        "{markdown}"
    );
}

#[test]
fn additive_changes_pass_the_breaking_gate() {
    let dir = home();
    let (before, _) = report(&dir, "before.json", &[]);
    let (bloated, _) = report(&dir, "bloated.json", &["--broken", "bloated"]);
    let (undescribed, _) = report(&dir, "undescribed.json", &["--broken", "undescribed"]);

    let output = run(&dir, &["diff", &before, &bloated, "--fail-on-breaking"]);
    assert_eq!(output.status.code(), Some(0));
    assert!(
        has_line(
            &output,
            &["extra_padding_tool", "catalog", "compatible", "tool-added"]
        ),
        "{output:?}"
    );
    // Rewritten descriptions are prose, not contract.
    let output = run(&dir, &["diff", &before, &undescribed, "--fail-on-breaking"]);
    assert_eq!(output.status.code(), Some(0));
    assert!(
        has_line(
            &output,
            &[
                "catalog",
                "0",
                "breaking,",
                "0",
                "to",
                "review,",
                "0",
                "compatible"
            ]
        ),
        "{output:?}"
    );
}

#[test]
fn a_document_without_a_catalog_cannot_be_gated_on_it() {
    let dir = home();
    let (before, document) = report(&dir, "before.json", &[]);
    let mut legacy = document.clone();
    legacy.as_object_mut().unwrap().remove("catalog");
    let legacy_path = dir.join("legacy.json");
    std::fs::write(&legacy_path, serde_json::to_vec(&legacy).unwrap()).unwrap();
    let legacy_path = legacy_path.display().to_string();

    let output = run(&dir, &["diff", &legacy_path, &before]);
    assert_eq!(output.status.code(), Some(0));
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.contains("catalog  not comparable"), "{text}");
    let output = run(&dir, &["diff", &legacy_path, &before, "--fail-on-breaking"]);
    assert_eq!(output.status.code(), Some(2));
}
