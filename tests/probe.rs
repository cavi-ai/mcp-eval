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

fn token_cost_options(
    home: &TestHome,
    max_total_tokens: u64,
    max_tool_tokens: Option<u64>,
) -> ProbeOptions {
    let mut case = json!({
        "id": "token-budget",
        "probe": "token-cost",
        "access": "read_only",
        "max_total_tokens": max_total_tokens
    });
    if let Some(limit) = max_tool_tokens {
        case["max_tool_tokens"] = json!(limit);
    }
    let value = json!({"version": 1, "probes": [case]});
    let path = home.0.join("token-cost.manifest.json");
    let mut file = std::fs::File::create(&path).unwrap();
    serde_json::to_writer(&mut file, &value).unwrap();
    file.flush().unwrap();
    ProbeOptions {
        server: "fixture".into(),
        manifest_path: path,
        selected_probe: None,
        selected_case: None,
        allow_mutation: false,
        command: vec!["python3".into(), FIXTURE.into(), "clean".into()],
        http_url: None,
        allow_remote_http: false,
    }
}

#[test]
fn token_cost_passes_within_budget_and_reports_sorted_usage() {
    let home = TestHome::new();
    let mut store = Store::open(Some(home.0.clone())).unwrap();
    let report = run(token_cost_options(&home, 100_000, Some(10_000)), &mut store).unwrap();
    assert!(report.passed());
    let case = &report.cases[0];
    assert_eq!(case.probe, ProbeKind::TokenCost);
    assert_eq!(case.tool_count, Some(7));
    let usage = case.token_usage.as_ref().unwrap();
    assert!(usage.total_tokens > 0);
    assert_eq!(usage.per_tool.len(), 7);
    assert!(usage
        .per_tool
        .windows(2)
        .all(|pair| pair[0].tokens >= pair[1].tokens));
    let total: u64 = usage.per_tool.iter().map(|tool| tool.tokens).sum();
    assert_eq!(total, usage.total_tokens);
    // Token budgets measure catalog size only; nothing crosses the store boundary.
    let stored = std::fs::read_dir(home.0.join("store"))
        .map(|entries| {
            entries
                .map(|entry| std::fs::read_to_string(entry.unwrap().path()).unwrap())
                .collect::<String>()
        })
        .unwrap_or_default();
    assert!(!stored.contains("CANARY"));
}

#[test]
fn token_cost_fails_on_total_and_per_tool_budgets() {
    let home = TestHome::new();
    let mut store = Store::open(Some(home.0.clone())).unwrap();
    let report = run(token_cost_options(&home, 1, None), &mut store).unwrap();
    assert!(!report.passed());
    assert_eq!(
        report.cases[0].reason,
        Some(FailureReason::TokenBudgetExceeded)
    );
    assert_eq!(report.cases[0].first_failure, Some(1));

    let home = TestHome::new();
    let mut store = Store::open(Some(home.0.clone())).unwrap();
    let report = run(token_cost_options(&home, 100_000, Some(1)), &mut store).unwrap();
    assert!(!report.passed());
    assert_eq!(
        report.cases[0].reason,
        Some(FailureReason::TokenBudgetExceeded)
    );
}

#[test]
fn token_estimation_is_deterministic_ceil_division() {
    assert_eq!(mcpeval::probe::estimate_tokens(0), 0);
    assert_eq!(mcpeval::probe::estimate_tokens(1), 1);
    assert_eq!(mcpeval::probe::estimate_tokens(4), 1);
    assert_eq!(mcpeval::probe::estimate_tokens(5), 2);
}
