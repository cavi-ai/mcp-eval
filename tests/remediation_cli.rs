use std::process::Command;

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_mcpeval")
}

fn demo() -> &'static str {
    env!("CARGO_BIN_EXE_mcpeval-demo")
}

fn home() -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!("mcpeval-remediation-{}", uuid::Uuid::new_v4()));
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
fn explain_lists_reasons_and_prints_the_matching_hint() {
    let dir = home();
    let list = run(&dir, &["explain"]);
    assert!(list.status.success());
    let stdout = String::from_utf8(list.stdout).unwrap();
    for expected in [
        "pagination-stalled-cursor",
        "payload-unhandled",
        "value-mismatch",
    ] {
        assert!(stdout.contains(expected), "{expected} not listed: {stdout}");
    }

    let single = run(&dir, &["explain", "pagination-duplicate-tool"]);
    assert!(single.status.success());
    let stdout = String::from_utf8(single.stdout).unwrap();
    assert!(
        stdout.contains("paginate the catalog without overlap"),
        "{stdout}"
    );
    assert!(!stdout.contains("cursor sequence never terminated"));

    let unknown = run(&dir, &["explain", "made-up-reason"]);
    assert!(!unknown.status.success());
    assert!(String::from_utf8_lossy(&unknown.stderr).contains("unknown reason"));
}

#[test]
fn text_output_carries_a_hint_that_brief_suppresses() {
    let dir = home();
    let manifest = dir.join("m.json");
    std::fs::write(
        &manifest,
        r#"{"version":1,"probes":[{"id":"p","probe":"pagination","access":"read_only","max_pages":2}]}"#,
    )
    .unwrap();
    let mut command = Command::new(bin());
    command
        .args([
            "probe",
            "--server",
            "demo",
            "--manifest",
            manifest.to_str().unwrap(),
        ])
        .args(["--", demo(), "--broken", "stalled-cursor"])
        .env("MCPEVAL_HOME", &dir);
    let output = command.output().unwrap();
    assert!(!output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("hint:"), "{stdout}");
    assert!(stdout.contains("never re-serve a page"));

    let mut command = Command::new(bin());
    command
        .args([
            "probe",
            "--server",
            "demo",
            "--manifest",
            manifest.to_str().unwrap(),
            "--brief",
        ])
        .args(["--", demo(), "--broken", "stalled-cursor"])
        .env("MCPEVAL_HOME", &dir);
    let output = command.output().unwrap();
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(!stdout.contains("hint:"), "{stdout}");
    assert!(stdout.contains("reason=pagination-stalled-cursor"));
}

#[test]
fn markdown_failure_carries_a_remediation_section() {
    let dir = home();
    let manifest = dir.join("m.json");
    std::fs::write(
        &manifest,
        r#"{"version":1,"probes":[{"id":"p","probe":"pagination","access":"read_only","max_pages":2}]}"#,
    )
    .unwrap();
    let mut command = Command::new(bin());
    command
        .args([
            "probe",
            "--server",
            "demo",
            "--manifest",
            manifest.to_str().unwrap(),
            "--format",
            "markdown",
        ])
        .args(["--", demo(), "--broken", "duplicate-page"])
        .env("MCPEVAL_HOME", &dir);
    let output = command.output().unwrap();
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("### Remediation"), "{stdout}");
    assert!(stdout.contains("paginate the catalog without overlap"));
}

#[test]
fn calibration_context_appears_when_the_corpus_resolves() {
    let dir = home();
    let manifest = dir.join("m.json");
    std::fs::write(
        &manifest,
        r#"{"version":1,"probes":[{"id":"d","probe":"discovery-cost","access":"read_only","max_tools":50,"max_schema_bytes":200000}]}"#,
    )
    .unwrap();
    let output = Command::new(bin())
        .args([
            "probe",
            "--server",
            "demo",
            "--manifest",
            manifest.to_str().unwrap(),
        ])
        .args(["--", demo()])
        .env("MCPEVAL_HOME", &dir)
        .output()
        .unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    // The repository corpus (all strong servers, median 100) resolves even
    // from a bare temp home.
    assert!(stdout.contains("beats "), "{stdout}");
    assert!(stdout.contains("corpus median 100"), "{stdout}");

    // A home corpus overrides the repository default.
    std::fs::write(
        dir.join("corpus.json"),
        r#"{"schema":"mcpeval.readiness-corpus/v1","source":"test corpus","observations":[
            {"server":"a","score":0},{"server":"b","score":25}
        ]}"#,
    )
    .unwrap();
    let output = Command::new(bin())
        .args([
            "probe",
            "--server",
            "demo",
            "--manifest",
            manifest.to_str().unwrap(),
        ])
        .args(["--", demo()])
        .env("MCPEVAL_HOME", &dir)
        .output()
        .unwrap();
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("beats 100%"), "{stdout}");
    assert!(stdout.contains("corpus median 12"), "{stdout}");
}

#[test]
fn every_failure_reason_has_a_nonempty_hint() {
    // The full battery against the demo's broken aspects must never print
    // an empty hint, and every declared reason resolves.
    let dir = home();
    let list = run(&dir, &["explain"]);
    let stdout = String::from_utf8(list.stdout).unwrap();
    let reasons: Vec<&str> = stdout
        .lines()
        .map(str::trim)
        .filter(|line| line.contains('-') && !line.contains(' ') && !line.starts_with("fixed"))
        .collect();
    assert!(
        reasons.len() >= 20,
        "reason inventory shrinked: {reasons:?}"
    );
    for reason in reasons {
        let shown = run(&dir, &["explain", reason]);
        assert!(shown.status.success(), "{reason}");
        let body = String::from_utf8(shown.stdout).unwrap();
        assert!(body.trim().len() > 40, "{reason} hint is thin: {body:?}");
    }
}
