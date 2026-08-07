use std::io::Write;

use mcpeval::manifest::ProbeKind;
use mcpeval::probe::{run, FailureReason, ProbeOptions};
use mcpeval::store::Store;
use serde_json::json;

const FIXTURE: &str = "tests/fixtures/probe_clean_server.py";

struct TestHome(std::path::PathBuf);

impl TestHome {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("mcpeval-probe-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}

impl Drop for TestHome {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn write_manifest(home: &TestHome, mutating: bool) -> std::path::PathBuf {
    let access = if mutating { "mutating" } else { "read_only" };
    let sandbox = if mutating {
        json!({"sandbox": "fixture"})
    } else {
        json!({})
    };
    let mut degradation = json!({
        "id": "repeat-read",
        "probe": "degradation-over-n",
        "tool": "read_counter",
        "access": access,
        "arguments": {"secret": "CANARY-argument"},
        "max_attempts": 5
    });
    degradation
        .as_object_mut()
        .unwrap()
        .extend(sandbox.as_object().unwrap().clone());
    let value = json!({
        "version": 1,
        "sandboxes": {"fixture": {"description": "disposable"}},
        "probes": [
            degradation,
            {
                "id": "literal-status",
                "probe": "instruction-fidelity",
                "tool": "describe_status",
                "access": "read_only",
                "arguments": {},
                "expect": {"outcome": "ok", "required_result_fields": ["status"], "equals": {"status": "ready"}}
            }
        ]
    });
    let path = home.0.join("mcp-eval.manifest.json");
    let mut file = std::fs::File::create(&path).unwrap();
    serde_json::to_writer(&mut file, &value).unwrap();
    file.flush().unwrap();
    path
}

fn options(home: &TestHome, mode: &str, selected_probe: Option<ProbeKind>) -> ProbeOptions {
    ProbeOptions {
        server: "fixture".into(),
        manifest_path: write_manifest(home, false),
        selected_probe,
        selected_case: None,
        allow_mutation: false,
        command: vec!["python3".into(), FIXTURE.into(), mode.into()],
        http_url: None,
        allow_remote_http: false,
    }
}

#[test]
fn authorization_fails_before_spawning_a_mutating_case() {
    let home = TestHome::new();
    let mut opts = options(&home, "clean", Some(ProbeKind::DegradationOverN));
    opts.manifest_path = write_manifest(&home, true);
    opts.command = vec!["definitely-not-a-real-command".into()];
    let mut store = Store::open(Some(home.0.clone())).unwrap();
    let error = run(opts, &mut store).unwrap_err().to_string();
    assert!(error.contains("--allow-mutation"));
    assert!(!error.contains("spawning"));
}

#[test]
fn clean_degradation_reaches_the_declared_bound() {
    let home = TestHome::new();
    let mut store = Store::open(Some(home.0.clone())).unwrap();
    let report = run(
        options(&home, "clean", Some(ProbeKind::DegradationOverN)),
        &mut store,
    )
    .unwrap();
    assert!(report.passed());
    assert_eq!(report.cases[0].attempts, 5);
    assert_eq!(report.cases[0].first_failure, None);
}

#[test]
fn broken_degradation_reports_only_the_first_failure_index() {
    let home = TestHome::new();
    let mut store = Store::open(Some(home.0.clone())).unwrap();
    let report = run(
        options(&home, "broken", Some(ProbeKind::DegradationOverN)),
        &mut store,
    )
    .unwrap();
    assert!(!report.passed());
    assert_eq!(report.cases[0].attempts, 3);
    assert_eq!(report.cases[0].first_failure, Some(3));
    assert_eq!(
        report.cases[0].reason,
        Some(FailureReason::UnexpectedOutcome)
    );
}

#[test]
fn fidelity_matches_structured_content_and_uses_fixed_mismatch_reasons() {
    let clean_home = TestHome::new();
    let mut clean_store = Store::open(Some(clean_home.0.clone())).unwrap();
    let clean = run(
        options(&clean_home, "clean", Some(ProbeKind::InstructionFidelity)),
        &mut clean_store,
    )
    .unwrap();
    assert!(clean.passed());

    let broken_home = TestHome::new();
    let mut broken_store = Store::open(Some(broken_home.0.clone())).unwrap();
    let broken = run(
        options(&broken_home, "broken", Some(ProbeKind::InstructionFidelity)),
        &mut broken_store,
    )
    .unwrap();
    assert_eq!(broken.cases[0].reason, Some(FailureReason::ValueMismatch));
    let debug = format!("{broken:?}");
    assert!(!debug.contains("wrong"));
    assert!(!debug.contains("CANARY"));
}
