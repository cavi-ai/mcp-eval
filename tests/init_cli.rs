use std::process::Command;

const CLEAN: &str = "tests/fixtures/probe_clean_server.py";

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_mcpeval")
}

fn home() -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!("mcpeval-init-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&path).unwrap();
    path
}

#[test]
fn init_scaffolds_a_read_only_manifest_without_schema_calls() {
    let home = home();
    let manifest = home.join("mcp-eval.manifest.json");
    let output = Command::new(bin())
        .args(["init", "--server", "fixture"])
        .arg("--output")
        .arg(&manifest)
        .args(["--", "python3", CLEAN])
        .env("MCPEVAL_HOME", &home)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("wrote") && stdout.contains("7 tools"));
    assert!(stdout.contains("0 schema-guessability cases"), "{stdout}");

    // The scaffold must be strict-manifest valid and the measured bounds
    // must be generous enough for the very same server to pass them.
    let probe = Command::new(bin())
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
    assert!(
        probe.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&probe.stdout),
        String::from_utf8_lossy(&probe.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&probe.stdout).unwrap();
    assert_eq!(report["passed"], true);
    let kinds: Vec<&str> = report["cases"]
        .as_array()
        .unwrap()
        .iter()
        .map(|case| case["probe"].as_str().unwrap())
        .collect();
    assert!(kinds.contains(&"discovery-cost"));
    assert!(kinds.contains(&"token-cost"));
    assert!(!kinds.contains(&"schema-guessability"));

    let body = std::fs::read_to_string(&manifest).unwrap();
    let value: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(value["version"], 1);
}

#[test]
fn init_adds_schema_cases_only_after_read_only_attestation() {
    let home = home();
    let manifest = home.join("mcp-eval.manifest.json");
    let output = Command::new(bin())
        .args([
            "init",
            "--server",
            "fixture",
            "--confirm-read-only",
            "--output",
        ])
        .arg(&manifest)
        .args(["--", "python3", CLEAN])
        .env("MCPEVAL_HOME", &home)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let value: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&manifest).unwrap()).unwrap();
    assert!(value["probes"]
        .as_array()
        .unwrap()
        .iter()
        .any(|case| case["probe"] == "schema-guessability"));
}

#[test]
fn init_refuses_to_overwrite_without_force() {
    let home = home();
    let manifest = home.join("mcp-eval.manifest.json");
    let first = Command::new(bin())
        .args(["init", "--server", "fixture"])
        .arg("--output")
        .arg(&manifest)
        .args(["--", "python3", CLEAN])
        .env("MCPEVAL_HOME", &home)
        .output()
        .unwrap();
    assert!(first.status.success());
    assert!(manifest.exists());

    let denied = Command::new(bin())
        .args(["init", "--server", "fixture"])
        .arg("--output")
        .arg(&manifest)
        .args(["--", "python3", CLEAN])
        .env("MCPEVAL_HOME", &home)
        .output()
        .unwrap();
    assert!(!denied.status.success());
    assert!(String::from_utf8_lossy(&denied.stderr).contains("--force"));

    let forced = Command::new(bin())
        .args(["init", "--server", "fixture", "--force"])
        .arg("--output")
        .arg(&manifest)
        .args(["--", "python3", CLEAN])
        .env("MCPEVAL_HOME", &home)
        .output()
        .unwrap();
    assert!(
        forced.status.success(),
        "{}",
        String::from_utf8_lossy(&forced.stderr)
    );
}

#[test]
fn schema_command_emits_a_parseable_schema_covering_every_probe_kind() {
    let output = Command::new(bin()).arg("schema").output().unwrap();
    assert!(output.status.success());
    let schema: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        schema["$schema"],
        "https://json-schema.org/draft/2020-12/schema"
    );
    assert_eq!(schema["properties"]["version"]["const"], 1);
    let body = schema.to_string();
    for kind in [
        "contention",
        "error-honesty",
        "state-recovery",
        "discovery-cost",
        "token-cost",
        "schema-guessability",
        "degradation-over-n",
        "instruction-fidelity",
        "latency-budget",
        "pagination",
    ] {
        assert!(
            body.contains(&format!("\"const\":\"{kind}\"")),
            "{kind} missing"
        );
    }
    assert!(body.contains("\"additionalProperties\":false"));
}
