use std::process::Command;

use serde_json::Value;

const CLEAN: &str = "tests/fixtures/probe_clean_server.py";

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_mcpeval")
}

fn demo() -> &'static str {
    env!("CARGO_BIN_EXE_mcpeval-demo")
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
    assert!(
        stdout.contains(
            "(7 tools, 4 cases: discovery-cost 1, token-cost 1, pagination 1, protocol-negotiation 1)"
        ),
        "{stdout}"
    );

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
    assert_eq!(
        kinds,
        [
            "discovery-cost",
            "token-cost",
            "pagination",
            "protocol-negotiation"
        ]
    );

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

/// Runs `init` with `args`, recording every `tools/call` the fixture server
/// receives. Returns the output and the called tool names, or `None` when
/// no call was made.
fn init(
    home: &std::path::Path,
    args: &[&str],
    server: &[&str],
) -> (std::process::Output, Option<Vec<String>>) {
    let call_log = home.join(format!("{}.calls", uuid::Uuid::new_v4()));
    let output = Command::new(bin())
        .args(["init", "--server", "fixture"])
        .args(args)
        .arg("--")
        .args(server)
        .env("MCPEVAL_HOME", home)
        .env("PROBE_FIXTURE_CALL_LOG", &call_log)
        .output()
        .unwrap();
    let calls = std::fs::read_to_string(&call_log)
        .ok()
        .map(|log| log.lines().map(str::to_owned).collect());
    (output, calls)
}

fn init_manifest(home: &std::path::Path, args: &[&str], server: &[&str]) -> (String, Value) {
    let manifest = home.join(format!("{}.manifest.json", uuid::Uuid::new_v4()));
    let mut args = args.to_vec();
    args.extend(["--output", manifest.to_str().unwrap()]);
    let (output, _) = init(home, &args, server);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout)
        .unwrap()
        .replace(manifest.to_str().unwrap(), "<manifest>");
    let value = serde_json::from_slice(&std::fs::read(&manifest).unwrap()).unwrap();
    (stdout, value)
}

fn init_read_only(home: &std::path::Path, server: &[&str]) -> (String, Value) {
    init_manifest(home, &["--confirm-read-only"], server)
}

fn case_ids(manifest: &Value) -> Vec<(String, String)> {
    manifest["probes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|case| {
            (
                case["id"].as_str().unwrap().to_owned(),
                case["probe"].as_str().unwrap().to_owned(),
            )
        })
        .collect()
}

fn case<'a>(manifest: &'a Value, id: &str) -> &'a Value {
    manifest["probes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|case| case["id"] == id)
        .unwrap_or_else(|| panic!("{id} missing"))
}

fn expected(
    fixed: &[(&str, &str)],
    tools: &[&str],
    tail: &[(&str, &str)],
) -> Vec<(String, String)> {
    let mut ids: Vec<(String, String)> = fixed
        .iter()
        .map(|(id, kind)| ((*id).to_owned(), (*kind).to_owned()))
        .collect();
    for tool in tools {
        for (suffix, kind) in [
            ("guessable", "schema-guessability"),
            ("repeat", "degradation-over-n"),
            ("latency", "latency-budget"),
        ] {
            ids.push((format!("{tool}-{suffix}"), kind.to_owned()));
        }
    }
    ids.extend(
        tail.iter()
            .map(|(id, kind)| ((*id).to_owned(), (*kind).to_owned())),
    );
    ids
}

/// The demo tools that answer a naive `{}` call and are not annotated as
/// writers (`break_session` is destructive, `recover_session` is not
/// read-only).
const DEMO_VERIFIED: [&str; 6] = [
    "describe_status",
    "read_counter",
    "shared_read",
    "slow_read",
    "session_status",
    "report_weather",
];

fn demo_battery() -> Vec<(String, String)> {
    expected(
        &[
            ("discovery-budget", "discovery-cost"),
            ("token-budget", "token-cost"),
            ("catalog-pagination", "pagination"),
            ("protocol-negotiation", "protocol-negotiation"),
            ("declared-surfaces", "surface-listing"),
        ],
        &DEMO_VERIFIED,
        &[
            ("describe_status-contention", "contention"),
            ("report_weather-payload", "payload-bounds"),
        ],
    )
}

