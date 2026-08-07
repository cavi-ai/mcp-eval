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
