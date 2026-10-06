use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use chrono::{TimeZone, Utc};
use mcpeval::generate;
use mcpeval::index;
use mcpeval::promote::{promote, PromotionConfig};
use mcpeval::record::{CallRecord, ErrorInfo};
use mcpeval::store::Store;
use serde_json::json;

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_mcpeval")
}

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("mcpeval-generate-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&path).unwrap();
        Self { path }
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

fn call(session: &str, tool: &str, arguments: serde_json::Value, retryable: bool) -> CallRecord {
    CallRecord {
        identity: None,
        ts: "2026-08-05T12:00:00Z".into(),
        session: session.into(),
        seq: 1,
        server: "synthetic-canary-server".into(),
        method: "tools/call".into(),
        tool: Some(tool.into()),
        args: Some(arguments),
        latency_ms: Some(5),
        outcome: "error".into(),
        error: Some(ErrorInfo {
            code: Some(json!("blocked")),
            layer: None,
            retryable: Some(retryable),
            kind: None,
            template: Some("synthetic-canary-error".into()),
            template_id: Some("aaaaaaaaaaaaaaaa".into()),
        }),
        shim_self_us: 1,
        kind: "real".into(),
    }
}

fn promoted_finding(root: &Path, arguments: serde_json::Value) -> String {
    promoted_finding_for_tool(root, "read_status", arguments)
}

fn promoted_finding_for_tool(root: &Path, tool: &str, arguments: serde_json::Value) -> String {
    promoted_finding_with(root, tool, arguments, false)
}

fn promoted_finding_with(
    root: &Path,
    tool: &str,
    arguments: serde_json::Value,
    retryable: bool,
) -> String {
    let mut store = Store::open(Some(root.to_owned())).unwrap();
    store
        .append(&call("first-session", tool, arguments.clone(), retryable))
        .unwrap();
    store
        .append(&call("second-session", tool, arguments, retryable))
        .unwrap();
    index::build(root).unwrap();
    promote(
        root,
        PromotionConfig {
            threshold: 0.0,
            now: Utc.with_ymd_and_hms(2026, 8, 6, 0, 0, 0).unwrap(),
        },
    )
    .unwrap();

    rusqlite::Connection::open(root.join("index.db"))
        .unwrap()
        .query_row("SELECT finding_id FROM findings", [], |row| row.get(0))
        .unwrap()
}

fn set_issue(root: &Path, finding_id: &str, column: &str, value: &dyn rusqlite::ToSql) {
    rusqlite::Connection::open(root.join("index.db"))
        .unwrap()
        .execute(
            &format!("UPDATE issues SET {column}=?2 WHERE finding_id=?1"),
            rusqlite::params![finding_id, value],
        )
        .unwrap();
}

fn generated_probe(output: &Path) -> serde_json::Value {
    let manifest: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(output).unwrap()).unwrap();
    manifest["probes"][0].clone()
}

fn generate_cli(
    root: &Path,
    finding_id: &str,
    output: &Path,
    force: bool,
    confirm_read_only: bool,
) -> Output {
    let mut command = Command::new(bin());
    command
        .args([
            "generate",
            "--finding",
            finding_id,
            "--output",
            output.to_str().unwrap(),
        ])
        .env("MCPEVAL_HOME", root);
    if force {
        command.arg("--force");
    }
    if confirm_read_only {
        command.arg("--confirm-read-only");
    }
    command.output().unwrap()
}

#[test]
fn semantic_findings_require_an_oracle_before_writing_even_with_force() {
    for class in ["false-success", "blocked-optimal-path"] {
        let root = TempDir::new();
        let finding_id = promoted_finding(&root.path, json!({}));
        set_issue(&root.path, &finding_id, "class", &class);
        let output = root.path.join("generated.json");
        std::fs::write(&output, "keep existing manifest").unwrap();

        let denied = generate_cli(&root.path, &finding_id, &output, true, true);

        assert!(
            !denied.status.success(),
            "{class} must not become repeated success"
        );
        assert!(String::from_utf8_lossy(&denied.stderr).contains("--expect"));
        assert_eq!(
            std::fs::read_to_string(&output).unwrap(),
            "keep existing manifest"
        );
    }
}