#[test]
fn init_scaffolds_every_input_free_probe_kind_on_the_demo() {
    let home = home();
    let (stdout, manifest) = init_read_only(&home, &[demo()]);
    assert_eq!(
        stdout,
        "wrote <manifest> (12 tools, 25 cases: discovery-cost 1, token-cost 1, pagination 1, \
         protocol-negotiation 1, surface-listing 1, schema-guessability 6, degradation-over-n 6, \
         latency-budget 6, contention 1, payload-bounds 1) (2 tools skipped by annotations)\n\
         next: review budgets; add error-honesty, state-recovery, and instruction-fidelity cases \
         (they need expected inputs) and cancellation, sampling, elicitation, \
         resource-subscription, and completion cases where the server supports them, then run \
         mcpeval probe\n"
    );
    let verified = DEMO_VERIFIED;
    assert_eq!(case_ids(&manifest), demo_battery());
    assert_eq!(case(&manifest, "catalog-pagination")["max_pages"], 5);
    assert_eq!(case(&manifest, "declared-surfaces")["max_pages"], 5);
    assert_eq!(
        case(&manifest, "protocol-negotiation")["bogus_version"],
        "2000-01-01"
    );
    for tool in verified {
        assert_eq!(
            case(&manifest, &format!("{tool}-repeat"))["max_attempts"],
            5
        );
        let latency = case(&manifest, &format!("{tool}-latency"));
        assert_eq!(latency["attempts"], 3);
        let bound = latency["max_latency_ms"].as_u64().unwrap();
        assert!(
            (1000..=60_000).contains(&bound) && bound.is_multiple_of(100),
            "{tool}: {bound}"
        );
    }
    // slow_read sleeps 400 ms, so its bound is four times a measured call.
    assert!(
        case(&manifest, "slow_read-latency")["max_latency_ms"]
            .as_u64()
            .unwrap()
            >= 1600
    );
    assert_eq!(
        case(&manifest, "describe_status-contention")["tool"],
        "describe_status"
    );
    let payload = case(&manifest, "report_weather-payload");
    assert_eq!(payload["tool"], "report_weather");
    assert_eq!(payload["field"], "city");
    assert_eq!(payload["size_bytes"], 1_000_000);
    assert_eq!(payload["expect_handled"], false);
    assert_eq!(payload["arguments"], serde_json::json!({}));

    // A declared outputSchema adds an output-schema case right after the
    // tool's latency case.
    let (_, declared) = init_read_only(&home, &[demo(), "--broken", "output-schema"]);
    let ids = case_ids(&declared);
    let latency = ids
        .iter()
        .position(|(id, _)| id == "report_weather-latency")
        .unwrap();
    assert_eq!(
        ids[latency + 1],
        (
            "report_weather-output".to_owned(),
            "output-schema".to_owned()
        )
    );
    assert_eq!(
        ids.iter()
            .filter(|(_, kind)| kind == "output-schema")
            .count(),
        1
    );
}

#[test]
fn init_omits_surface_listing_when_neither_resources_nor_prompts_are_declared() {
    let home = home();
    let (stdout, manifest) = init_read_only(&home, &["python3", CLEAN]);
    assert!(
        stdout.starts_with(
            "wrote <manifest> (7 tools, 21 cases: discovery-cost 1, token-cost 1, pagination 1, \
             protocol-negotiation 1, schema-guessability 5, degradation-over-n 5, \
             latency-budget 5, contention 1, payload-bounds 1)\n"
        ),
        "{stdout}"
    );
    assert_eq!(
        case_ids(&manifest),
        expected(
            &[
                ("discovery-budget", "discovery-cost"),
                ("token-budget", "token-cost"),
                ("catalog-pagination", "pagination"),
                ("protocol-negotiation", "protocol-negotiation"),
            ],
            &[
                "read_counter",
                "describe_status",
                "recover_session",
                "session_status",
                "shared_read",
            ],
            &[
                ("read_counter-contention", "contention"),
                ("read_counter-payload", "payload-bounds"),
            ],
        )
    );
    // No verified tool declares a string property: the representative tool
    // receives the payload under a fixed field name.
    assert_eq!(case(&manifest, "read_counter-payload")["field"], "payload");

    let manifest_path = home.join("fixture.manifest.json");
    std::fs::write(&manifest_path, manifest.to_string()).unwrap();
    let probe = probe_json(&home, &manifest_path, &[], &["python3", CLEAN]);
    assert_eq!(probe.0, Some(0), "{}", probe.1);
    assert_eq!(probe.1["passed"], true);
}

fn probe_json(
    home: &std::path::Path,
    manifest: &std::path::Path,
    extra: &[&str],
    server: &[&str],
) -> (Option<i32>, Value) {
    let output = Command::new(bin())
        .args(["probe", "--server", "fixture", "--format", "json"])
        .arg("--manifest")
        .arg(manifest)
        .args(extra)
        .arg("--")
        .args(server)
        .env("MCPEVAL_HOME", home)
        .output()
        .unwrap();
    let report = serde_json::from_slice(&output.stdout).unwrap_or_else(|_| {
        panic!(
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    });
    (output.status.code(), report)
}

fn reason<'a>(report: &'a Value, id: &str) -> &'a str {
    report["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|case| case["id"] == id)
        .and_then(|case| case["reason"].as_str())
        .unwrap_or_else(|| panic!("{id} has no reason in {report}"))
}

