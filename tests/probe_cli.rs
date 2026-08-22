use std::process::Command;

const MANIFEST: &str = "tests/fixtures/mcp-eval.manifest.json";
const CLEAN: &str = "tests/fixtures/probe_clean_server.py";
const BROKEN: &str = "tests/fixtures/probe_broken_server.py";

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_mcpeval")
}

fn home() -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!("mcpeval-probe-cli-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&path).unwrap();
    path
}

fn run(server: &str, probe: Option<&str>) -> (std::path::PathBuf, std::process::Output) {
    let home = home();
    let mut command = Command::new(bin());
    command.args(["probe", "--server", "fixture", "--manifest", MANIFEST]);
    if let Some(probe) = probe {
        command.args(["--probe", probe]);
    }
    let output = command
        .args(["--", "python3", server])
        .env("MCPEVAL_HOME", &home)
        .output()
        .unwrap();
    (home, output)
}

#[test]
fn clean_fixture_passes_both_probes_without_exposing_payloads() {
    let (home, output) = run(CLEAN, None);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("repeat-read degradation-over-n pass attempts=5"));
    assert!(stdout.contains("literal-status instruction-fidelity pass attempts=1"));
    assert!(!stdout.contains("CANARY"));
    let stored = std::fs::read_dir(home.join("store"))
        .unwrap()
        .map(|entry| std::fs::read_to_string(entry.unwrap().path()).unwrap())
        .collect::<String>();
    assert!(stored
        .lines()
        .all(|line| line.contains("\"kind\":\"synthetic\"")));
    assert!(stored.contains("str<32"));
    assert!(!stored.contains("CANARY"));
}

#[test]
fn broken_fixture_fails_each_probe_with_fixed_reasons() {
    let (_, degradation) = run(BROKEN, Some("degradation-over-n"));
    assert!(!degradation.status.success());
    let stdout = String::from_utf8(degradation.stdout).unwrap();
    assert!(stdout.contains("fail attempts=3 first_failure=3 reason=unexpected-outcome"));
    assert!(!stdout.contains("CANARY"));

    let (_, fidelity) = run(BROKEN, Some("instruction-fidelity"));
    assert!(!fidelity.status.success());
    let stdout = String::from_utf8(fidelity.stdout).unwrap();
    assert!(stdout.contains("fail attempts=1 first_failure=1 reason=value-mismatch"));
    assert!(!stdout.contains("wrong"));
}

#[test]
fn clean_and_broken_fixtures_measure_discovery_and_schema_guessability() {
    let (_, discovery) = run(CLEAN, Some("discovery-cost"));
    assert!(discovery.status.success());
    let stdout = String::from_utf8(discovery.stdout).unwrap();
    assert!(stdout.contains("bounded-discovery discovery-cost pass attempts=1 tools=7"));

    let (_, discovery) = run(BROKEN, Some("discovery-cost"));
    assert!(!discovery.status.success());
    let stdout = String::from_utf8(discovery.stdout).unwrap();
    assert!(stdout.contains("reason=discovery-limit-exceeded"));
    assert!(!stdout.contains("CANARY"));
    assert!(!stdout.contains("xxxxxxxx"));

    let (_, schema) = run(CLEAN, Some("schema-guessability"));
    assert!(schema.status.success());
    assert!(
        String::from_utf8_lossy(&schema.stdout).contains("naive-status schema-guessability pass")
    );

    let (_, schema) = run(BROKEN, Some("schema-guessability"));
    assert!(!schema.status.success());
    assert!(String::from_utf8_lossy(&schema.stdout).contains("reason=invalid-schema"));
}

