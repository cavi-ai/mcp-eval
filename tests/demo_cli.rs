use std::process::Command;

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_mcpeval")
}

fn demo() -> &'static str {
    env!("CARGO_BIN_EXE_mcpeval-demo")
}

fn home() -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!("mcpeval-demo-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&path).unwrap();
    path
}

fn run(dir: &std::path::Path, args: &[&str]) -> std::process::Output {
    Command::new(bin())
        .args(args)
        .env("MCPEVAL_HOME", dir)
        .output()
        .unwrap()
}

#[test]
fn init_then_probe_scores_the_demo_server_green() {
    let dir = home();
    let manifest = dir.join("mcp-eval.manifest.json");
    let init = run(
        &dir,
        &[
            "init",
            "--server",
            "demo",
            "--confirm-read-only",
            "--output",
            manifest.to_str().unwrap(),
            "--",
            demo(),
        ],
    );
    assert!(
        init.status.success(),
        "{}",
        String::from_utf8_lossy(&init.stderr)
    );
    let stdout = String::from_utf8(init.stdout).unwrap();
    assert!(stdout.contains("9 tools"), "{stdout}");
    assert!(stdout.contains("schema-guessability cases"), "{stdout}");

    let probe = run(
        &dir,
        &[
            "probe",
            "--server",
            "demo",
            "--manifest",
            manifest.to_str().unwrap(),
            "--format",
            "json",
            "--",
            demo(),
        ],
    );
    assert!(
        probe.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&probe.stdout),
        String::from_utf8_lossy(&probe.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&probe.stdout).unwrap();
    assert_eq!(report["passed"], true);
    assert_eq!(report["readiness"]["score"], 100);
    // No payloads from the demo server leak into the report.
    let text = String::from_utf8(probe.stdout).unwrap();
    assert!(!text.contains("status: ready"));
}

#[test]
fn broken_modes_fail_with_their_fixed_reasons() {
    let cases: &[(&str, &str, &str)] = &[
        (
            "schema",
            r#"{"id":"x","probe":"schema-guessability","tool":"describe_status","access":"read_only","arguments":{}}"#,
            "invalid-schema",
        ),
        (
            "fidelity",
            r#"{"id":"x","probe":"instruction-fidelity","tool":"describe_status","access":"read_only","arguments":{},"expect":{"outcome":"ok","required_result_fields":["status"],"equals":{"status":"ready"}}}"#,
            "value-mismatch",
        ),
        (
            "unstable-errors",
            r#"{"id":"x","probe":"error-honesty","tool":"flaky_read","access":"read_only","arguments":{},"max_attempts":4,"expect_retryable":true}"#,
            "unstable-error-code",
        ),
        (
            "bloated",
            r#"{"id":"x","probe":"token-cost","access":"read_only","max_total_tokens":100,"max_tool_tokens":50}"#,
            "token-budget-exceeded",
        ),
        (
            "duplicate-page",
            r#"{"id":"x","probe":"pagination","access":"read_only","max_pages":3}"#,
            "pagination-duplicate-tool",
        ),
        (
            "stalled-cursor",
            r#"{"id":"x","probe":"pagination","access":"read_only","max_pages":3}"#,
            "pagination-stalled-cursor",
        ),
        (
            "slow",
            r#"{"id":"x","probe":"latency-budget","tool":"slow_read","access":"read_only","arguments":{},"attempts":2,"max_latency_ms":50}"#,
            "latency-budget-exceeded",
        ),
    ];
    for (aspect, probe, reason) in cases {
        let dir = home();
        let manifest = dir.join("m.json");
        std::fs::write(&manifest, format!(r#"{{"version":1,"probes":[{probe}]}}"#)).unwrap();
        let output = Command::new(bin())
            .args([
                "probe",
                "--server",
                "demo",
                "--manifest",
                manifest.to_str().unwrap(),
            ])
            .args(["--", demo(), "--broken", aspect])
            .env("MCPEVAL_HOME", &dir)
            .output()
            .unwrap();
        assert!(!output.status.success(), "{aspect} should fail");
        let stdout = String::from_utf8(output.stdout).unwrap();
        assert!(
            stdout.contains(reason),
            "{aspect}: expected {reason} in {stdout}"
        );
    }
}

#[test]
fn contention_and_state_recovery_pass_against_the_demo() {
    let dir = home();
    let manifest = dir.join("m.json");
    std::fs::write(
        &manifest,
        r#"{
          "version": 1,
          "sandboxes": {"demo": {"description": "disposable demo session"}},
          "probes": [
            {"id":"parallel-read","probe":"contention","tool":"shared_read","access":"read_only","arguments":{}},
            {"id":"recover","probe":"state-recovery","access":"mutating","sandbox":"demo",
             "failure_tool":"break_session","failure_arguments":{},
             "recovery_tool":"recover_session","recovery_arguments":{},
             "validation_tool":"session_status","validation_arguments":{}}
          ]
        }"#,
    )
    .unwrap();
    let output = Command::new(bin())
        .args([
            "probe",
            "--server",
            "demo",
            "--manifest",
            manifest.to_str().unwrap(),
            "--allow-mutation",
        ])
        .args(["--", demo()])
        .env("MCPEVAL_HOME", &dir)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("parallel-read contention pass"));
    assert!(stdout.contains("recover state-recovery pass attempts=3"));
}
