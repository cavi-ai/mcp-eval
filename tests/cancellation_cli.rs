use std::process::Command;

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_mcpeval")
}

fn demo() -> &'static str {
    env!("CARGO_BIN_EXE_mcpeval-demo")
}

fn home() -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!("mcpeval-cancel-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&path).unwrap();
    path
}

fn probe_manifest(body: &str, extra: &[&str]) -> std::process::Output {
    let dir = home();
    let manifest = dir.join("m.json");
    std::fs::write(&manifest, body).unwrap();
    Command::new(bin())
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
        .args(extra)
        .env("MCPEVAL_HOME", &dir)
        .output()
        .unwrap()
}

#[test]
fn an_honoring_server_passes_and_the_defect_fails_with_fixed_reasons() {
    let clean = probe_manifest(
        r#"{"version":1,"probes":[{"id":"cancel-slow","probe":"cancellation","tool":"slow_read","access":"read_only","arguments":{},"grace_seconds":3,"reason":"probe"}]}"#,
        &[],
    );
    assert!(
        clean.status.success(),
        "{}",
        String::from_utf8_lossy(&clean.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&clean.stdout).unwrap();
    assert_eq!(report["passed"], true);
    assert_eq!(report["cases"][0]["probe"], "cancellation");

    let broken = probe_manifest(
        r#"{"version":1,"probes":[{"id":"cancel-slow","probe":"cancellation","tool":"slow_read","access":"read_only","arguments":{},"grace_seconds":3,"reason":"probe"}]}"#,
        &["--broken", "cancellation"],
    );
    assert!(!broken.status.success());
    let report: serde_json::Value = serde_json::from_slice(&broken.stdout).unwrap();
    assert_eq!(report["cases"][0]["reason"], "cancellation-ignored");
}

#[test]
fn a_tool_that_cannot_be_cancelled_unless_it_works() {
    // Preflight: a tool that errors uncancelled must fail with
    // unexpected-outcome, not pass via a broken-server accident.
    let output = probe_manifest(
        r#"{"version":1,"probes":[{"id":"cancel-broken-tool","probe":"cancellation","tool":"break_session","access":"read_only","arguments":{},"grace_seconds":3,"reason":"probe"}]}"#,
        &[],
    );
    assert!(!output.status.success());
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["cases"][0]["reason"], "unexpected-outcome");
}

#[test]
fn cancellation_counts_toward_reliability() {
    let dir = home();
    let manifest = dir.join("m.json");
    std::fs::write(
        &manifest,
        r#"{"version":1,"probes":[{"id":"cancel-slow","probe":"cancellation","tool":"slow_read","access":"read_only","arguments":{},"grace_seconds":3,"reason":"probe"}]}"#,
    )
    .unwrap();
    let output = Command::new(bin())
        .args([
            "probe",
            "--server",
            "demo",
            "--manifest",
            manifest.to_str().unwrap(),
        ])
        .args(["--", demo()])
        .env("MCPEVAL_HOME", &dir)
        .output()
        .unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("reliability=1/1"), "{stdout}");
}

#[test]
fn manifest_validation_rejects_out_of_bounds_cancellation() {
    let dir = home();
    let manifest = dir.join("m.json");
    std::fs::write(
        &manifest,
        r#"{"version":1,"probes":[{"id":"cancel","probe":"cancellation","tool":"slow_read","access":"read_only","arguments":{},"grace_seconds":0,"reason":"probe"}]}"#,
    )
    .unwrap();
    let output = Command::new(bin())
        .args([
            "probe",
            "--server",
            "demo",
            "--manifest",
            manifest.to_str().unwrap(),
        ])
        .args(["--", demo()])
        .env("MCPEVAL_HOME", &dir)
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("grace_seconds"));

    let manifest2 = dir.join("m2.json");
    std::fs::write(
        &manifest2,
        r#"{"version":1,"probes":[{"id":"cancel","probe":"cancellation","tool":"slow_read","access":"read_only","arguments":{},"grace_seconds":3,"reason":"has spaces"}]}"#,
    )
    .unwrap();
    let output = Command::new(bin())
        .args([
            "probe",
            "--server",
            "demo",
            "--manifest",
            manifest2.to_str().unwrap(),
        ])
        .args(["--", demo()])
        .env("MCPEVAL_HOME", &dir)
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("reason is invalid"));
}
