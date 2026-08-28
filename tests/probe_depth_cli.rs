use std::process::Command;

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_mcpeval")
}

fn demo() -> &'static str {
    env!("CARGO_BIN_EXE_mcpeval-demo")
}

fn home() -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!("mcpeval-depth-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&path).unwrap();
    path
}

fn probe(manifest_body: &str, extra: &[&str]) -> std::process::Output {
    let dir = home();
    let manifest = dir.join("m.json");
    std::fs::write(&manifest, manifest_body).unwrap();
    let mut command = Command::new(bin());
    command
        .args([
            "probe",
            "--server",
            "demo",
            "--manifest",
            manifest.to_str().unwrap(),
        ])
        .args(["--", demo()])
        .args(extra)
        .env("MCPEVAL_HOME", &dir);
    command.output().unwrap()
}

#[test]
fn payload_bounds_distinguishes_honest_rejection_from_acceptance() {
    // A clean JSON-RPC error at the declared size is honest bounded
    // behavior when expect_handled is false.
    let output = probe(
        r#"{"version":1,"probes":[{"id":"reject-ok","probe":"payload-bounds","tool":"describe_status","access":"read_only","arguments":{},"field":"blob","size_bytes":5000,"expect_handled":false}]}"#,
        &[],
    );
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("reject-ok payload-bounds pass"));

    // The same size with expect_handled=true: the demo tool accepts and
    // answers, so the case passes too. An erroring server would fail with
    // unexpected-outcome; a crashing server with payload-unhandled.
    let output = probe(
        r#"{"version":1,"probes":[{"id":"handle-ok","probe":"payload-bounds","tool":"describe_status","access":"read_only","arguments":{},"field":"blob","size_bytes":5000,"expect_handled":true}]}"#,
        &[],
    );
    assert!(output.status.success());

    // The injected argument never leaks into output.
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(!stdout.contains("aaaaa"));
}

#[test]
fn surface_listing_validates_declared_surfaces_only() {
    let output = probe(
        r#"{"version":1,"probes":[{"id":"surfaces","probe":"surface-listing","access":"read_only","max_pages":5}]}"#,
        &[],
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("surfaces surface-listing pass"));
    assert!(stdout.contains("tools=2"));

    // A declared surface returning garbage fails with a fixed reason.
    let output = probe(
        r#"{"version":1,"probes":[{"id":"surfaces","probe":"surface-listing","access":"read_only","max_pages":5}]}"#,
        &["--broken", "surface"],
    );
    assert!(!output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("surface-invalid-envelope"), "{stdout}");
}

#[test]
fn output_schema_checks_declared_structured_content() {
    // Clean mode: report_weather declares no outputSchema -> trivial pass.
    let output = probe(
        r#"{"version":1,"probes":[{"id":"weather","probe":"output-schema","tool":"report_weather","access":"read_only","arguments":{"city":"Oslo"}}]}"#,
        &[],
    );
    assert!(output.status.success());
    assert!(String::from_utf8(output.stdout)
        .unwrap()
        .contains("weather output-schema pass attempts=0"));

    // Broken mode declares outputSchema but omits structuredContent.
    let output = probe(
        r#"{"version":1,"probes":[{"id":"weather","probe":"output-schema","tool":"report_weather","access":"read_only","arguments":{"city":"Oslo"}}]}"#,
        &["--broken", "output-schema"],
    );
    assert!(!output.status.success());
    assert!(String::from_utf8(output.stdout)
        .unwrap()
        .contains("output-schema-declared-but-missing"));
}

#[test]
fn new_probes_count_toward_readiness_categories() {
    let dir = home();
    let manifest = dir.join("m.json");
    std::fs::write(
        &manifest,
        r#"{"version":1,"probes":[
            {"id":"surfaces","probe":"surface-listing","access":"read_only","max_pages":5},
            {"id":"blob","probe":"payload-bounds","tool":"describe_status","access":"read_only","arguments":{},"field":"blob","size_bytes":2000,"expect_handled":false},
            {"id":"weather","probe":"output-schema","tool":"report_weather","access":"read_only","arguments":{"city":"Oslo"}}
        ]}"#,
    )
    .unwrap();
    let output = Command::new(bin())
        .args([
            "probe",
            "--server",
            "demo",
            "--manifest",
            manifest.to_str().unwrap(),
            "--format",
            "json",
        ])
        .args(["--", demo()])
        .env("MCPEVAL_HOME", &dir)
        .output()
        .unwrap();
    assert!(output.status.success());
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let categories: Vec<&str> = report["readiness"]["categories"]
        .as_array()
        .unwrap()
        .iter()
        .map(|category| category["name"].as_str().unwrap())
        .collect();
    assert_eq!(categories, ["discovery", "reliability", "contract"]);
    assert_eq!(report["readiness"]["score"], 100);
}
