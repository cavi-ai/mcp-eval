#![cfg(target_os = "macos")]

use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/sandbox_server.py")
}

fn score(home: &Path, flags: &[&str], escape: &Path, port: u16) -> Value {
    let output = Command::new(env!("CARGO_BIN_EXE_mcpeval"))
        .args(["score", "--server", "sandbox", "--format", "json"])
        .args(flags)
        .arg("--")
        .arg("python3")
        .arg(fixture())
        .arg(escape)
        .arg(port.to_string())
        .env("MCPEVAL_HOME", home)
        .output()
        .unwrap();
    serde_json::from_slice(&output.stdout)
        .unwrap_or_else(|_| panic!("{}", String::from_utf8_lossy(&output.stderr)))
}

fn accepted(listener: &TcpListener) -> usize {
    std::iter::from_fn(|| listener.accept().ok()).count()
}

fn coverage_reasons(report: &Value) -> Vec<(String, String)> {
    report["readiness"]["areas"]
        .as_array()
        .unwrap()
        .iter()
        .find(|area| area["name"] == "coverage")
        .unwrap()["checks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|check| {
            (
                check["tool"].as_str().unwrap_or_default().to_owned(),
                check["reason"].as_str().unwrap().to_owned(),
            )
        })
        .collect()
}

#[test]
fn a_sandboxed_score_calls_every_tool_and_contains_writes_and_network() {
    let home = std::env::temp_dir().join(format!("mcpeval-sandbox-cli-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&home).unwrap();
    let escape = home.join("escaped");
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let port = listener.local_addr().unwrap().port();

    let plain = score(&home, &[], &escape, port);
    assert_eq!(
        coverage_reasons(&plain),
        [("save_note".to_owned(), "coverage-unannotated".to_owned())],
        "{plain}"
    );
    assert!(
        accepted(&listener) > 0,
        "the read-only dial tool did not run"
    );
    assert!(!escape.exists(), "a writer was called outside the sandbox");

    let sandboxed = score(&home, &["--sandbox"], &escape, port);
    assert_eq!(
        sandboxed["readiness"]["measurement_profile"]["sandboxed"], true,
        "{sandboxed}"
    );
    assert_eq!(
        coverage_reasons(&sandboxed),
        [
            ("wipe_target".to_owned(), "coverage-call-failed".to_owned()),
            ("dial".to_owned(), "coverage-call-failed".to_owned()),
        ],
        "{sandboxed}"
    );
    assert_eq!(
        sandboxed["readiness"]["surface"]["exercised"], 1,
        "{sandboxed}"
    );
    assert!(!escape.exists(), "the writer escaped the sandbox");
    assert_eq!(accepted(&listener), 0, "the sandbox reached the network");
    assert!(
        sandboxed["readiness"]["measurement_profile"]
            .get("sandbox_network")
            .is_none(),
        "{sandboxed}"
    );

    // Working arguments may name every tool the sandbox makes callable.
    let arguments = home.join("arguments.json");
    std::fs::write(
        &arguments,
        r#"{"save_note": {"text": "from arguments"}, "wipe_target": {}}"#,
    )
    .unwrap();
    let fed = score(
        &home,
        &["--sandbox", "--arguments", arguments.to_str().unwrap()],
        &escape,
        port,
    );
    assert!(
        fed["readiness"]["measurement_profile"]["arguments_sha256"].is_string(),
        "{fed}"
    );
    assert!(!escape.exists(), "the writer escaped the sandbox");

    // The profile records the network permission, so runs with and without
    // it are not compared.
    let networked = score(&home, &["--sandbox", "--sandbox-network"], &escape, port);
    assert_eq!(
        networked["readiness"]["measurement_profile"]["sandbox_network"], true,
        "{networked}"
    );
    assert!(
        accepted(&listener) > 0,
        "--sandbox-network kept the network out"
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
        .iter_errors(&sandboxed)
        .map(|error| format!("{error} at {}", error.instance_path()))
        .collect();
    assert!(errors.is_empty(), "{errors:#?}");
}