#[test]
fn unstable_codes_generate_an_error_honesty_case_with_recorded_retryability() {
    for retryable in [false, true] {
        let root = TempDir::new();
        let finding_id = promoted_finding_with(&root.path, "read_status", json!({}), retryable);
        set_issue(&root.path, &finding_id, "class", &"unstable-error-code");
        let output = root.path.join("generated.json");

        let result = generate_cli(&root.path, &finding_id, &output, false, true);

        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        let probe = generated_probe(&output);
        assert_eq!(probe["probe"], "error-honesty");
        assert_eq!(probe["expect_retryable"], retryable);
        assert_eq!(probe["max_attempts"], 3);
        mcpeval::manifest::Manifest::load(&output).unwrap();
    }
}

#[test]
fn unstable_codes_without_retryability_do_not_invent_an_error_contract() {
    let root = TempDir::new();
    let finding_id = promoted_finding(&root.path, json!({}));
    set_issue(&root.path, &finding_id, "class", &"unstable-error-code");
    set_issue(&root.path, &finding_id, "retryable", &rusqlite::types::Null);
    let output = root.path.join("generated.json");

    let result = generate_cli(&root.path, &finding_id, &output, false, true);

    assert!(!result.status.success());
    assert!(!output.exists());
    assert!(String::from_utf8_lossy(&result.stderr).contains("--expect"));
}

#[test]
fn rare_unstable_errors_respect_the_error_honesty_attempt_limit() {
    let root = TempDir::new();
    let finding = promoted_finding_with(&root.path, "flaky_read", json!({}), true);
    set_issue(&root.path, &finding, "class", &"unstable-error-code");
    set_issue(&root.path, &finding, "rate", &0.001);
    let output = root.path.join("generated.json");
    let result = generate_cli(&root.path, &finding, &output, false, true);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(generated_probe(&output)["max_attempts"], 20);
    mcpeval::manifest::Manifest::load(&output).unwrap();
}

fn generate_with_expectation(root: &Path, finding: &str, output: &Path, expect: &Path) -> Output {
    Command::new(bin())
        .args([
            "generate",
            "--finding",
            finding,
            "--output",
            output.to_str().unwrap(),
            "--confirm-read-only",
            "--expect",
            expect.to_str().unwrap(),
        ])
        .env("MCPEVAL_HOME", root)
        .output()
        .unwrap()
}