#[test]
fn the_quickstart_init_manifest_fails_each_broken_demo_with_its_reason() {
    let home = home();
    let (_, manifest) = init_read_only(&home, &[demo()]);
    let path = home.join("demo.manifest.json");
    std::fs::write(&path, manifest.to_string()).unwrap();

    let (code, clean) = probe_json(&home, &path, &[], &[demo()]);
    assert_eq!(code, Some(0), "{clean}");
    assert_eq!(clean["readiness"]["score"], 100, "{clean}");

    let (code, stalled) = probe_json(&home, &path, &[], &[demo(), "--broken", "stalled-cursor"]);
    assert_eq!(code, Some(1), "{stalled}");
    assert_eq!(
        reason(&stalled, "catalog-pagination"),
        "pagination-stalled-cursor"
    );

    let (code, negotiation) = probe_json(&home, &path, &[], &[demo(), "--broken", "negotiation"]);
    assert_eq!(code, Some(1), "{negotiation}");
    assert!(reason(&negotiation, "protocol-negotiation").starts_with("negotiation-"));

    let (code, bloated) = probe_json(&home, &path, &[], &[demo(), "--broken", "bloated"]);
    assert_eq!(code, Some(1), "{bloated}");
    assert!(bloated["cases"]
        .as_array()
        .unwrap()
        .iter()
        .any(|case| matches!(
            case["reason"].as_str(),
            Some("discovery-limit-exceeded" | "token-budget-exceeded")
        )));

    // Only the latency-budget cases: the full 25-case manifest would call
    // slow_read 9 times at 2 s apiece (schema-guessability once,
    // degradation-over-n five times, latency-budget three times).
    let (code, slow) = probe_json(
        &home,
        &path,
        &["--probe", "latency-budget"],
        &[demo(), "--broken", "slow"],
    );
    assert_eq!(code, Some(1), "{slow}");
    assert_eq!(
        reason(&slow, "slow_read-latency"),
        "latency-budget-exceeded"
    );
}

const ANNOTATED: [&str; 3] = ["python3", CLEAN, "annotated"];

#[test]
fn init_scaffolds_read_only_annotated_tools_without_attestation() {
    let home = home();
    let (stdout, manifest) = init_manifest(&home, &[], &[demo()]);
    assert!(
        stdout.starts_with(
            "wrote <manifest> (12 tools, 25 cases: discovery-cost 1, token-cost 1, pagination 1, \
             protocol-negotiation 1, surface-listing 1, schema-guessability 6, \
             degradation-over-n 6, latency-budget 6, contention 1, payload-bounds 1) \
             (2 tools skipped by annotations)\n"
        ),
        "{stdout}"
    );
    assert_eq!(case_ids(&manifest), demo_battery());
}

#[test]
fn init_never_calls_annotated_writers_and_needs_attestation_for_unannotated_tools() {
    let home = home();
    let manifest = home.join("annotated.manifest.json");
    let output_arg = manifest.to_str().unwrap();

    let (output, calls) = init(&home, &["--output", output_arg], &ANNOTATED);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(calls.unwrap(), ["read_counter"]);
    let value: Value = serde_json::from_slice(&std::fs::read(&manifest).unwrap()).unwrap();
    let fixed = [
        ("discovery-budget", "discovery-cost"),
        ("token-budget", "token-cost"),
        ("catalog-pagination", "pagination"),
        ("protocol-negotiation", "protocol-negotiation"),
    ];
    assert_eq!(
        case_ids(&value),
        expected(
            &fixed,
            &["read_counter"],
            &[
                ("read_counter-contention", "contention"),
                ("read_counter-payload", "payload-bounds"),
            ],
        )
    );

    let (output, calls) = init(
        &home,
        &["--confirm-read-only", "--force", "--output", output_arg],
        &ANNOTATED,
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8(output.stdout)
        .unwrap()
        .contains(") (2 tools skipped by annotations)\n"));
    assert_eq!(
        calls.unwrap(),
        [
            "read_counter",
            "describe_status",
            "flaky_read",
            "session_status",
            "shared_read"
        ]
    );
    let value: Value = serde_json::from_slice(&std::fs::read(&manifest).unwrap()).unwrap();
    assert_eq!(
        case_ids(&value),
        expected(
            &fixed,
            &[
                "read_counter",
                "describe_status",
                "session_status",
                "shared_read"
            ],
            &[
                ("read_counter-contention", "contention"),
                ("read_counter-payload", "payload-bounds"),
            ],
        )
    );
}

