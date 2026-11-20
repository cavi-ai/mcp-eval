use std::path::Path;
use std::process::Command;

use serde_json::Value;

#[test]
fn catalog_flags_duplicate_descriptions_off_spec_names_and_branching_schema_roots() {
    let home = std::env::temp_dir().join(format!("mcpeval-catalog-{}", uuid::Uuid::new_v4()));
    let output = Command::new(env!("CARGO_BIN_EXE_mcpeval"))
        .args([
            "score", "--server", "docs", "--format", "json", "--", "python3",
        ])
        .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/catalog_checks_server.py"))
        .env("MCPEVAL_HOME", &home)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    let lost: Vec<(String, String, String)> = report["readiness"]["areas"]
        .as_array()
        .unwrap()
        .iter()
        .find(|area| area["name"] == "catalog")
        .unwrap()["checks"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|check| check["id"] != "catalog.output-schema")
        .map(|check| {
            let field = |key: &str| check[key].as_str().unwrap().to_owned();
            (field("tool"), field("id"), field("reason"))
        })
        .collect();
    let expected = [
        (
            "search_docs",
            "catalog.distinct-description",
            "catalog-description-duplicated",
        ),
        (
            "find_docs",
            "catalog.distinct-description",
            "catalog-description-duplicated",
        ),
        ("docs:fetch", "catalog.name-format", "catalog-name-off-spec"),
        (
            "lookup",
            "catalog.schema-portable",
            "catalog-schema-root-unportable",
        ),
    ]
    .map(|(tool, id, reason)| (tool.to_owned(), id.to_owned(), reason.to_owned()));
    assert_eq!(lost, expected, "{report:#}");

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
}
