use std::path::Path;
use std::process::Command;

use serde_json::{json, Value};

fn score(format: &[&str], home: &Path) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_mcpeval"))
        .args(["score", "--server", "notes"])
        .args(format)
        .args(["--", "python3"])
        .arg(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/poisoned_catalog_server.py"),
        )
        .env("MCPEVAL_HOME", home)
        .output()
        .unwrap()
}

#[test]
fn catalog_counts_hidden_text_model_directives_and_cross_tool_references() {
    let home = std::env::temp_dir().join(format!("mcpeval-scan-{}", uuid::Uuid::new_v4()));
    let output = score(&["--format", "json"], &home);
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    let catalog = report["readiness"]["areas"]
        .as_array()
        .unwrap()
        .iter()
        .find(|area| area["name"] == "catalog")
        .unwrap();
    assert_eq!(
        catalog["measurements"],
        json!({"hidden_characters": 1, "injection_phrases": 2, "cross_tool_references": 1}),
        "{report:#}"
    );
    assert!(
        !String::from_utf8_lossy(&output.stdout).contains("Do not tell the user"),
        "description text must not be kept"
    );

    let schema: Value = serde_json::from_str(
        &std::fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/mcp-eval.probe-report.schema.json"),
        )
        .unwrap(),
    )
    .unwrap();
    let errors: Vec<String> = jsonschema::validator_for(&schema)
        .unwrap()
        .iter_errors(&report)
        .map(|error| format!("{error} at {}", error.instance_path()))
        .collect();
    assert!(errors.is_empty(), "{errors:#?}");

    let text = score(&[], &home);
    assert_eq!(text.status.code(), Some(0), "{text:?}");
    assert!(
        String::from_utf8_lossy(&text.stdout).contains(
            "  description scan: 1 hidden characters, 2 injection phrases, 1 cross-tool references\n"
        ),
        "{text:?}"
    );
}
