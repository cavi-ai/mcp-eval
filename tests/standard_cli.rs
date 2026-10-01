use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::{json, Value};

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_mcpeval")
}

fn demo() -> &'static str {
    env!("CARGO_BIN_EXE_mcpeval-demo")
}

fn home() -> PathBuf {
    let path = std::env::temp_dir().join(format!("mcpeval-standard-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&path).unwrap();
    path
}

fn run(dir: &Path, args: &[&str]) -> (Output, Value) {
    let output = Command::new(bin())
        .args(args)
        .env("MCPEVAL_HOME", dir)
        .output()
        .unwrap();
    let document = serde_json::from_slice(&output.stdout).unwrap_or(Value::Null);
    (output, document)
}

fn score(dir: &Path, server: &[&str]) -> (Output, Value) {
    let mut args = vec!["score", "--server", "demo", "--format", "json", "--"];
    args.extend_from_slice(server);
    run(dir, &args)
}

fn area(document: &Value, name: &str) -> u64 {
    document["readiness"]["areas"]
        .as_array()
        .unwrap()
        .iter()
        .find(|area| area["name"] == name)
        .unwrap_or_else(|| panic!("no area {name}: {document:#}"))["score"]
        .as_u64()
        .unwrap()
}

fn lost(document: &Value) -> Vec<(String, Option<String>, String)> {
    document["readiness"]["areas"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|area| area["checks"].as_array().unwrap().iter())
        .map(|check| {
            (
                check["id"].as_str().unwrap().to_owned(),
                check["tool"].as_str().map(str::to_owned),
                check["reason"].as_str().unwrap().to_owned(),
            )
        })
        .collect()
}

/// Lost checks of one area only: later slices add areas whose checks
/// must not break assertions about this one.
fn lost_in(document: &Value, name: &str) -> Vec<(String, Option<String>, String)> {
    lost(document)
        .into_iter()
        .filter(|(id, _, _)| id.split('.').next() == Some(name))
        .collect()
}

fn owned(checks: &[(&str, Option<&str>, &str)]) -> Vec<(String, Option<String>, String)> {
    checks
        .iter()
        .map(|(id, tool, reason)| ((*id).into(), tool.map(str::to_owned), (*reason).into()))
        .collect()
}

#[test]
fn the_clean_demo_loses_points_only_for_its_deliberate_fixtures() {
    let (output, document) = score(&home(), &[demo()]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(document["schema"], "mcpeval.probe-report/v2");
    assert_eq!(document["gate"], Value::Null);
    let readiness = &document["readiness"];
    assert_eq!(readiness["standard"], "mcpeval-standard/1");
    assert_eq!(
        readiness["surface"],
        json!({"tools": 12, "read_only": 10, "writers": 2, "exercised": 10})
    );
    assert_eq!(area(&document, "protocol"), 100);
    assert_eq!(area(&document, "catalog"), 63);
    assert_eq!(area(&document, "context"), 100);
    assert_eq!(area(&document, "reliability"), 94);
    assert_eq!(area(&document, "coverage"), 100);
    assert_eq!(area(&document, "error-honesty"), 100);
    assert_eq!(readiness["score"], 91);
    assert_eq!(lost_in(&document, "error-honesty"), owned(&[]));
    assert_eq!(lost_in(&document, "protocol"), owned(&[]));
    // No demo tool declares outputSchema; two descriptions are under 40
    // characters; break_session omits readOnlyHint.
    let no_schema = |tool| {
        (
            "catalog.output-schema",
            Some(tool),
            "catalog-no-output-schema",
        )
    };
    let mut catalog = vec![
        (
            "catalog.description",
            Some("describe_status"),
            "catalog-short-description",
        ),
        no_schema("describe_status"),
        no_schema("read_counter"),
        no_schema("shared_read"),
        no_schema("flaky_read"),
        no_schema("slow_read"),
        (
            "catalog.description",
            Some("break_session"),
            "catalog-short-description",
        ),
        (
            "catalog.read-only-declared",
            Some("break_session"),
            "catalog-read-only-undeclared",
        ),
        no_schema("break_session"),
    ];
    catalog.extend(
        [
            "recover_session",
            "session_status",
            "report_weather",
            "sampled_read",
            "elicited_read",
            "publish_status",
        ]
        .map(no_schema),
    );
    assert_eq!(lost_in(&document, "catalog"), owned(&catalog));
    // sampled_read and elicited_read issue server-to-client requests
    // mid-call; declined, they still count as ordinary successful calls.
    assert_eq!(
        lost_in(&document, "reliability"),
        owned(&[
            (
                "reliability.consistent",
                Some("flaky_read"),
                "reliability-inconsistent"
            ),
            ("reliability.latency", Some("slow_read"), "reliability-slow"),
        ])
    );
    assert_eq!(lost_in(&document, "context"), owned(&[]));
    assert_eq!(lost_in(&document, "coverage"), owned(&[]));
}

#[test]
fn two_runs_of_one_build_score_the_same() {
    let (_, first) = score(&home(), &[demo()]);
    let (_, second) = score(&home(), &[demo()]);
    assert_eq!(first["readiness"]["score"], second["readiness"]["score"]);
    for name in [
        "protocol",
        "catalog",
        "context",
        "error-honesty",
        "reliability",
        "coverage",
    ] {
        assert_eq!(area(&first, name), area(&second, name), "{name}");
    }
    assert_eq!(lost(&first), lost(&second));
}

#[test]
fn each_new_demo_aspect_lowers_only_reliability() {
    for (aspect, reliability) in [("slow", 93), ("flaky", 90)] {
        let (output, document) = score(&home(), &[demo(), "--broken", aspect]);
        assert!(
            output.status.success(),
            "{aspect}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(area(&document, "protocol"), 100, "{aspect}");
        assert_eq!(area(&document, "error-honesty"), 100, "{aspect}");
        assert_eq!(area(&document, "catalog"), 63, "{aspect}");
        assert_eq!(area(&document, "context"), 100, "{aspect}");
        assert_eq!(area(&document, "coverage"), 100, "{aspect}");
        assert_eq!(area(&document, "reliability"), reliability, "{aspect}");
    }
}

#[test]
fn undescribed_lowers_only_the_catalog() {
    let (_, clean) = score(&home(), &[demo()]);
    let (output, document) = score(&home(), &[demo(), "--broken", "undescribed"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(area(&document, "catalog"), 35);
    for name in [
        "protocol",
        "context",
        "error-honesty",
        "reliability",
        "coverage",
    ] {
        assert_eq!(area(&document, name), area(&clean, name), "{name}");
    }
    assert!(lost_in(&document, "catalog").contains(
        &owned(&[(
            "catalog.params-described",
            Some("report_weather"),
            "catalog-undescribed-params"
        )])[0]
    ));
}

#[test]
fn paged_repeated_or_endless_catalogs_count_each_tool_once() {
    for aspect in ["duplicate-page", "stalled-cursor"] {
        let (output, document) = score(&home(), &[demo(), "--broken", aspect]);
        assert!(
            output.status.success(),
            "{aspect}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(document["readiness"]["surface"]["tools"], 12, "{aspect}");
    }
}

#[test]
fn unannotated_tools_are_called_only_when_attested() {
    let (_, document) = score(&home(), &[demo(), "--broken", "bloated"]);
    assert!(lost(&document).contains(
        &owned(&[(
            "coverage.exercised",
            Some("extra_padding_tool"),
            "coverage-unannotated"
        )])[0]
    ));
    let dir = home();
    let (_, attested) = run(
        &dir,
        &[
            "score",
            "--server",
            "demo",
            "--format",
            "json",
            "--confirm-read-only",
            "--",
            demo(),
            "--broken",
            "bloated",
        ],
    );
    assert_eq!(attested["readiness"]["attested_read_only"], true);
    // Only a called tool can be rejected: the demo answers the unknown
    // tool with -32602.
    assert!(lost(&attested).contains(
        &owned(&[(
            "coverage.exercised",
            Some("extra_padding_tool"),
            "coverage-rejected-arguments"
        )])[0]
    ));
}

#[test]
fn a_tool_that_kills_the_connection_costs_only_itself() {
    let (output, document) = run(
        &home(),
        &[
            "score",
            "--server",
            "fault",
            "--format",
            "json",
            "--confirm-read-only",
            "--",
            "python3",
            "tests/fixtures/transport_fault_server.py",
        ],
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        document["readiness"]["surface"],
        json!({"tools": 3, "read_only": 3, "writers": 0, "exercised": 2})
    );
    assert_eq!(area(&document, "coverage"), 67);
    assert_eq!(area(&document, "reliability"), 100);
    assert_eq!(
        lost_in(&document, "coverage"),
        owned(&[("coverage.exercised", Some("crash"), "coverage-call-failed")])
    );
    assert_eq!(lost_in(&document, "reliability"), owned(&[]));
}

#[test]
fn probe_reports_the_gate_and_the_standard_separately_and_journals_neither_standard_call() {
    let dir = home();
    let manifest = dir.join("m.json");
    std::fs::write(
        &manifest,
        r#"{"version":1,"probes":[{"id":"d","probe":"discovery-cost","access":"read_only","max_tools":1,"max_schema_bytes":100000}]}"#,
    )
    .unwrap();
    let (output, document) = run(
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
    assert_eq!(
        output.status.code(),
        Some(1),
        "the gate failed: 12 tools > 1"
    );
    assert_eq!(document["passed"], false);
    assert_eq!(document["gate"], json!({"passed": 0, "total": 1}));
    assert_eq!(
        document["readiness"]["score"], 91,
        "the gate does not move the standard"
    );
    let journaled = std::fs::read_dir(dir.join("store"))
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .filter(|entry| entry.file_name().to_string_lossy().starts_with("calls-"))
                .count()
        })
        .unwrap_or(0);
    assert_eq!(
        journaled, 0,
        "discovery-cost makes no calls and the standard journals none"
    );
}

#[test]
fn gate_only_and_selected_probes_skip_the_standard() {
    let dir = home();
    let manifest = dir.join("m.json");
    std::fs::write(
        &manifest,
        r#"{"version":1,"probes":[{"id":"d","probe":"discovery-cost","access":"read_only","max_tools":50,"max_schema_bytes":100000}]}"#,
    )
    .unwrap();
    let base = [
        "probe",
        "--server",
        "demo",
        "--manifest",
        manifest.to_str().unwrap(),
        "--format",
        "json",
    ];
    for extra in [&["--gate-only"][..], &["--probe", "discovery-cost"][..]] {
        let mut args: Vec<&str> = base.to_vec();
        args.extend_from_slice(extra);
        args.extend_from_slice(&["--", demo()]);
        let (output, document) = run(&dir, &args);
        assert!(output.status.success(), "{extra:?}");
        assert_eq!(document["readiness"], Value::Null, "{extra:?}");
        assert_eq!(
            document["gate"],
            json!({"passed": 1, "total": 1}),
            "{extra:?}"
        );
    }
}

#[test]
fn text_output_names_every_lost_point_with_its_hint() {
    let output = Command::new(bin())
        .args(["score", "--server", "demo", "--", demo()])
        .env("MCPEVAL_HOME", home())
        .output()
        .unwrap();
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(
        stdout.contains("demo readiness 91/100 protocol=100 catalog=63 context=100 error-honesty=100 reliability=94 coverage=100 standard=mcpeval-standard/1"),
        "{stdout}"
    );
    assert!(
        stdout.contains("  surface: 12 tools, 10 read-only, 2 writers, 10 exercised"),
        "{stdout}"
    );
    assert!(
        stdout.contains(
            "  lost reliability.consistent score=0 tool=flaky_read reason=reliability-inconsistent"
        ),
        "{stdout}"
    );
    assert!(stdout.contains("    hint: "), "{stdout}");
    assert!(
        !stdout.contains(" gate "),
        "score runs no manifest: {stdout}"
    );
    // Twelve tools lose catalog.output-schema; its hint prints once.
    assert_eq!(
        stdout
            .matches("lost catalog.output-schema score=0 tool=")
            .count(),
        12,
        "{stdout}"
    );
    assert_eq!(
        stdout
            .matches("hint: declare outputSchema and return structuredContent")
            .count(),
        1,
        "{stdout}"
    );
}

#[test]
fn markdown_lost_points_give_each_hint_once() {
    let output = Command::new(bin())
        .args([
            "score",
            "--server",
            "demo",
            "--format",
            "markdown",
            "--",
            demo(),
        ])
        .env("MCPEVAL_HOME", home())
        .output()
        .unwrap();
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("### Lost points"), "{stdout}");
    assert_eq!(
        stdout.matches("- **`catalog.output-schema`** `").count(),
        12,
        "{stdout}"
    );
    assert_eq!(
        stdout
            .matches("declare outputSchema and return structuredContent")
            .count(),
        1,
        "{stdout}"
    );
}

#[test]
fn committed_v1_baselines_render_and_diff_as_not_comparable() {
    let dir = home();
    let baseline = dir.join("baseline.json");
    std::fs::write(
        &baseline,
        serde_json::to_vec(&json!({
            "schema": "mcpeval.probe-report/v1", "server": "demo", "passed": true,
            "readiness": {"score": 100, "categories": [], "badge": "https://img.shields.io/badge/mcpeval-100%2F100-brightgreen"},
            "cases": []
        }))
        .unwrap(),
    )
    .unwrap();
    let (_, current) = score(&dir, &[demo()]);
    let current_path = dir.join("current.json");
    std::fs::write(&current_path, serde_json::to_vec(&current).unwrap()).unwrap();

    let report = Command::new(bin())
        .args(["report"])
        .arg(&baseline)
        .env("MCPEVAL_HOME", &dir)
        .output()
        .unwrap();
    assert!(report.status.success());
    assert!(String::from_utf8_lossy(&report.stdout)
        .contains("manifest pass rate 100/100 (legacy v1 score"));

    let (output, document) = run(
        &dir,
        &[
            "diff",
            baseline.to_str().unwrap(),
            current_path.to_str().unwrap(),
            "--format",
            "json",
            "--fail-on-regression",
        ],
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(document["schema"], "mcpeval.probe-diff/v2");
    assert_eq!(document["readiness"]["comparable"], false);
    assert_eq!(document["readiness"]["baseline"], 100);
    assert_eq!(document["readiness"]["current"], 91);
}

#[test]
fn report_rerenders_a_v2_document_with_its_lost_points() {
    let dir = home();
    let (_, document) = score(&dir, &[demo()]);
    let path = dir.join("r.json");
    std::fs::write(&path, serde_json::to_vec(&document).unwrap()).unwrap();
    let output = Command::new(bin())
        .args(["report"])
        .arg(&path)
        .env("MCPEVAL_HOME", &dir)
        .output()
        .unwrap();
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("demo readiness 91/100"), "{stdout}");
    assert!(
        stdout.contains("lost reliability.latency score=50 tool=slow_read"),
        "{stdout}"
    );
}

#[test]
fn a_failed_later_tools_page_ends_the_listing_not_the_battery() {
    for mode in ["page-error", "invalid"] {
        let (output, document) = run(
            &home(),
            &[
                "score",
                "--server",
                "paged",
                "--format",
                "json",
                "--",
                "python3",
                "tests/fixtures/probe_paged_server.py",
                mode,
            ],
        );
        assert!(
            output.status.success(),
            "{mode}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            document["readiness"]["surface"]["tools"], 3,
            "{mode}: the first page's tools are kept"
        );
        assert!(
            lost_in(&document, "protocol").contains(
                &owned(&[("protocol.pagination", None, "protocol-pagination-invalid")])[0]
            ),
            "{mode}: {document:#}"
        );
    }
}

/// Runs one manifest case under --gate-only and returns its text line.
fn gate_case(case: &str, server: &[&str]) -> String {
    let dir = home();
    let manifest = dir.join("m.json");
    std::fs::write(&manifest, format!(r#"{{"version":1,"probes":[{case}]}}"#)).unwrap();
    let output = Command::new(bin())
        .args(["probe", "--server", "s", "--gate-only", "--manifest"])
        .arg(&manifest)
        .arg("--")
        .args(server)
        .env("MCPEVAL_HOME", &dir)
        .output()
        .unwrap();
    String::from_utf8(output.stdout).unwrap()
}

#[test]
fn server_cases_decline_requests_the_tool_sends_mid_call() {
    // sampled_read issues sampling/createMessage before it answers.
    let payload = gate_case(
        r#"{"id":"big","probe":"payload-bounds","tool":"sampled_read","access":"read_only","arguments":{},"field":"note","size_bytes":5000,"expect_handled":true}"#,
        &[demo()],
    );
    assert!(payload.contains("big payload-bounds pass"), "{payload}");
    let contention = gate_case(
        r#"{"id":"both","probe":"contention","tool":"sampled_read","access":"read_only","arguments":{}}"#,
        &[demo()],
    );
    assert!(contention.contains("both contention pass"), "{contention}");
}

#[test]
fn contention_finds_a_tool_listed_on_a_later_page() {
    // Every page-one tool fails, so contention runs on gamma_reset, which
    // only the second page lists.
    let (output, document) = run(
        &home(),
        &[
            "score",
            "--server",
            "paged",
            "--format",
            "json",
            "--confirm-read-only",
            "--",
            "python3",
            "tests/fixtures/probe_paged_server.py",
            "late",
        ],
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(document["readiness"]["surface"]["tools"], 5);
    assert!(
        !lost_in(&document, "reliability")
            .iter()
            .any(|(id, _, _)| id == "reliability.contention"),
        "{document:#}"
    );
}

#[test]
fn unknown_method_lowers_only_the_protocol_area() {
    let (_, clean) = score(&home(), &[demo()]);
    let (output, document) = score(&home(), &[demo(), "--broken", "unknown-method"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(area(&document, "protocol"), 83);
    assert_eq!(
        lost_in(&document, "protocol"),
        owned(&[(
            "protocol.unknown-method",
            None,
            "protocol-unknown-method-answered"
        )])
    );
    for name in [
        "catalog",
        "context",
        "error-honesty",
        "reliability",
        "coverage",
    ] {
        assert_eq!(area(&document, name), area(&clean, name), "{name}");
    }
}

#[test]
fn an_older_protocol_server_passes_the_protocol_area() {
    let (output, document) = run(
        &home(),
        &[
            "score",
            "--server",
            "old",
            "--format",
            "json",
            "--",
            "python3",
            "tests/fixtures/old_protocol_server.py",
        ],
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(area(&document, "protocol"), 100, "{document:#}");
}

#[test]
fn lying_errors_lowers_only_error_honesty() {
    let (_, clean) = score(&home(), &[demo()]);
    let (output, document) = score(&home(), &[demo(), "--broken", "lying-errors"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(area(&document, "error-honesty"), 0);
    assert_eq!(
        lost_in(&document, "error-honesty"),
        owned(&[
            (
                "error-honesty.invalid-arguments",
                Some("shared_read"),
                "honesty-accepted-invalid"
            ),
            (
                "error-honesty.invalid-arguments",
                Some("report_weather"),
                "honesty-accepted-invalid"
            ),
        ])
    );
    for name in ["protocol", "catalog", "context", "reliability", "coverage"] {
        assert_eq!(area(&document, name), area(&clean, name), "{name}");
    }
}

#[test]
fn a_refusal_counts_even_when_the_server_lacks_ping() {
    let (output, document) = run(
        &home(),
        &[
            "score",
            "--server",
            "no-ping",
            "--format",
            "json",
            "--",
            "python3",
            "tests/fixtures/no_ping_server.py",
        ],
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(area(&document, "error-honesty"), 100, "{document:#}");
    assert!(lost_in(&document, "protocol")
        .contains(&owned(&[("protocol.ping", None, "protocol-ping-failed")])[0]));
}

#[test]
fn skipping_a_tool_never_raises_the_score() {
    let (_, clean) = score(&home(), &[demo()]);
    let (output, skipped) = run(
        &home(),
        &[
            "score",
            "--server",
            "demo",
            "--format",
            "json",
            "--skip-tool",
            "slow_read",
            "--",
            demo(),
        ],
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(lost(&skipped)
        .contains(&owned(&[("coverage.exercised", Some("slow_read"), "coverage-skipped")])[0]));
    assert!(
        skipped["readiness"]["score"].as_u64() <= clean["readiness"]["score"].as_u64(),
        "skipping the slow tool raised the score: {skipped:#}"
    );
    assert!(lost(&skipped)
        .contains(&owned(&[("reliability.skipped", Some("slow_read"), "coverage-skipped")])[0]));
    let (output, _) = run(
        &home(),
        &[
            "score",
            "--server",
            "demo",
            "--skip-tool",
            "no_such_tool",
            "--",
            demo(),
        ],
    );
    assert_eq!(output.status.code(), Some(2));
}

#[test]
fn synthesized_arguments_reach_required_argument_tools() {
    let (output, document) = run(
        &home(),
        &[
            "score",
            "--server",
            "args",
            "--format",
            "json",
            "--",
            "python3",
            "tests/fixtures/required_args_server.py",
        ],
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(document["readiness"]["surface"]["exercised"], 4);
    assert_eq!(area(&document, "coverage"), 80);
    assert_eq!(area(&document, "error-honesty"), 100);
    assert_eq!(
        lost_in(&document, "coverage"),
        owned(&[(
            "coverage.exercised",
            Some("patterned"),
            "coverage-unsynthesizable"
        )])
    );
}

#[test]
fn score_places_readiness_only_among_a_corpus_of_its_own_standard() {
    let dir = home();
    let (_, document) = score(&dir, &[demo()]);
    let standard = document["readiness"]["standard"]
        .as_str()
        .unwrap()
        .to_owned();
    let corpus = |standard: &str| {
        format!(
            r#"{{"schema":"mcpeval.readiness-corpus/v2","source":"test","standard":"{standard}",
                "observations":[{{"server":"a","score":40}},{{"server":"b","score":100}}]}}"#
        )
    };
    let text = |dir: &Path| {
        let output = Command::new(bin())
            .args(["score", "--server", "demo", "--", demo()])
            .env("MCPEVAL_HOME", dir)
            .output()
            .unwrap();
        String::from_utf8(output.stdout).unwrap()
    };
    std::fs::write(dir.join("corpus.json"), corpus(&standard)).unwrap();
    let stdout = text(&dir);
    assert!(
        stdout.contains(&format!(
            "\n  standard corpus ({standard}): above 1, tied 0, below 1 of 2 observed servers\n"
        )),
        "{stdout}"
    );
    std::fs::write(dir.join("corpus.json"), corpus("mcpeval-standard/0")).unwrap();
    assert!(!text(&dir).contains("standard corpus"));
}
