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
    assert_eq!(unknown.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&unknown.stderr);
    assert!(
        stderr.contains("unknown reason made-up-reason; run mcpeval explain for the list"),
        "{stderr}"
    );

    let near = run(&dir, &["explain", "pagination-stalled"]);
    assert_eq!(near.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&near.stderr);
    let suggestions = stderr
        .split_once("unknown reason pagination-stalled; did you mean: ")
        .map(|(_, rest)| rest.trim_end())
        .unwrap_or_else(|| panic!("no suggestions: {stderr}"));
    assert!(
        suggestions
            .split(", ")
            .any(|label| label == "pagination-stalled-cursor"),
        "{stderr}"
    );

    let prefix = run(&dir, &["explain", "unstable"]);
    assert_eq!(prefix.status.code(), Some(2));
    assert!(
        String::from_utf8_lossy(&prefix.stderr).contains("did you mean: unstable-error-code"),
        "{}",
        String::from_utf8_lossy(&prefix.stderr)
    );
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

fn probe_demo(dir: &std::path::Path, probes: &str, demo_args: &[&str]) -> (bool, String) {
    let manifest = dir.join("m.json");
    std::fs::write(&manifest, format!(r#"{{"version":1,"probes":[{probes}]}}"#)).unwrap();
    let output = Command::new(bin())
        .args([
            "probe",
            "--server",
            "demo",
            "--manifest",
            manifest.to_str().unwrap(),
        ])
        .arg("--")
        .arg(demo())
        .args(demo_args)
        .env("MCPEVAL_HOME", dir)
        .output()
        .unwrap();
    (
        output.status.success(),
        String::from_utf8(output.stdout).unwrap(),
    )
}

const DISCOVERY: &str = r#"{"id":"d","probe":"discovery-cost","access":"read_only","max_tools":50,"max_schema_bytes":200000}"#;
const TOKENS: &str =
    r#"{"id":"t","probe":"token-cost","access":"read_only","max_total_tokens":100000}"#;

fn measured(stdout: &str, case: &str, key: &str) -> u64 {
    let line = stdout
        .lines()
        .find(|line| line.starts_with(&format!("{case} ")))
        .unwrap_or_else(|| panic!("no {case} line: {stdout}"));
    line.split_whitespace()
        .find_map(|field| field.strip_prefix(&format!("{key}=")))
        .unwrap_or_else(|| panic!("no {key} on {line}"))
        .parse()
        .unwrap()
}

#[test]
fn calibration_context_appears_when_the_corpus_resolves() {
    // The repository corpus resolves even from a bare temp home.
    let dir = home();
    let (passed, stdout) = probe_demo(&dir, DISCOVERY, &[]);
    assert!(passed, "{stdout}");
    assert!(
        stdout.contains("\ndemo readiness 100/100 discovery=1/1\n"),
        "{stdout}"
    );
    assert!(
        stdout.contains(
            "\n  corpus battery (discovery-cost, token-cost, pagination, surface-listing): 100/100, above "
        ),
        "{stdout}"
    );
    assert!(!stdout.contains("beats"), "{stdout}");

    // A home corpus overrides the repository default; its battery and
    // catalog measurements drive both calibration lines.
    std::fs::write(
        dir.join("corpus.json"),
        r#"{"schema":"mcpeval.readiness-corpus/v1","source":"test corpus",
            "battery":["discovery-cost","token-cost"],
            "observations":[
                {"server":"a","score":100,"tool_count":3,"catalog_tokens":1},
                {"server":"b","score":100,"tool_count":90,"catalog_tokens":1000000},
                {"server":"c","score":50,"tool_count":200,"catalog_tokens":2000000},
                {"server":"d","score":25}
        ]}"#,
    )
    .unwrap();
    let (passed, stdout) = probe_demo(&dir, &format!("{DISCOVERY},{TOKENS}"), &[]);
    assert!(passed, "{stdout}");
    let tokens = measured(&stdout, "t", "total_tokens");
    let tools = measured(&stdout, "t", "tools");
    assert!(
        stdout.contains(
            "\n  corpus battery (discovery-cost, token-cost): 100/100, above 2, tied with 2, below 0 of 4 observed servers\n"
        ),
        "{stdout}"
    );
    assert!(
        stdout.contains(&format!(
            "\n  catalog: {tokens} tokens over {tools} tools, lighter than 2 of 3 observed servers (median 1000000 tokens)\n"
        )),
        "{stdout}"
    );

    // A v1 corpus without a battery or measurements: the default battery
    // labels the score line, and the catalog line is omitted.
    std::fs::write(
        dir.join("corpus.json"),
        r#"{"schema":"mcpeval.readiness-corpus/v1","source":"test corpus","observations":[
            {"server":"a","score":0},{"server":"b","score":100}
        ]}"#,
    )
    .unwrap();
    let (passed, stdout) = probe_demo(&dir, &format!("{DISCOVERY},{TOKENS}"), &[]);
    assert!(passed, "{stdout}");
    assert!(
        stdout.contains(
            "\n  corpus battery (discovery-cost, token-cost, pagination, surface-listing): 100/100, above 1, tied with 1, below 0 of 2 observed servers\n"
        ),
        "{stdout}"
    );
    assert!(!stdout.contains("catalog:"), "{stdout}");
}

#[test]
fn corpus_placement_scores_only_the_corpus_battery() {
    // A failing non-battery case pulls the overall score down; the corpus
    // line compares only the battery cases the corpus was collected with.
    let dir = home();
    std::fs::write(
        dir.join("corpus.json"),
        r#"{"schema":"mcpeval.readiness-corpus/v1","source":"test corpus","observations":[
            {"server":"a","score":75},{"server":"b","score":100}
        ]}"#,
    )
    .unwrap();
    let slow = r#"{"id":"slow","probe":"latency-budget","tool":"slow_read","access":"read_only","arguments":{},"attempts":2,"max_latency_ms":50}"#;
    let (passed, stdout) = probe_demo(&dir, &format!("{DISCOVERY},{slow}"), &["--broken", "slow"]);
    assert!(!passed, "{stdout}");
    assert!(
        stdout.contains("\ndemo readiness 42/100 discovery=1/1 reliability=0/1\n"),
        "{stdout}"
    );
    assert!(
        stdout.contains(
            "\n  corpus battery (discovery-cost, token-cost, pagination, surface-listing): 100/100, above 1, tied with 1, below 0 of 2 observed servers\n"
        ),
        "{stdout}"
    );

    // No case of the corpus battery: no corpus line at all.
    let (_, stdout) = probe_demo(&dir, slow, &["--broken", "slow"]);
    assert!(
        stdout.contains("\ndemo readiness 0/100 reliability=0/1\n"),
        "{stdout}"
    );
    assert!(!stdout.contains("corpus battery"), "{stdout}");
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
        assert!(
            !body.contains('\\'),
            "{reason} hint carries a literal backslash: {body:?}"
        );
        assert_eq!(
            body.trim().lines().count(),
            1,
            "{reason} hint spans more than one line: {body:?}"
        );
    }
}