#[test]
fn clean_and_broken_fixtures_measure_error_honesty() {
    let (_, clean) = run(CLEAN, Some("error-honesty"));
    assert!(
        clean.status.success(),
        "{}",
        String::from_utf8_lossy(&clean.stderr)
    );
    assert!(String::from_utf8_lossy(&clean.stdout)
        .contains("honest-retry error-honesty pass attempts=3"));

    let (_, broken) = run(BROKEN, Some("error-honesty"));
    assert!(!broken.status.success());
    assert!(String::from_utf8_lossy(&broken.stdout).contains("reason=unstable-error-code"));
    assert!(!String::from_utf8_lossy(&broken.stdout).contains("CANARY"));
}

#[test]
fn clean_and_broken_fixtures_measure_two_client_contention() {
    let (_, clean) = run(CLEAN, Some("contention"));
    assert!(
        clean.status.success(),
        "{}",
        String::from_utf8_lossy(&clean.stderr)
    );
    assert!(
        String::from_utf8_lossy(&clean.stdout).contains("parallel-read contention pass attempts=2")
    );

    let (_, broken) = run(BROKEN, Some("contention"));
    assert!(!broken.status.success());
    assert!(String::from_utf8_lossy(&broken.stdout).contains("reason=contended-client-failed"));
    assert!(!String::from_utf8_lossy(&broken.stdout).contains("CANARY"));
}

fn run_state_recovery(server: &str, allow_mutation: bool) -> std::process::Output {
    let home = home();
    let manifest = home.join("state-recovery.json");
    std::fs::write(
        &manifest,
        r#"{
      "version":1,
      "sandboxes":{"fixture":{"description":"disposable"}},
      "probes":[{
        "id":"recover-session","probe":"state-recovery","access":"mutating","sandbox":"fixture",
        "failure_tool":"break_session","failure_arguments":{},
        "recovery_tool":"recover_session","recovery_arguments":{},
        "validation_tool":"session_status","validation_arguments":{}
      }]
    }"#,
    )
    .unwrap();
    let mut command = Command::new(bin());
    command
        .args(["probe", "--server", "fixture", "--manifest"])
        .arg(&manifest);
    if allow_mutation {
        command.arg("--allow-mutation");
    }
    command
        .args(["--", "python3", server])
        .env("MCPEVAL_HOME", &home)
        .output()
        .unwrap()
}

#[test]
fn state_recovery_is_double_gated_and_validates_post_recovery_health() {
    let denied = run_state_recovery(CLEAN, false);
    assert!(!denied.status.success());
    assert!(String::from_utf8_lossy(&denied.stderr).contains("--allow-mutation"));

    let clean = run_state_recovery(CLEAN, true);
    assert!(
        clean.status.success(),
        "{}",
        String::from_utf8_lossy(&clean.stderr)
    );
    assert!(String::from_utf8_lossy(&clean.stdout)
        .contains("recover-session state-recovery pass attempts=3"));

    let broken = run_state_recovery(BROKEN, true);
    assert!(!broken.status.success());
    assert!(String::from_utf8_lossy(&broken.stdout).contains("reason=validation-failed"));
    assert!(!String::from_utf8_lossy(&broken.stdout).contains("CANARY"));
}

#[test]
fn invalid_manifest_fails_before_launching_the_child() {
    let home = home();
    let manifest = home.join("invalid.json");
    std::fs::write(
        &manifest,
        r#"{"version":1,"mutation_allowed":true,"probes":[]}"#,
    )
    .unwrap();
    let marker = home.join("child-launched");
    let output = Command::new(bin())
        .args(["probe", "--server", "fixture", "--manifest"])
        .arg(&manifest)
        .args(["--", "sh", "-c"])
        .arg(format!("touch {}", marker.display()))
        .env("MCPEVAL_HOME", &home)
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(!marker.exists());
}

