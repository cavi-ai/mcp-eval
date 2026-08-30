use std::process::Command;

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_mcpeval")
}

fn demo() -> &'static str {
    env!("CARGO_BIN_EXE_mcpeval-demo")
}

fn home() -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!("mcpeval-landing-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&path).unwrap();
    path
}

fn write_manifest(dir: &std::path::Path, body: &str) -> std::path::PathBuf {
    let path = dir.join("m.json");
    std::fs::write(&path, body).unwrap();
    path
}

#[test]
fn sarif_format_is_emitted_by_the_cli() {
    let dir = home();
    let manifest = write_manifest(
        &dir,
        r#"{"version":1,"probes":[{"id":"p","probe":"pagination","access":"read_only","max_pages":2}]}"#,
    );
    let output = Command::new(bin())
        .args([
            "probe",
            "--server",
            "demo",
            "--manifest",
            manifest.to_str().unwrap(),
            "--format",
            "sarif",
        ])
        .args(["--", demo(), "--broken", "stalled-cursor"])
        .env("MCPEVAL_HOME", &dir)
        .output()
        .unwrap();
    assert!(!output.status.success());
    let sarif: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(sarif["version"], "2.1.0");
    let results = sarif["runs"][0]["results"].as_array().unwrap();
    assert_eq!(results.len(), 1);
    let text = results[0]["message"]["text"].as_str().unwrap();
    assert!(text.contains("pagination-stalled-cursor"));
    assert!(text.contains("never re-serve a page"));
}

#[test]
fn report_rerenders_a_committed_document_without_any_server() {
    let dir = home();
    let manifest = write_manifest(
        &dir,
        r#"{"version":1,"probes":[
            {"id":"d","probe":"discovery-cost","access":"read_only","max_tools":50,"max_schema_bytes":200000},
            {"id":"t","probe":"token-cost","access":"read_only","max_total_tokens":100000}
        ]}"#,
    );
    let baseline = dir.join("baseline.json");
    let run = Command::new(bin())
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
    assert!(run.status.success());
    std::fs::write(&baseline, &run.stdout).unwrap();

    // Text re-render matches the live summary shape.
    let text = Command::new(bin())
        .args(["report", baseline.to_str().unwrap()])
        .env("MCPEVAL_HOME", &dir)
        .output()
        .unwrap();
    assert!(text.status.success());
    let stdout = String::from_utf8(text.stdout).unwrap();
    assert!(stdout.contains("demo readiness 100/100"), "{stdout}");

    // SARIF re-render is valid SARIF with zero results.
    let sarif = Command::new(bin())
        .args(["report", baseline.to_str().unwrap(), "--format", "sarif"])
        .env("MCPEVAL_HOME", &dir)
        .output()
        .unwrap();
    let sarif: serde_json::Value = serde_json::from_slice(&sarif.stdout).unwrap();
    assert_eq!(sarif["version"], "2.1.0");
    assert_eq!(sarif["runs"][0]["results"].as_array().unwrap().len(), 0);

    // A failing baseline re-renders red and exits non-zero.
    let failing_manifest = write_manifest(
        &dir,
        r#"{"version":1,"probes":[{"id":"p","probe":"pagination","access":"read_only","max_pages":2}]}"#,
    );
    let failing_run = Command::new(bin())
        .args([
            "probe",
            "--server",
            "demo",
            "--manifest",
            failing_manifest.to_str().unwrap(),
            "--format",
            "json",
        ])
        .args(["--", demo(), "--broken", "duplicate-page"])
        .env("MCPEVAL_HOME", &dir)
        .output()
        .unwrap();
    assert!(!failing_run.status.success());
    let failing_path = dir.join("failing.json");
    std::fs::write(&failing_path, &failing_run.stdout).unwrap();

    let rerendered = Command::new(bin())
        .args([
            "report",
            failing_path.to_str().unwrap(),
            "--format",
            "markdown",
        ])
        .env("MCPEVAL_HOME", &dir)
        .output()
        .unwrap();
    assert!(!rerendered.status.success());
    let body = String::from_utf8(rerendered.stdout).unwrap();
    assert!(body.contains("pagination-duplicate-tool"), "{body}");
    assert!(body.contains("### Remediation"), "{body}");

    // Rejects documents that are not probe reports.
    let not_a_report = dir.join("not-a-report.json");
    std::fs::write(&not_a_report, r#"{"hello": "world"}"#).unwrap();
    let rejected = Command::new(bin())
        .args(["report", not_a_report.to_str().unwrap()])
        .env("MCPEVAL_HOME", &dir)
        .output()
        .unwrap();
    assert!(!rejected.status.success());
    assert!(String::from_utf8_lossy(&rejected.stderr).contains("probe-report/v1"));
}

#[test]
fn serve_print_config_emits_client_wiring() {
    let dir = home();
    let output = Command::new(bin())
        .args(["serve", "--listen", "127.0.0.1:8091", "--print-config"])
        .env("MCPEVAL_HOME", &dir)
        .output()
        .unwrap();
    assert!(output.status.success());
    let config: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        config["mcpServers"]["mcpeval"]["url"],
        "http://127.0.0.1:8091/mcp"
    );
}
