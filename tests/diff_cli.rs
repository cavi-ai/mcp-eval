use std::process::Command;

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_mcpeval")
}

fn demo() -> &'static str {
    env!("CARGO_BIN_EXE_mcpeval-demo")
}

fn home() -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!("mcpeval-diff-{}", uuid::Uuid::new_v4()));
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

/// A small manifest with a pagination case, so a `--broken stalled-cursor`
/// run regresses exactly one case against the clean baseline.
fn write_manifest(dir: &std::path::Path) -> String {
    let path = dir.join("diff.manifest.json");
    std::fs::write(
        &path,
        r#"{
          "version": 1,
          "probes": [
            {"id":"discovery-budget","probe":"discovery-cost","access":"read_only","max_tools":20,"max_schema_bytes":2000},
            {"id":"token-budget","probe":"token-cost","access":"read_only","max_total_tokens":500},
            {"id":"describe_status-guessable","probe":"schema-guessability","tool":"describe_status","access":"read_only","arguments":{}},
            {"id":"catalog-pagination","probe":"pagination","access":"read_only","max_pages":5}
          ]
        }"#,
    )
    .unwrap();
    path.to_string_lossy().into_owned()
}

fn probe_json(dir: &std::path::Path, manifest: &str, broken: Option<&str>) -> Vec<u8> {
    let mut args: Vec<String> = vec![
        "probe".to_owned(),
        "--server".to_owned(),
        "demo".to_owned(),
        "--manifest".to_owned(),
        manifest.to_owned(),
        "--format".to_owned(),
        "json".to_owned(),
    ];
    args.push("--".to_owned());
    args.push(demo().to_owned());
    if let Some(aspect) = broken {
        args.push("--broken".to_owned());
        args.push(aspect.to_owned());
    }
    let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let output = run(dir, &arg_refs);
    // A failing battery still emits the JSON report and exits non-zero;
    // accept either so a regressed run can be captured as the current
    // document.
    assert!(
        output.status.success() || !output.stdout.is_empty(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    output.stdout
}

#[test]
fn diff_classifies_regression_fix_and_gate() {
    let dir = home();
    let manifest = write_manifest(&dir);
    let baseline_path = dir.join("baseline.json");
    std::fs::write(&baseline_path, probe_json(&dir, &manifest, None)).unwrap();
    let regressed_path = dir.join("regressed.json");
    std::fs::write(
        &regressed_path,
        probe_json(&dir, &manifest, Some("stalled-cursor")),
    )
    .unwrap();

    // Informational by default: text verdicts, zero exit even with a
    // regression.
    let output = run(
        &dir,
        &[
            "diff",
            baseline_path.to_str().unwrap(),
            regressed_path.to_str().unwrap(),
        ],
    );
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(
        stdout.contains("REGRESSED pagination-stalled-cursor"),
        "{stdout}"
    );
    assert!(stdout.contains("unchanged"), "{stdout}");
    assert!(stdout.contains("100 → "), "{stdout}");
    assert!(!stdout.contains("CANARY"), "{stdout}");

    // The gate flag makes the same diff exit non-zero.
    let gated = run(
        &dir,
        &[
            "diff",
            baseline_path.to_str().unwrap(),
            regressed_path.to_str().unwrap(),
            "--fail-on-regression",
        ],
    );
    assert!(!gated.status.success());
    assert!(String::from_utf8_lossy(&gated.stdout).contains("REGRESSED"));

    // The reverse direction reads as a fix: no gate.
    let repaired = run(
        &dir,
        &[
            "diff",
            regressed_path.to_str().unwrap(),
            baseline_path.to_str().unwrap(),
            "--fail-on-regression",
        ],
    );
    assert!(repaired.status.success());
    assert!(String::from_utf8_lossy(&repaired.stdout).contains("fixed"));

    // Clean run against clean baseline: unchanged, still green.
    let clean_path = dir.join("clean.json");
    std::fs::write(&clean_path, probe_json(&dir, &manifest, None)).unwrap();
    let unchanged = run(
        &dir,
        &[
            "diff",
            baseline_path.to_str().unwrap(),
            clean_path.to_str().unwrap(),
            "--fail-on-regression",
        ],
    );
    assert!(unchanged.status.success());
    assert!(String::from_utf8_lossy(&unchanged.stdout).contains("0 regressed"));
}

#[test]
fn diff_json_document_is_versioned_and_deterministic() {
    let dir = home();
    let manifest = write_manifest(&dir);
    let baseline_path = dir.join("baseline.json");
    std::fs::write(&baseline_path, probe_json(&dir, &manifest, None)).unwrap();
    let regressed_path = dir.join("regressed.json");
    std::fs::write(
        &regressed_path,
        probe_json(&dir, &manifest, Some("stalled-cursor")),
    )
    .unwrap();

    let baseline_str = baseline_path.to_str().unwrap();
    let args = |current: &str| -> Vec<&'static str> {
        let mut owned: Vec<String> = vec![
            "diff".to_owned(),
            baseline_str.to_owned(),
            current.to_owned(),
            "--format".to_owned(),
            "json".to_owned(),
        ];
        // Leak the strings: the command borrows them for its lifetime
        // only, and the test runs a fixed set of invocations.
        owned
            .drain(..)
            .map(|value| value.leak() as &'static str)
            .collect()
    };
    let first = run(&dir, &args(regressed_path.to_str().unwrap()));
    assert!(first.status.success());
    let document: serde_json::Value = serde_json::from_slice(&first.stdout).unwrap();
    assert_eq!(document["schema"], "mcpeval.probe-diff/v1");
    assert_eq!(document["readiness"]["baseline"], 100);
    assert_eq!(document["readiness"]["current"], 85);
    assert_eq!(document["summary"]["regressed"], 1);
    let regressed_case = document["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|case| case["verdict"] == "regressed")
        .unwrap();
    assert_eq!(
        regressed_case["current_reason"],
        "pagination-stalled-cursor"
    );
    assert_eq!(regressed_case["baseline_reason"], serde_json::Value::Null);

    // Deterministic: the same inputs produce byte-identical output.
    let second = run(&dir, &args(regressed_path.to_str().unwrap()));
    assert_eq!(first.stdout, second.stdout);
}

#[test]
fn diff_rejects_non_report_documents() {
    let dir = home();
    let bad = dir.join("bad.json");
    std::fs::write(&bad, r#"{"schema": "nope"}"#).unwrap();
    let manifest = write_manifest(&dir);
    let baseline_path = dir.join("baseline.json");
    std::fs::write(&baseline_path, probe_json(&dir, &manifest, None)).unwrap();

    let output = run(
        &dir,
        &[
            "diff",
            bad.to_str().unwrap(),
            baseline_path.to_str().unwrap(),
        ],
    );
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("not a usable probe report"), "{stderr}");
    assert!(stderr.contains("mcpeval.probe-report/v1"), "{stderr}");

    let missing = run(
        &dir,
        &[
            "diff",
            dir.join("absent.json").to_str().unwrap(),
            baseline_path.to_str().unwrap(),
        ],
    );
    assert!(!missing.status.success());
    let stderr = String::from_utf8_lossy(&missing.stderr);
    assert!(stderr.contains("loading baseline"), "{stderr}");
}

#[test]
fn diff_markdown_renders_the_movement_table() {
    let dir = home();
    let manifest = write_manifest(&dir);
    let baseline_path = dir.join("baseline.json");
    std::fs::write(&baseline_path, probe_json(&dir, &manifest, None)).unwrap();
    let regressed_path = dir.join("regressed.json");
    std::fs::write(
        &regressed_path,
        probe_json(&dir, &manifest, Some("stalled-cursor")),
    )
    .unwrap();

    let output = run(
        &dir,
        &[
            "diff",
            baseline_path.to_str().unwrap(),
            regressed_path.to_str().unwrap(),
            "--format",
            "markdown",
        ],
    );
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("## mcp-eval baseline diff"), "{stdout}");
    assert!(
        stdout.contains("| Case | Verdict | Baseline | Current |"),
        "{stdout}"
    );
    assert!(
        stdout.contains("**regressed** (`pagination-stalled-cursor`)"),
        "{stdout}"
    );
    assert!(!stdout.contains("CANARY"), "{stdout}");
}

#[test]
fn diff_reports_a_changed_failure_mode_and_gates_it_on_request() {
    let dir = home();
    let manifest = write_manifest(&dir);
    let baseline_path = dir.join("duplicate.json");
    std::fs::write(
        &baseline_path,
        probe_json(&dir, &manifest, Some("duplicate-page")),
    )
    .unwrap();
    let current_path = dir.join("stalled.json");
    std::fs::write(
        &current_path,
        probe_json(&dir, &manifest, Some("stalled-cursor")),
    )
    .unwrap();
    let baseline = baseline_path.to_str().unwrap();
    let current = current_path.to_str().unwrap();

    let informational = run(&dir, &["diff", baseline, current, "--fail-on-regression"]);
    assert!(informational.status.success());
    let stdout = String::from_utf8(informational.stdout).unwrap();
    assert!(
        stdout.contains("CHANGED pagination-duplicate-tool → pagination-stalled-cursor"),
        "{stdout}"
    );
    assert!(
        stdout.contains("0 regressed, 0 fixed, 1 changed"),
        "{stdout}"
    );

    let gated = run(&dir, &["diff", baseline, current, "--fail-on-change"]);
    assert!(!gated.status.success());

    let json = run(&dir, &["diff", baseline, current, "--format", "json"]);
    let document: serde_json::Value = serde_json::from_slice(&json.stdout).unwrap();
    assert_eq!(document["summary"]["changed"], 1);
    let changed = document["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|case| case["id"] == "catalog-pagination")
        .unwrap();
    assert_eq!(changed["verdict"], "changed");
    assert_eq!(changed["baseline_reason"], "pagination-duplicate-tool");
    assert_eq!(changed["current_reason"], "pagination-stalled-cursor");

    let markdown = run(&dir, &["diff", baseline, current, "--format", "markdown"]);
    let body = String::from_utf8(markdown.stdout).unwrap();
    assert!(
        body.contains("| catalog-pagination | **changed** |"),
        "{body}"
    );
}

#[test]
fn diff_refuses_reports_from_different_servers() {
    let dir = home();
    let manifest = write_manifest(&dir);
    let baseline_path = dir.join("baseline.json");
    std::fs::write(&baseline_path, probe_json(&dir, &manifest, None)).unwrap();
    let mut other: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&baseline_path).unwrap()).unwrap();
    other["server"] = "staging".into();
    let other_path = dir.join("other.json");
    std::fs::write(&other_path, serde_json::to_vec(&other).unwrap()).unwrap();

    let output = run(
        &dir,
        &[
            "diff",
            baseline_path.to_str().unwrap(),
            other_path.to_str().unwrap(),
        ],
    );
    assert_eq!(
        output.status.code(),
        Some(2),
        "a server mismatch is a usage error"
    );
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.contains("`demo`") && stderr.contains("`staging`"),
        "{stderr}"
    );
}