#[test]
fn false_success_oracle_detects_wrong_content_and_verifies_the_repair() {
    let root = TempDir::new();
    let finding_id = promoted_finding_for_tool(&root.path, "describe_status", json!({}));
    set_issue(&root.path, &finding_id, "class", &"false-success");
    let output = root.path.join("generated.json");
    let expect = root.path.join("expect.json");
    std::fs::write(&expect, r#"{"outcome":"ok","equals":{"status":"ready"}}"#).unwrap();

    let generated = generate_with_expectation(&root.path, &finding_id, &output, &expect);
    assert!(
        generated.status.success(),
        "{}",
        String::from_utf8_lossy(&generated.stderr)
    );
    assert_eq!(generated_probe(&output)["probe"], "instruction-fidelity");
    let fixture =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/probe_clean_server.py");
    for (mode, passed) in [("broken", false), ("clean", true)] {
        let result = Command::new(bin())
            .args([
                "verify",
                "--finding",
                &finding_id,
                "--case",
                &finding_id,
                "--manifest",
                output.to_str().unwrap(),
                "--",
                "python3",
                fixture.to_str().unwrap(),
                mode,
            ])
            .env("MCPEVAL_HOME", &root.path)
            .output()
            .unwrap();
        assert_eq!(
            result.status.code(),
            Some(if passed { 0 } else { 1 }),
            "{mode}: {} {}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
        if !passed {
            assert!(String::from_utf8_lossy(&result.stdout).contains("value-mismatch"));
        }
    }
}

#[test]
fn false_success_rejects_a_success_only_oracle() {
    let root = TempDir::new();
    let finding_id = promoted_finding(&root.path, json!({}));
    set_issue(&root.path, &finding_id, "class", &"false-success");
    let output = root.path.join("generated.json");
    let expect = root.path.join("expect.json");
    std::fs::write(&expect, r#"{"outcome":"ok"}"#).unwrap();

    let result = generate_with_expectation(&root.path, &finding_id, &output, &expect);

    assert!(!result.status.success());
    assert!(!output.exists());
    assert!(String::from_utf8_lossy(&result.stderr).contains("result assertion"));
}

#[test]
fn verification_refuses_false_success_cases_without_an_oracle_before_launch() {
    let root = TempDir::new();
    let finding = promoted_finding_for_tool(&root.path, "read_status", json!({}));
    set_issue(&root.path, &finding, "class", &"false-success");
    let manifest = root.path.join("verify.json");
    let log = root.path.join("calls.jsonl");
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/workflow_server.py");
    for case in [
        json!({"id":"check","probe":"latency-budget","access":"read_only",
            "tool":"read_status","arguments":{},"attempts":3,"max_latency_ms":10000}),
        json!({"id":"check","probe":"instruction-fidelity","access":"read_only",
            "tool":"read_status","arguments":{},"expect":{"outcome":"ok"}}),
        json!({"id":"check","probe":"workflow","access":"read_only","repetitions":1,
        "steps":[
            {"tool":"read_status","arguments":{},"expect":{"outcome":"ok"}},
            {"tool":"read_other","arguments":{},"expect":{"outcome":"ok"}}
        ]}),
    ] {
        mcpeval::manifest::Manifest::parse(
            json!({"version":1,"probes":[case.clone()]})
                .to_string()
                .as_bytes(),
        )
        .unwrap()
        .validate()
        .unwrap();
        std::fs::write(&manifest, json!({"version":1,"probes":[case]}).to_string()).unwrap();
        let result = Command::new(bin())
            .args([
                "verify",
                "--finding",
                &finding,
                "--case",
                "check",
                "--manifest",
            ])
            .arg(&manifest)
            .args(["--", "python3", fixture.to_str().unwrap(), "clean"])
            .env("MCPEVAL_HOME", &root.path)
            .env("WORKFLOW_CALL_LOG", &log)
            .output()
            .unwrap();
        assert_eq!(
            result.status.code(),
            Some(2),
            "{} {}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
        assert!(
            String::from_utf8_lossy(&result.stderr)
                .contains("result assertion or an expected error"),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert!(!log.exists(), "rejected verification launched the server");
    }
    let db = rusqlite::Connection::open(root.path.join("lifecycle.db")).unwrap();
    let state: (String, i64, Option<String>) = db
        .query_row(
            "SELECT state,consecutive_passes,probe_id FROM finding_lifecycle WHERE finding_id=?1",
            [&finding],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(state, ("open".into(), 0, None));
    let history: i64 = db
        .query_row("SELECT COUNT(*) FROM probe_history", [], |row| row.get(0))
        .unwrap();
    assert_eq!(history, 0);
}

#[test]
fn false_success_workflow_can_validate_with_a_later_step_oracle() {
    let root = TempDir::new();
    let finding = promoted_finding_for_tool(&root.path, "read_status", json!({}));
    set_issue(&root.path, &finding, "class", &"false-success");
    let manifest = root.path.join("verify.json");
    std::fs::write(
        &manifest,
        json!({"version":1,"probes":[
            {"id":"check","probe":"workflow","access":"read_only","repetitions":1,"steps":[
                {"tool":"read_status","arguments":{},"expect":{"outcome":"ok"}},
                {"tool":"read_other","arguments":{},"expect":{"outcome":"ok",
                    "equals_paths":{"/structuredContent/status":"ready"}}}
            ]}
        ]})
        .to_string(),
    )
    .unwrap();
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/workflow_server.py");
    for (mode, code, state, passes) in [
        ("poison", 1, "fix-claimed", 0),
        ("clean", 0, "verifying", 1),
    ] {
        let result = Command::new(bin())
            .args([
                "verify",
                "--finding",
                &finding,
                "--case",
                "check",
                "--manifest",
            ])
            .arg(&manifest)
            .args(["--", "python3", fixture.to_str().unwrap(), mode])
            .env("MCPEVAL_HOME", &root.path)
            .output()
            .unwrap();
        assert_eq!(
            result.status.code(),
            Some(code),
            "{} {}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
        let actual: (String, i64) = rusqlite::Connection::open(root.path.join("lifecycle.db"))
            .unwrap()
            .query_row(
                "SELECT state,consecutive_passes FROM finding_lifecycle WHERE finding_id=?1",
                [&finding],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(actual, (state.into(), passes));
    }
}

#[test]
fn generated_false_success_can_use_only_a_nested_result_oracle() {
    let root = TempDir::new();
    let mut store = Store::open(Some(root.path.clone())).unwrap();
    for session in ["first", "second"] {
        let mut record = call(session, "describe_status", json!({}), false);
        record.outcome = "ok".into();
        record.error = None;
        let identity = mcpeval::record::EventIdentity::new(uuid::Uuid::new_v4());
        record.identity = Some(identity.clone());
        store.append(&record).unwrap();
        store
            .append_annotation(&mcpeval::record::AnnotationRecord {
                ts: record.ts,
                event_id: Some(identity.event_id),
                session: None,
                seq: None,
                kind: "false-success".into(),
                note: "observed incorrect status".into(),
            })
            .unwrap();
    }
    index::build(&root.path).unwrap();
    let stats = promote(
        &root.path,
        PromotionConfig {
            threshold: 0.0,
            now: Utc.with_ymd_and_hms(2026, 8, 6, 0, 0, 0).unwrap(),
        },
    )
    .unwrap();
    assert_eq!(stats.findings, 1);
    let finding: String = rusqlite::Connection::open(root.path.join("index.db"))
        .unwrap()
        .query_row("SELECT finding_id FROM findings", [], |row| row.get(0))
        .unwrap();
    let output = root.path.join("generated.json");
    let expect = root.path.join("expect.json");
    std::fs::write(
        &expect,
        r#"{"outcome":"ok","equals_paths":{"/structuredContent/status":"ready"}}"#,
    )
    .unwrap();
    let generated = generate_with_expectation(&root.path, &finding, &output, &expect);
    assert!(
        generated.status.success(),
        "{}",
        String::from_utf8_lossy(&generated.stderr)
    );
    assert_eq!(
        generated_probe(&output)["expect"]["equals_paths"]["/structuredContent/status"],
        "ready"
    );
    let fixture =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/probe_clean_server.py");
    for (mode, code) in [("broken", 1), ("clean", 0)] {
        let result = Command::new(bin())
            .args([
                "verify",
                "--finding",
                &finding,
                "--case",
                &finding,
                "--manifest",
                output.to_str().unwrap(),
                "--",
                "python3",
                fixture.to_str().unwrap(),
                mode,
            ])
            .env("MCPEVAL_HOME", &root.path)
            .output()
            .unwrap();
        assert_eq!(
            result.status.code(),
            Some(code),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
    }
}

#[test]
fn generated_error_honesty_detects_unstable_codes_and_accepts_stable_recovery() {
    let root = TempDir::new();
    let finding = promoted_finding_with(&root.path, "flaky_read", json!({}), true);
    set_issue(&root.path, &finding, "class", &"unstable-error-code");
    let output = root.path.join("generated.json");
    assert!(generate_cli(&root.path, &finding, &output, false, true)
        .status
        .success());

    for (broken, passed) in [(true, false), (false, true)] {
        let mut command = Command::new(bin());
        command.args([
            "verify",
            "--finding",
            &finding,
            "--case",
            &finding,
            "--manifest",
            output.to_str().unwrap(),
            "--",
            env!("CARGO_BIN_EXE_mcpeval-demo"),
        ]);
        if broken {
            command.args(["--broken", "unstable-errors"]);
        }
        let result = command.env("MCPEVAL_HOME", &root.path).output().unwrap();
        assert_eq!(
            result.status.code(),
            Some(if passed { 0 } else { 1 }),
            "{} {}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
        if broken {
            assert!(String::from_utf8_lossy(&result.stdout).contains("unstable-error-code"));
        }
    }
}

#[test]
fn explicit_error_oracle_can_replace_false_success_without_inventing_retryability() {
    let root = TempDir::new();
    let finding = promoted_finding_for_tool(&root.path, "break_session", json!({}));
    set_issue(&root.path, &finding, "class", &"false-success");
    let output = root.path.join("generated.json");
    let expect = root.path.join("expect.json");
    std::fs::write(&expect, r#"{"outcome":"error","error_code":-32002}"#).unwrap();
    assert!(
        generate_with_expectation(&root.path, &finding, &output, &expect)
            .status
            .success()
    );
    let fixture =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/probe_clean_server.py");
    let result = Command::new(bin())
        .args([
            "verify",
            "--finding",
            &finding,
            "--case",
            &finding,
            "--manifest",
            output.to_str().unwrap(),
            "--",
            "python3",
            fixture.to_str().unwrap(),
        ])
        .env("MCPEVAL_HOME", &root.path)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(generated_probe(&output)["expect"]["error_code"], -32002);
}

#[test]
fn invalid_or_oversized_oracles_do_not_write_or_echo_content_or_paths() {
    let root = TempDir::new();
    let finding = promoted_finding(&root.path, json!({}));
    let output = root.path.join("generated.json");
    let expect = root.path.join("private-canary-path.json");
    for body in [
        r#"{"outcome":"private-canary-value"}"#.to_owned(),
        r#"{"outcome":"ok","unknown":"private-canary-value"}"#.to_owned(),
        r#"{"outcome":"ok","equals":{"status":"private-canary-value/path"}}"#.to_owned(),
        "private-canary-value".repeat(4000),
    ] {
        std::fs::write(&expect, body).unwrap();
        let result = generate_with_expectation(&root.path, &finding, &output, &expect);
        assert!(!result.status.success());
        assert!(!output.exists());
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
        assert!(!text.contains("private-canary"));
        assert!(!text.contains(root.path.to_str().unwrap()));
    }
}

#[test]
fn generates_a_deterministic_read_only_manifest_for_empty_arguments() {
    let root = TempDir::new();
    let finding_id = promoted_finding(&root.path, json!({}));
    let output = root.path.join("generated.json");

    let generated = generate::run(&root.path, &finding_id, &output, false, true).unwrap();
    let body = std::fs::read_to_string(&output).unwrap();
    let expected = format!(
        "{{\n  \"version\": 1,\n  \"sandboxes\": {{}},\n  \"probes\": [\n    {{\n      \"probe\": \"degradation-over-n\",\n      \"id\": \"{finding_id}\",\n      \"tool\": \"read_status\",\n      \"access\": \"read_only\",\n      \"sandbox\": null,\n      \"arguments\": {{}},\n      \"max_attempts\": 3\n    }}\n  ]\n}}\n"
    );

    assert_eq!(generated.probe_id, finding_id);
    assert!(generated.placeholders.is_empty());
    assert_eq!(body, expected);
    assert!(!body.contains("synthetic-canary"));
    mcpeval::manifest::Manifest::load(&output).unwrap();
}

#[test]
fn creates_new_files_and_replaces_existing_files_only_when_forced() {
    let root = TempDir::new();
    let finding_id = promoted_finding(&root.path, json!({}));
    let output = root.path.join("generated.json");
    std::fs::write(&output, "existing manifest\n").unwrap();

    assert!(generate::run(&root.path, &finding_id, &output, false, true).is_err());
    assert_eq!(
        std::fs::read_to_string(&output).unwrap(),
        "existing manifest\n"
    );

    generate::run(&root.path, &finding_id, &output, true, true).unwrap();
    assert_ne!(
        std::fs::read_to_string(&output).unwrap(),
        "existing manifest\n"
    );
}

#[test]
fn rejects_promoted_findings_without_a_tool() {
    let root = TempDir::new();
    let finding_id = promoted_finding(&root.path, json!({}));
    rusqlite::Connection::open(root.path.join("index.db"))
        .unwrap()
        .execute(
            "UPDATE issues SET tool=NULL WHERE finding_id=?1",
            [&finding_id],
        )
        .unwrap();

    assert!(generate::run(
        &root.path,
        &finding_id,
        &root.path.join("generated.json"),
        false,
        true,
    )
    .is_err());
}

#[test]
fn non_empty_shaped_arguments_become_placeholders_without_leaking_captured_values() {
    let root = TempDir::new();
    let finding_id = promoted_finding(&root.path, json!({"target": "str<32"}));
    let output = root.path.join("generated.json");

    let generated = generate::run(&root.path, &finding_id, &output, false, true).unwrap();

    assert_eq!(generated_probe(&output)["arguments"], json!({"target": ""}));
    assert_eq!(
        generated.placeholders,
        [("target".to_owned(), "str<32".to_owned())]
    );
    assert!(!std::fs::read_to_string(&output)
        .unwrap()
        .contains("synthetic-canary"));
}

#[test]
fn a_finding_without_recorded_arguments_generates_empty_arguments() {
    let root = TempDir::new();
    let finding_id = promoted_finding(&root.path, json!({"target": "str<32"}));
    set_issue(&root.path, &finding_id, "args", &rusqlite::types::Null);
    let output = root.path.join("generated.json");

    generate::run(&root.path, &finding_id, &output, false, true).unwrap();

    assert_eq!(generated_probe(&output)["arguments"], json!({}));
}

#[test]
fn an_intermittent_failure_gets_enough_degradation_attempts_to_observe_it() {
    for (rate, attempts) in [(0.17, 17), (0.5, 5), (0.89, 3), (1.0, 3)] {
        let root = TempDir::new();
        let finding_id = promoted_finding_with(&root.path, "read_status", json!({}), true);
        set_issue(&root.path, &finding_id, "rate", &rate);
        let output = root.path.join("generated.json");

        let generated = generate::run(&root.path, &finding_id, &output, false, true).unwrap();

        let probe = generated_probe(&output);
        assert_eq!(
            (&probe["probe"], &probe["max_attempts"]),
            (&json!("degradation-over-n"), &json!(attempts)),
            "rate {rate}"
        );
        assert_eq!(
            generated.summary(Path::new("generated.json")),
            format!("{finding_id}\nprobe=degradation-over-n max_attempts={attempts}\n")
        );
        assert_ne!(probe["probe"], json!("error-honesty"), "rate {rate}");
    }
}

#[test]
fn generate_cli_builds_an_argument_skeleton_and_lists_each_placeholder() {
    let root = TempDir::new();
    let finding_id = promoted_finding(
        &root.path,
        json!({
            "city": "str<8",
            "n": "num:3",
            "kind": "enum:fast",
            "id": "uuid",
            "tags": {"array": 1, "items": "str<32"}
        }),
    );

    let result = Command::new(bin())
        .args([
            "generate",
            "--finding",
            &finding_id,
            "--output",
            "generated.json",
            "--confirm-read-only",
        ])
        .current_dir(&root.path)
        .env("MCPEVAL_HOME", &root.path)
        .output()
        .unwrap();

    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(
        String::from_utf8(result.stdout).unwrap(),
        format!(
            "{finding_id}\nprobe=degradation-over-n max_attempts=3\n\
             fill: city (str<8)\nfill: id (uuid)\nfill: tags ({{\"array\":1,\"items\":\"str<32\"}})\n\
             fill the placeholders in generated.json before mcpeval verify\n"
        )
    );
    assert_eq!(
        generated_probe(&root.path.join("generated.json"))["arguments"],
        json!({
            "city": "",
            "n": 3,
            "kind": "fast",
            "id": "00000000-0000-0000-0000-000000000000",
            "tags": []
        })
    );
}

#[test]
fn generate_cli_prints_only_the_generated_probe_for_an_eligible_finding() {
    let root = TempDir::new();
    let finding_id = promoted_finding(&root.path, json!({}));
    let output = root.path.join("generated.json");

    let result = generate_cli(&root.path, &finding_id, &output, false, true);

    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(
        String::from_utf8(result.stdout).unwrap(),
        format!("{finding_id}\nprobe=degradation-over-n max_attempts=3\n")
    );
    assert!(result.stderr.is_empty());
    mcpeval::manifest::Manifest::load(&output).unwrap();
}

#[test]
fn generate_cli_writes_the_requested_manifest_without_creating_a_store_directory() {
    let root = TempDir::new();
    let finding_id = promoted_finding(&root.path, json!({}));
    std::fs::remove_dir_all(root.path.join("store")).unwrap();
    let output = root.path.join("generated.json");

    let result = generate_cli(&root.path, &finding_id, &output, false, true);

    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(output.is_file());
    assert!(!root.path.join("store").exists());
}

#[test]
fn generate_cli_rejects_unshaped_arguments_without_canaries_or_absolute_paths() {
    let root = TempDir::new();
    let finding_id = promoted_finding(&root.path, json!({"target": "str<32"}));
    set_issue(
        &root.path,
        &finding_id,
        "args",
        &r#"{"target":"synthetic-canary-value"}"#,
    );
    let output = root.path.join("generated.json");

    let result = generate_cli(&root.path, &finding_id, &output, false, true);
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );

    assert!(!result.status.success());
    assert!(text.contains("finding arguments are not a recorded argument shape"));
    assert!(!output.exists());
    assert!(!text.contains("synthetic-canary"));
    assert!(!text.contains(root.path.to_str().unwrap()));
}

#[test]
fn generate_cli_requires_force_to_replace_an_existing_output() {
    let root = TempDir::new();
    let finding_id = promoted_finding(&root.path, json!({}));
    let output = root.path.join("generated.json");
    std::fs::write(&output, "existing manifest\n").unwrap();

    let denied = generate_cli(&root.path, &finding_id, &output, false, true);

    assert!(!denied.status.success());
    assert_eq!(
        std::fs::read_to_string(&output).unwrap(),
        "existing manifest\n"
    );

    let forced = generate_cli(&root.path, &finding_id, &output, true, true);

    assert!(
        forced.status.success(),
        "{}",
        String::from_utf8_lossy(&forced.stderr)
    );
    assert_eq!(
        String::from_utf8(forced.stdout).unwrap(),
        format!("{finding_id}\nprobe=degradation-over-n max_attempts=3\n")
    );
}

#[test]
fn mutation_named_zero_argument_tool_requires_read_only_confirmation() {
    let root = TempDir::new();
    let finding_id = promoted_finding_for_tool(&root.path, "delete_all_records", json!({}));
    let output = root.path.join("generated.json");

    let error = generate::run(&root.path, &finding_id, &output, false, false)
        .unwrap_err()
        .to_string();

    assert!(error.contains("--confirm-read-only"));
    assert!(!output.exists());

    generate::run(&root.path, &finding_id, &output, false, true).unwrap();
    let manifest = mcpeval::manifest::Manifest::load(&output).unwrap();
    assert_eq!(
        manifest.probes[0].access(),
        mcpeval::manifest::Access::ReadOnly
    );
    assert_eq!(manifest.probes[0].sandbox(), None);
}

#[test]
fn missing_index_is_not_created() {
    let root = TempDir::new();
    let index = root.path.join("index.db");

    assert!(generate::run(
        &root.path,
        "finding-0123456789abcdef",
        &root.path.join("generated.json"),
        false,
        true,
    )
    .is_err());
    assert!(!index.exists());
}

#[test]
fn generate_cli_requires_read_only_confirmation() {
    let root = TempDir::new();
    let finding_id = promoted_finding_for_tool(&root.path, "delete_all_records", json!({}));
    let output = root.path.join("generated.json");

    let result = generate_cli(&root.path, &finding_id, &output, false, false);

    assert!(!result.status.success());
    assert!(!output.exists());
}
