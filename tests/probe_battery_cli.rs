use std::process::Command;

const PAGED: &str = "tests/fixtures/probe_paged_server.py";
const CLEAN: &str = "tests/fixtures/probe_clean_server.py";

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_mcpeval")
}

fn home() -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!("mcpeval-battery-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&path).unwrap();
    path
}

fn run_probe(
    server: &str,
    manifest: &std::path::Path,
    fixture: &str,
    home: &std::path::Path,
) -> std::process::Output {
    Command::new(bin())
        .args([
            "probe",
            "--server",
            server,
            "--manifest",
            manifest.to_str().unwrap(),
        ])
        .args(["--", "python3", fixture])
        .env("MCPEVAL_HOME", home)
        .output()
        .unwrap()
}

fn pagination_manifest(dir: &std::path::Path, max_pages: u64) -> std::path::PathBuf {
    let path = dir.join("pagination.json");
    std::fs::write(
        &path,
        format!(
            r#"{{"version":1,"probes":[{{"id":"catalog-pages","probe":"pagination","access":"read_only","max_pages":{max_pages}}}]}}"#
        ),
    )
    .unwrap();
    path
}

#[test]
fn pagination_passes_when_pages_are_unique_and_complete() {
    let dir = home();
    let manifest = pagination_manifest(&dir, 10);
    let output = run_probe("fixture", &manifest, PAGED, &dir);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("catalog-pages pagination pass attempts=2 tools=5 pages=2"));
}

#[test]
fn pagination_fails_with_fixed_reasons_for_each_catalog_defect() {
    for (mode, reason) in [
        ("duplicate", "pagination-duplicate-tool"),
        ("invalid", "pagination-invalid-entry"),
    ] {
        let dir = home();
        let manifest = pagination_manifest(&dir, 10);
        let output = Command::new(bin())
            .args([
                "probe",
                "--server",
                "fixture",
                "--manifest",
                manifest.to_str().unwrap(),
            ])
            .args(["--", "python3", PAGED, mode])
            .env("MCPEVAL_HOME", &dir)
            .output()
            .unwrap();
        assert!(!output.status.success(), "{mode} should fail");
        let stdout = String::from_utf8(output.stdout).unwrap();
        assert!(stdout.contains(reason), "{mode}: {stdout}");
    }

    let dir = home();
    let manifest = pagination_manifest(&dir, 3);
    let output = Command::new(bin())
        .args([
            "probe",
            "--server",
            "fixture",
            "--manifest",
            manifest.to_str().unwrap(),
        ])
        .args(["--", "python3", PAGED, "stalled"])
        .env("MCPEVAL_HOME", &dir)
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8(output.stdout)
        .unwrap()
        .contains("pagination-stalled-cursor"));
}

#[test]
fn latency_budget_passes_generous_and_fails_tight() {
    let dir = home();
    let generous = dir.join("generous.json");
    std::fs::write(
        &generous,
        r#"{"version":1,"probes":[{"id":"fast-enough","probe":"latency-budget","tool":"read_counter","access":"read_only","arguments":{},"attempts":3,"max_latency_ms":600000}]}"#,
    )
    .unwrap();
    let output = run_probe("fixture", &generous, CLEAN, &dir);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("fast-enough latency-budget pass attempts=3"));
    assert!(stdout.contains("latency_ms="));

    let tight = dir.join("tight.json");
    std::fs::write(
        &tight,
        r#"{"version":1,"probes":[{"id":"too-slow","probe":"latency-budget","tool":"slow_read","access":"read_only","arguments":{},"attempts":3,"max_latency_ms":10}]}"#,
    )
    .unwrap();
    let output = Command::new(bin())
        .args([
            "probe",
            "--server",
            "fixture",
            "--manifest",
            tight.to_str().unwrap(),
        ])
        .args(["--", "python3", PAGED, "clean"])
        .env("MCPEVAL_HOME", &dir)
        .output()
        .unwrap();
    assert!(!output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(
        stdout.contains("reason=latency-budget-exceeded"),
        "{stdout}"
    );
    assert!(stdout.contains("first_failure=1"));
}

#[test]
fn new_probes_respect_the_share_safe_boundary() {
    let dir = home();
    let manifest = pagination_manifest(&dir, 10);
    let output = run_probe("fixture", &manifest, PAGED, &dir);
    assert!(output.status.success());
    assert!(!String::from_utf8(output.stdout).unwrap().contains("CANARY"));

    // Walk everything the run persisted under MCPEVAL_HOME/store.
    fn collect(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        for entry in std::fs::read_dir(dir).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() {
                collect(&path, out);
            } else {
                out.push(path);
            }
        }
    }
    let mut files = Vec::new();
    collect(&dir.join("store"), &mut files);
    assert!(!files.is_empty());
    for file in files {
        let body = std::fs::read_to_string(&file).unwrap();
        assert!(
            !body.contains("CANARY"),
            "{} leaked payload text",
            file.display()
        );
    }
}
