use std::process::Command;

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_mcpeval")
}

fn demo() -> &'static str {
    env!("CARGO_BIN_EXE_mcpeval-demo")
}

fn home() -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!("mcpeval-demo-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&path).unwrap();
    path
}

fn run(dir: &std::path::Path, args: &[&str]) -> std::process::Output {
    Command::new(bin())
        .args(args)
        .env("MCPEVAL_HOME", dir)
        .output()
        .unwrap()
}

#[test]
fn init_then_probe_scores_the_demo_server_green() {
    let dir = home();
    let manifest = dir.join("mcp-eval.manifest.json");
    let init = run(
        &dir,
        &[
            "init",
            "--server",
            "demo",
            "--confirm-read-only",
            "--output",
            manifest.to_str().unwrap(),
            "--",
            demo(),
        ],
    );
    assert!(
        init.status.success(),
        "{}",
        String::from_utf8_lossy(&init.stderr)
    );
    let stdout = String::from_utf8(init.stdout).unwrap();
    assert!(stdout.contains("12 tools"), "{stdout}");
    assert!(stdout.contains("schema-guessability 6"), "{stdout}");

    let probe = run(
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
    assert!(
        probe.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&probe.stdout),
        String::from_utf8_lossy(&probe.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&probe.stdout).unwrap();
    assert_eq!(report["passed"], true);
    assert_eq!(report["readiness"]["score"], 100);
    // No payloads from the demo server leak into the report.
    let text = String::from_utf8(probe.stdout).unwrap();
    assert!(!text.contains("status: ready"));
}

#[test]
fn broken_modes_fail_with_their_fixed_reasons() {
    let cases: &[(&str, &str, &str)] = &[
        (
            "schema",
            r#"{"id":"x","probe":"schema-guessability","tool":"describe_status","access":"read_only","arguments":{}}"#,
            "invalid-schema",
        ),
        (
            "fidelity",
            r#"{"id":"x","probe":"instruction-fidelity","tool":"describe_status","access":"read_only","arguments":{},"expect":{"outcome":"ok","required_result_fields":["status"],"equals":{"status":"ready"}}}"#,
            "value-mismatch",
        ),
        (
            "unstable-errors",
            r#"{"id":"x","probe":"error-honesty","tool":"flaky_read","access":"read_only","arguments":{},"max_attempts":4,"expect_retryable":true}"#,
            "unstable-error-code",
        ),
        (
            "bloated",
            r#"{"id":"x","probe":"token-cost","access":"read_only","max_total_tokens":100,"max_tool_tokens":50}"#,
            "token-budget-exceeded",
        ),
        (
            "duplicate-page",
            r#"{"id":"x","probe":"pagination","access":"read_only","max_pages":3}"#,
            "pagination-duplicate-tool",
        ),
        (
            "stalled-cursor",
            r#"{"id":"x","probe":"pagination","access":"read_only","max_pages":3}"#,
            "pagination-stalled-cursor",
        ),
        (
            "slow",
            r#"{"id":"x","probe":"latency-budget","tool":"slow_read","access":"read_only","arguments":{},"attempts":2,"max_latency_ms":50}"#,
            "latency-budget-exceeded",
        ),
    ];
    for (aspect, probe, reason) in cases {
        let dir = home();
        let manifest = dir.join("m.json");
        std::fs::write(&manifest, format!(r#"{{"version":1,"probes":[{probe}]}}"#)).unwrap();
        let output = Command::new(bin())
            .args([
                "probe",
                "--server",
                "demo",
                "--manifest",
                manifest.to_str().unwrap(),
            ])
            .args(["--", demo(), "--broken", aspect])
            .env("MCPEVAL_HOME", &dir)
            .output()
            .unwrap();
        assert!(!output.status.success(), "{aspect} should fail");
        let stdout = String::from_utf8(output.stdout).unwrap();
        assert!(
            stdout.contains(reason),
            "{aspect}: expected {reason} in {stdout}"
        );
    }
}

#[test]
fn contention_and_state_recovery_pass_against_the_demo() {
    let dir = home();
    let manifest = dir.join("m.json");
    std::fs::write(
        &manifest,
        r#"{
          "version": 1,
          "sandboxes": {"demo": {"description": "disposable demo session"}},
          "probes": [
            {"id":"parallel-read","probe":"contention","tool":"shared_read","access":"read_only","arguments":{}},
            {"id":"recover","probe":"state-recovery","access":"mutating","sandbox":"demo",
             "failure_tool":"break_session","failure_arguments":{},
             "recovery_tool":"recover_session","recovery_arguments":{},
             "validation_tool":"session_status","validation_arguments":{}}
          ]
        }"#,
    )
    .unwrap();
    let output = Command::new(bin())
        .args([
            "probe",
            "--server",
            "demo",
            "--manifest",
            manifest.to_str().unwrap(),
            "--allow-mutation",
        ])
        .args(["--", demo()])
        .env("MCPEVAL_HOME", &dir)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("parallel-read contention pass"));
    assert!(stdout.contains("recover state-recovery pass attempts=3"));
}

#[test]
fn failing_bounds_render_in_text_markdown_and_report() {
    let dir = home();
    let probe = |format: &str| {
        run(
            &dir,
            &[
                "probe",
                "--server",
                "demo",
                "--manifest",
                "tests/fixtures/mcp-eval.manifest.json",
                "--format",
                format,
                "--",
                demo(),
            ],
        )
    };

    let text = probe("text");
    assert_eq!(text.status.code(), Some(1));
    let stdout = String::from_utf8(text.stdout).unwrap();
    let line = stdout
        .lines()
        .find(|line| line.starts_with("bounded-discovery discovery-cost fail"))
        .unwrap_or_else(|| panic!("no bounded-discovery line: {stdout}"));
    assert!(
        line.ends_with(" bound=max_tools limit=10 observed=12"),
        "{line}"
    );

    let markdown = probe("markdown");
    let body = String::from_utf8(markdown.stdout).unwrap();
    assert!(
        body.contains("| Case | Probe | Result | Attempts | First failure | Reason | Bound |"),
        "{body}"
    );
    assert!(
        body.contains("| discovery-limit-exceeded | max_tools 12 > 10 |"),
        "{body}"
    );
    assert!(
        body.lines()
            .any(|line| line.contains(" | pass | ") && line.ends_with(" | — | — | — |")),
        "{body}"
    );

    let json = probe("json");
    let document = dir.join("report.json");
    std::fs::write(&document, &json.stdout).unwrap();
    let report = run(&dir, &["report", document.to_str().unwrap()]);
    let replayed = String::from_utf8(report.stdout).unwrap();
    assert!(
        replayed.contains(line),
        "report must render the probe line: {replayed}"
    );
}

#[test]
fn token_budget_failure_names_the_heaviest_tools() {
    let dir = home();
    let manifest = dir.join("tokens.manifest.json");
    std::fs::write(
        &manifest,
        r#"{"version":1,"probes":[{"id":"tiny-budget","probe":"token-cost","access":"read_only","max_total_tokens":1}]}"#,
    )
    .unwrap();
    let probe = |format: &str| {
        run(
            &dir,
            &[
                "probe",
                "--server",
                "demo",
                "--manifest",
                manifest.to_str().unwrap(),
                "--format",
                format,
                "--",
                demo(),
            ],
        )
    };
    let heaviest = |body: &str, marker: &str| -> Vec<(String, u64)> {
        let line = body
            .lines()
            .find_map(|line| line.strip_prefix(marker))
            .unwrap_or_else(|| panic!("no {marker:?} line: {body}"));
        line.split(", ")
            .map(|entry| {
                let (tool, tokens) = entry.rsplit_once(' ').unwrap();
                (tool.to_owned(), tokens.parse().unwrap())
            })
            .collect()
    };

    let text = probe("text");
    assert_eq!(text.status.code(), Some(1));
    let stdout = String::from_utf8(text.stdout).unwrap();
    assert!(
        stdout.contains(" bound=max_total_tokens limit=1 observed="),
        "{stdout}"
    );
    let tools = heaviest(&stdout, "  heaviest: ");
    assert_eq!(tools.len(), 3, "{stdout}");
    assert!(
        tools.windows(2).all(|pair| pair[0].1 >= pair[1].1),
        "{stdout}"
    );

    let body = String::from_utf8(probe("markdown").stdout).unwrap();
    assert_eq!(heaviest(&body, "  - heaviest: "), tools, "{body}");
}

#[test]
fn demo_rejects_unknown_aspects_and_prints_help() {
    let unknown = Command::new(demo())
        .args(["--broken", "nonsense"])
        .output()
        .unwrap();
    assert_eq!(unknown.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&unknown.stderr);
    assert!(
        stderr.contains("unknown aspect nonsense; expected one of: "),
        "{stderr}"
    );
    assert!(stderr.contains("stalled-cursor"), "{stderr}");

    for flag in ["--help", "-h"] {
        let help = Command::new(demo()).arg(flag).output().unwrap();
        assert_eq!(help.status.code(), Some(0), "{flag}");
        let stdout = String::from_utf8_lossy(&help.stdout);
        assert!(
            stdout.contains("usage: mcpeval-demo [--broken <aspect>]"),
            "{stdout}"
        );
        assert!(stdout.contains("output-schema"), "{stdout}");
    }
}