#[test]
fn init_dry_run_lists_decisions_without_calling_or_writing() {
    let home = home();
    let manifest = home.join("dry.manifest.json");
    let output_arg = manifest.to_str().unwrap();
    let dry_run = |args: &[&str]| -> String {
        let mut args = args.to_vec();
        args.extend(["--dry-run", "--output", output_arg]);
        let (output, calls) = init(&home, &args, &ANNOTATED);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(calls, None);
        String::from_utf8(output.stdout).unwrap()
    };

    assert_eq!(
        dry_run(&[]),
        "read_counter  candidate (readOnlyHint)\n\
         describe_status  needs --confirm-read-only\n\
         flaky_read  needs --confirm-read-only\n\
         break_session  skipped: destructiveHint\n\
         recover_session  skipped: readOnlyHint=false\n\
         session_status  needs --confirm-read-only\n\
         shared_read  needs --confirm-read-only\n\
         lookup  skipped: required arguments\n\
         dry run: no tool was called and nothing was written\n"
    );
    assert!(!manifest.exists());

    std::fs::write(&manifest, "sentinel").unwrap();
    assert_eq!(
        dry_run(&["--confirm-read-only"]),
        "read_counter  candidate (readOnlyHint)\n\
         describe_status  candidate (attested)\n\
         flaky_read  candidate (attested)\n\
         break_session  skipped: destructiveHint\n\
         recover_session  skipped: readOnlyHint=false\n\
         session_status  candidate (attested)\n\
         shared_read  candidate (attested)\n\
         lookup  skipped: required arguments\n\
         dry run: no tool was called and nothing was written\n"
    );
    // A selected tool that needs attestation is a usage error even under
    // --dry-run: dry-run previews what a real run would do, and a real run
    // would refuse this the same way.
    let (output, calls) = init(
        &home,
        &[
            "--tool",
            "describe_status",
            "--dry-run",
            "--output",
            output_arg,
        ],
        &ANNOTATED,
    );
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(calls, None);
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.contains("--tool describe_status")
            && stderr.contains(
                "it carries no readOnlyHint annotation; pass --confirm-read-only to attest \
                 it is read-only"
            ),
        "{stderr}"
    );
    assert_eq!(std::fs::read_to_string(&manifest).unwrap(), "sentinel");
}

#[test]
fn init_tool_restricts_candidates_and_rejects_unknown_or_annotated_writers() {
    let home = home();
    let (stdout, manifest) = init_manifest(
        &home,
        &["--confirm-read-only", "--tool", "describe_status"],
        &[demo()],
    );
    assert!(
        stdout.starts_with(
            "wrote <manifest> (12 tools, 10 cases: discovery-cost 1, token-cost 1, pagination 1, \
             protocol-negotiation 1, surface-listing 1, schema-guessability 1, \
             degradation-over-n 1, latency-budget 1, contention 1, payload-bounds 1)\n"
        ),
        "{stdout}"
    );
    assert_eq!(
        case_ids(&manifest),
        expected(
            &[
                ("discovery-budget", "discovery-cost"),
                ("token-budget", "token-cost"),
                ("catalog-pagination", "pagination"),
                ("protocol-negotiation", "protocol-negotiation"),
                ("declared-surfaces", "surface-listing"),
            ],
            &["describe_status"],
            &[
                ("describe_status-contention", "contention"),
                ("describe_status-payload", "payload-bounds"),
            ],
        )
    );

    let rejected = home.join("rejected.manifest.json");
    for (tool, reason) in [
        ("nope", "not in the server's tool catalog"),
        ("break_session", "destructiveHint"),
        ("recover_session", "readOnlyHint=false"),
    ] {
        let (output, _) = init(
            &home,
            &[
                "--confirm-read-only",
                "--tool",
                tool,
                "--output",
                rejected.to_str().unwrap(),
            ],
            &[demo()],
        );
        assert_eq!(output.status.code(), Some(2), "{tool}");
        let stderr = String::from_utf8(output.stderr).unwrap();
        assert!(
            stderr.contains(&format!("--tool {tool}")) && stderr.contains(reason),
            "{stderr}"
        );
        assert!(!rejected.exists());
    }

    let rejected2 = home.join("rejected2.manifest.json");
    for (args, tool, reason) in [
        (
            vec!["--confirm-read-only", "--tool", "lookup"],
            "lookup",
            "it declares required arguments, so init cannot call it with {}",
        ),
        (
            vec!["--tool", "describe_status"],
            "describe_status",
            "it carries no readOnlyHint annotation; pass --confirm-read-only to attest it is read-only",
        ),
    ] {
        let mut args = args;
        args.extend(["--output", rejected2.to_str().unwrap()]);
        let (output, _) = init(&home, &args, &ANNOTATED);
        assert_eq!(output.status.code(), Some(2), "{tool}");
        let stderr = String::from_utf8(output.stderr).unwrap();
        assert!(
            stderr.contains(&format!("--tool {tool}")) && stderr.contains(reason),
            "{stderr}"
        );
        assert!(!rejected2.exists());
    }
}