#[test]
fn mutation_requires_both_a_declared_sandbox_and_cli_opt_in() {
    let home = home();
    let manifest = home.join("mutating.json");
    std::fs::write(&manifest, r#"{
      "version":1,
      "sandboxes":{"fixture":{"description":"disposable"}},
      "probes":[{"id":"reset","probe":"instruction-fidelity","tool":"reset_counter","access":"mutating","sandbox":"fixture","arguments":{},"expect":{"outcome":"ok","equals":{"reset":true}}}]
    }"#).unwrap();
    let denied = Command::new(bin())
        .args(["probe", "--server", "fixture", "--manifest"])
        .arg(&manifest)
        .args(["--", "python3", BROKEN])
        .env("MCPEVAL_HOME", &home)
        .output()
        .unwrap();
    assert!(!denied.status.success());
    assert!(String::from_utf8_lossy(&denied.stderr).contains("--allow-mutation"));

    let allowed = Command::new(bin())
        .args(["probe", "--server", "fixture", "--manifest"])
        .arg(&manifest)
        .args(["--allow-mutation", "--", "python3", BROKEN])
        .env("MCPEVAL_HOME", &home)
        .output()
        .unwrap();
    assert!(
        allowed.status.success(),
        "{}",
        String::from_utf8_lossy(&allowed.stderr)
    );
}

#[test]
fn token_cost_reports_sorted_usage_in_text_and_json() {
    let (_, text) = run(CLEAN, Some("token-cost"));
    assert!(text.status.success());
    let stdout = String::from_utf8(text.stdout).unwrap();
    let line = stdout
        .lines()
        .find(|line| line.starts_with("token-budget token-cost pass attempts=1 tools=7"))
        .expect("token-cost summary line");
    assert!(line.contains(" total_tokens="));
    assert!(!stdout.contains("CANARY"));

    let home = home();
    let output = Command::new(bin())
        .args([
            "probe",
            "--server",
            "fixture",
            "--manifest",
            MANIFEST,
            "--probe",
            "token-cost",
            "--format",
            "json",
            "--",
            "python3",
            CLEAN,
        ])
        .env("MCPEVAL_HOME", &home)
        .output()
        .unwrap();
    assert!(output.status.success());
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["schema"], "mcpeval.probe-report/v1");
    assert_eq!(report["server"], "fixture");
    assert_eq!(report["passed"], true);
    let case = &report["cases"][0];
    assert_eq!(case["id"], "token-budget");
    assert_eq!(case["probe"], "token-cost");
    assert_eq!(case["passed"], true);
    assert_eq!(case["reason"], serde_json::Value::Null);
    assert_eq!(case["measurements"]["tool_count"], 7);
    let per_tool = case["measurements"]["per_tool"].as_array().unwrap();
    assert_eq!(per_tool.len(), 7);
    let tokens: Vec<u64> = per_tool
        .iter()
        .map(|tool| tool["tokens"].as_u64().unwrap())
        .collect();
    assert!(tokens.windows(2).all(|pair| pair[0] >= pair[1]));
    let total: u64 = tokens.iter().sum();
    assert_eq!(
        case["measurements"]["total_tokens"].as_u64().unwrap(),
        total
    );
    // The JSON document is diffable: no timestamps, sessions, or raw payloads.
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(!text.contains("CANARY"));
    assert!(report.get("session").is_none());
    assert!(report.get("ts").is_none());
}

#[test]
fn token_cost_fails_with_fixed_reason_over_budget() {
    let home = home();
    let manifest = home.join("tight.manifest.json");
    std::fs::write(
        &manifest,
        serde_json::json!({
            "version": 1,
            "probes": [{
                "id": "tight-budget",
                "probe": "token-cost",
                "access": "read_only",
                "max_total_tokens": 1
            }]
        })
        .to_string(),
    )
    .unwrap();
    let output = Command::new(bin())
        .args([
            "probe",
            "--server",
            "fixture",
            "--manifest",
            manifest.to_str().unwrap(),
            "--format",
            "json",
            "--",
            "python3",
            CLEAN,
        ])
        .env("MCPEVAL_HOME", &home)
        .output()
        .unwrap();
    assert!(!output.status.success());
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["passed"], false);
    assert_eq!(report["cases"][0]["reason"], "token-budget-exceeded");
    assert_eq!(report["cases"][0]["first_failure"], 1);
}
