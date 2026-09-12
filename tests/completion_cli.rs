use std::process::Command;

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_mcpeval")
}

fn demo() -> &'static str {
    env!("CARGO_BIN_EXE_mcpeval-demo")
}

fn home() -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!("mcpeval-completion-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&path).unwrap();
    path
}

fn run_completion_probe(manifest_body: &str, demo_args: &[&str]) -> std::process::Output {
    let dir = home();
    let manifest = dir.join("m.json");
    std::fs::write(&manifest, manifest_body).unwrap();
    let mut command = Command::new(bin());
    command
        .args([
            "probe",
            "--server",
            "demo",
            "--manifest",
            manifest.to_str().unwrap(),
        ])
        .args(["--", demo()])
        .args(demo_args)
        .env("MCPEVAL_HOME", &dir);
    command.output().unwrap()
}

#[test]
fn completion_passes_clean_and_reports_malformed_values_on_broken() {
    let manifest = r#"{"version":1,"probes":[
        {"id":"completes-language","probe":"completion","access":"read_only",
         "ref_type":"ref/prompt","ref_uri":"welcome","argument_name":"language",
         "argument_value":"e","max_values":10}
    ]}"#;
    let clean = run_completion_probe(manifest, &[]);
    assert!(
        clean.status.success(),
        "{}",
        String::from_utf8_lossy(&clean.stderr)
    );
    let stdout = String::from_utf8(clean.stdout).unwrap();
    assert!(
        stdout.contains("completes-language completion pass attempts=1"),
        "{stdout}"
    );

    let broken = run_completion_probe(manifest, &["--broken", "completion"]);
    assert!(!broken.status.success());
    let stdout = String::from_utf8(broken.stdout).unwrap();
    assert!(stdout.contains("completion-invalid-request"), "{stdout}");
}

#[test]
fn completion_reports_unknown_argument_with_the_structured_error() {
    let manifest = r#"{"version":1,"probes":[
        {"id":"completes-unknown","probe":"completion","access":"read_only",
         "ref_type":"ref/prompt","ref_uri":"welcome","argument_name":"style",
         "argument_value":"e","max_values":10}
    ]}"#;
    let output = run_completion_probe(manifest, &[]);
    assert!(!output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("completion-argument-unknown"), "{stdout}");
}

#[test]
fn completion_manifest_bounds_are_validated() {
    let dir = home();
    let manifest = dir.join("m.json");
    std::fs::write(
        &manifest,
        r#"{"version":1,"probes":[
            {"id":"completes","probe":"completion","access":"read_only",
             "ref_type":"ref/prompt","ref_uri":"welcome","argument_name":"language",
             "argument_value":"e","max_values":101}
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
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("max_values must be between 1 and 100"),
        "{stderr}"
    );

    // A mutating completion case is rejected before launch.
    std::fs::write(
        &manifest,
        r#"{"version":1,"probes":[
            {"id":"completes","probe":"completion","access":"mutating",
             "ref_type":"ref/prompt","ref_uri":"welcome","argument_name":"language",
             "argument_value":"e","max_values":10}
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
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    // The sandbox gate fires first: a mutating completion case never
    // launches, whatever the access-specific message would be.
    assert!(
        stderr.contains("mutating probe must name a sandbox")
            || stderr.contains("completion must be read-only"),
        "{stderr}"
    );

    // A non-date ref_type is rejected.
    std::fs::write(
        &manifest,
        r#"{"version":1,"probes":[
            {"id":"completes","probe":"completion","access":"read_only",
             "ref_type":"ref/widget","ref_uri":"welcome","argument_name":"language",
             "argument_value":"e","max_values":10}
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
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("ref_type must be ref/prompt or ref/resource"),
        "{stderr}"
    );
}

#[test]
fn completion_counts_toward_the_contract_category() {
    let manifest = r#"{"version":1,"probes":[
        {"id":"completes","probe":"completion","access":"read_only",
         "ref_type":"ref/prompt","ref_uri":"welcome","argument_name":"language",
         "argument_value":"e","max_values":10}
    ]}"#;
    let dir = home();
    let path = dir.join("m.json");
    std::fs::write(&path, manifest).unwrap();
    let output = Command::new(bin())
        .args([
            "probe",
            "--server",
            "demo",
            "--manifest",
            path.to_str().unwrap(),
            "--format",
            "json",
        ])
        .args(["--", demo()])
        .env("MCPEVAL_HOME", &dir)
        .output()
        .unwrap();
    assert!(output.status.success());
    let document: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let categories = document["readiness"]["categories"].as_array().unwrap();
    let contract = categories
        .iter()
        .find(|category| category["name"] == "contract")
        .expect("completion is a contract probe");
    assert_eq!(contract["total"], 1);
    assert_eq!(contract["passed"], 1);
    assert_eq!(document["readiness"]["score"], 100);
}

#[test]
fn completion_json_report_is_share_safe() {
    let manifest = r#"{"version":1,"probes":[
        {"id":"completes","probe":"completion","access":"read_only",
         "ref_type":"ref/prompt","ref_uri":"welcome","argument_name":"language",
         "argument_value":"e","max_values":10}
    ]}"#;
    let dir = home();
    let path = dir.join("m.json");
    std::fs::write(&path, manifest).unwrap();
    let output = Command::new(bin())
        .args([
            "probe",
            "--server",
            "demo",
            "--manifest",
            path.to_str().unwrap(),
            "--format",
            "json",
        ])
        .args(["--", demo(), "--broken", "completion"])
        .env("MCPEVAL_HOME", &dir)
        .output()
        .unwrap();
    assert!(!output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("completion-invalid-request"));
    assert!(!stdout.contains("CANARY"));
    assert!(!stdout.contains("en, es, fr"));
}
