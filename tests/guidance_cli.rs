use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Output, Stdio};

use serde_json::{json, Value};

const FIXTURE: &str = "tests/fixtures/guidance_server.py";

fn run(mode: &str, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_mcpeval"))
        .args([
            "guidance",
            "--server",
            "fixture",
            "--format",
            "json",
            "--settle-ms",
            "0",
        ])
        .args(args)
        .args(["--", "python3", FIXTURE, mode])
        .output()
        .unwrap()
}

fn report(output: &Output) -> Value {
    let value: Value = serde_json::from_slice(&output.stdout).unwrap_or_else(|_| {
        panic!(
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    });
    let schema: Value =
        serde_json::from_str(include_str!("../docs/mcp-eval.guidance-report.schema.json")).unwrap();
    let validator = jsonschema::validator_for(&schema).unwrap();
    let errors: Vec<_> = validator
        .iter_errors(&value)
        .map(|error| error.to_string())
        .collect();
    assert!(errors.is_empty(), "{errors:?}");
    value
}

#[test]
fn capability_dependent_guidance_matches_each_fresh_session() {
    let output = run("clean", &["--require-tool", "read_status"]);
    assert_eq!(output.status.code(), Some(0));
    let value = report(&output);
    assert_eq!(value["schema"], "mcpeval.guidance-report/v1");
    assert_eq!(value["generator"]["name"], "mcpeval");
    assert_eq!(value["generator"]["version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(value["complete"], true);
    assert_eq!(value["passed"], true);
    assert_eq!(value["candidates"], json!([]));
    assert_eq!(value["profiles"][0]["tools"], json!(["read_status"]));
    assert_eq!(
        value["profiles"][1]["tools"],
        json!(["read_status", "roots_tool"])
    );
    assert_eq!(
        value["profiles"][2]["tools"],
        json!(["read_status", "sample_tool"])
    );
    assert_eq!(
        value["profiles"][3]["tools"],
        json!(["elicit_tool", "read_status"])
    );
    assert_eq!(
        value["profiles"][4]["tools"],
        json!(["elicit_tool", "read_status", "roots_tool", "sample_tool"])
    );
    assert_eq!(value["defects"], json!([]));
    assert!(!String::from_utf8_lossy(&output.stdout).contains("CANARY"));
    assert!(!String::from_utf8_lossy(&output.stderr).contains("CANARY"));
}

#[test]
fn static_guidance_referencing_gated_tools_needs_review_without_claiming_a_defect() {
    let output = run("broken", &[]);
    assert_eq!(output.status.code(), Some(0));
    let value = report(&output);
    assert_eq!(value["passed"], true);
    assert_eq!(value["defects"], json!([]));
    assert_eq!(value["candidates"].as_array().unwrap().len(), 9);
    assert!(value["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .any(|candidate| {
            candidate["profile"] == "none"
                && candidate["tool"] == "sample_tool"
                && candidate["reason"] == "referenced-tool-unavailable"
                && candidate["available_in"] == json!(["sampling", "all"])
        }));
    assert!(!String::from_utf8_lossy(&output.stdout).contains("CANARY"));
}

#[test]
fn explicit_tool_expectation_fails_only_in_profiles_where_it_is_missing() {
    let output = run("clean", &["--require-tool", "sample_tool"]);
    assert_eq!(output.status.code(), Some(1));
    let value = report(&output);
    assert_eq!(value["passed"], false);
    assert_eq!(value["defects"].as_array().unwrap().len(), 3);
    let selected = run(
        "clean",
        &["--profile", "sampling", "--require-tool", "sample_tool"],
    );
    assert_eq!(selected.status.code(), Some(0));
    assert_eq!(report(&selected)["profiles"].as_array().unwrap().len(), 1);
}

#[test]
fn partial_or_invalid_catalogs_never_become_missing_tool_verdicts() {
    for mode in ["late-error", "duplicate", "endless"] {
        let output = run(
            mode,
            &["--profile", "none", "--require-tool", "sample_tool"],
        );
        assert_eq!(output.status.code(), Some(3), "{mode}");
        let value = report(&output);
        assert_eq!(value["complete"], false);
        assert_eq!(value["passed"], Value::Null);
        assert_eq!(value["defects"], json!([]));
        assert!(!String::from_utf8_lossy(&output.stdout).contains("CANARY"));
        assert!(!String::from_utf8_lossy(&output.stderr).contains("CANARY"));
    }
}

struct HttpFixture(Child);
impl Drop for HttpFixture {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn http_discovery_uses_separate_capability_sessions_and_emits_only_safe_metadata() {
    http_discovery("broken", 9);
    http_discovery("roots-request", 0);
}

fn http_discovery(mode: &str, candidates: usize) {
    let mut server = HttpFixture(
        Command::new("python3")
            .args([FIXTURE, mode, "--http"])
            .stdout(Stdio::piped())
            .spawn()
            .unwrap(),
    );
    let mut endpoint = String::new();
    BufReader::new(server.0.stdout.take().unwrap())
        .read_line(&mut endpoint)
        .unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_mcpeval"))
        .args([
            "guidance",
            "--server",
            "fixture",
            "--format",
            "json",
            "--settle-ms",
            "0",
            "--url",
            endpoint.trim(),
        ])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0));
    let value = report(&output);
    assert_eq!(value["profiles"].as_array().unwrap().len(), 5);
    assert_eq!(value["candidates"].as_array().unwrap().len(), candidates);
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(!text.contains("CANARY"));
    assert!(!text.contains(endpoint.trim()));
}

#[test]
fn roots_discovery_answers_empty_roots_and_bounds_server_requests() {
    let output = run(
        "roots-request",
        &["--profile", "roots", "--require-tool", "roots_tool"],
    );
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(
        report(&output)["profiles"][0]["tools"],
        json!(["read_status", "roots_tool"])
    );
    let flooded = run("roots-flood", &["--profile", "roots"]);
    assert_eq!(flooded.status.code(), Some(3));
    assert_eq!(report(&flooded)["passed"], Value::Null);
}

#[test]
fn reviewed_settling_window_includes_delayed_capability_registration() {
    let output = Command::new(env!("CARGO_BIN_EXE_mcpeval"))
        .args([
            "guidance",
            "--server",
            "fixture",
            "--format",
            "json",
            "--profile",
            "sampling",
            "--settle-ms",
            "120",
            "--require-tool",
            "sample_tool",
        ])
        .args(["--", "python3", FIXTURE, "delayed"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(report(&output)["candidates"], json!([]));
}

#[test]
fn invalid_options_are_usage_errors_before_discovery() {
    for args in [
        vec!["--server", "fixture", "--url", "https://remote.example/mcp"],
        vec![
            "--server",
            "fixture",
            "--settle-ms",
            "5001",
            "--url",
            "http://127.0.0.1:9/mcp",
        ],
        vec![
            "--server",
            "fixture",
            "--require-tool",
            "private/path",
            "--url",
            "http://127.0.0.1:9/mcp",
        ],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_mcpeval"))
            .arg("guidance")
            .args(args)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(2));
        assert!(output.stdout.is_empty());
        assert!(!String::from_utf8_lossy(&output.stderr).contains("remote.example"));
    }
}
