use std::process::Command;

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_mcpeval")
}

fn demo() -> &'static str {
    env!("CARGO_BIN_EXE_mcpeval-demo")
}

fn home() -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!("mcpeval-spec-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&path).unwrap();
    path
}

fn run_spec_probe(manifest_body: &str, demo_args: &[&str]) -> std::process::Output {
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
fn protocol_negotiation_passes_clean_and_fails_on_echoing_unknown_versions() {
    let manifest = r#"{"version":1,"probes":[{"id":"negotiates","probe":"protocol-negotiation","access":"read_only","bogus_version":"1999-12-31"}]}"#;
    let clean = run_spec_probe(manifest, &[]);
    assert!(
        clean.status.success(),
        "{}",
        String::from_utf8_lossy(&clean.stderr)
    );
    let stdout = String::from_utf8(clean.stdout).unwrap();
    assert!(stdout.contains("negotiates protocol-negotiation pass attempts=3"));

    let broken = run_spec_probe(manifest, &["--broken", "negotiation"]);
    assert!(!broken.status.success());
    let stdout = String::from_utf8(broken.stdout).unwrap();
    assert!(stdout.contains("negotiation-echoed-unknown"), "{stdout}");
}

#[test]
fn protocol_negotiation_rejects_non_date_bogus_versions() {
    let dir = home();
    let manifest = dir.join("m.json");
    std::fs::write(
        &manifest,
        r#"{"version":1,"probes":[{"id":"negotiates","probe":"protocol-negotiation","access":"read_only","bogus_version":"not-a-date"}]}"#,
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
    assert!(String::from_utf8_lossy(&output.stderr).contains("date-shaped"));
}

#[test]
fn sampling_passes_when_the_stub_reply_is_accepted_and_fails_on_malformed_requests() {
    let manifest = r#"{"version":1,"probes":[{"id":"samples","probe":"sampling","tool":"sampled_read","access":"read_only","arguments":{},"max_requests":3}]}"#;
    let clean = run_spec_probe(manifest, &[]);
    assert!(
        clean.status.success(),
        "{}",
        String::from_utf8_lossy(&clean.stderr)
    );
    let stdout = String::from_utf8(clean.stdout).unwrap();
    assert!(stdout.contains("samples sampling pass"));

    let broken = run_spec_probe(manifest, &["--broken", "sampling"]);
    assert!(!broken.status.success());
    let stdout = String::from_utf8(broken.stdout).unwrap();
    assert!(stdout.contains("sampling-invalid-request"), "{stdout}");
}

#[test]
fn sampling_manifest_bounds_are_validated() {
    let dir = home();
    let manifest = dir.join("m.json");
    std::fs::write(
        &manifest,
        r#"{"version":1,"probes":[{"id":"samples","probe":"sampling","tool":"sampled_read","access":"read_only","arguments":{},"max_requests":11}]}"#,
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
    assert!(String::from_utf8_lossy(&output.stderr).contains("between 1 and 10"));
}

#[test]
fn elicitation_passes_when_the_action_reply_is_accepted_and_fails_on_malformed_requests() {
    let manifest = r#"{"version":1,"probes":[{"id":"elicits","probe":"elicitation","tool":"elicited_read","access":"read_only","arguments":{},"max_requests":3,"respond":"accept"}]}"#;
    let clean = run_spec_probe(manifest, &[]);
    assert!(
        clean.status.success(),
        "{}",
        String::from_utf8_lossy(&clean.stderr)
    );
    let stdout = String::from_utf8(clean.stdout).unwrap();
    assert!(stdout.contains("elicits elicitation pass"));

    let broken = run_spec_probe(manifest, &["--broken", "elicitation"]);
    assert!(!broken.status.success());
    let stdout = String::from_utf8(broken.stdout).unwrap();
    assert!(stdout.contains("elicitation-invalid-request"), "{stdout}");
}

#[test]
fn resource_subscription_passes_with_notification_and_fails_without() {
    let manifest = r#"{"version":1,"probes":[{"id":"subscribes","probe":"resource-subscription","access":"read_only","uri":"demo://status","trigger_tool":"publish_status","trigger_arguments":{},"max_wait_seconds":3}]}"#;
    let clean = run_spec_probe(manifest, &[]);
    assert!(
        clean.status.success(),
        "{}",
        String::from_utf8_lossy(&clean.stderr)
    );
    let stdout = String::from_utf8(clean.stdout).unwrap();
    assert!(stdout.contains("subscribes resource-subscription pass attempts=3"));

    let broken = run_spec_probe(manifest, &["--broken", "subscription"]);
    assert!(!broken.status.success());
    let stdout = String::from_utf8(broken.stdout).unwrap();
    assert!(
        stdout.contains("subscription-notification-missing"),
        "{stdout}"
    );
}

#[test]
fn resource_subscription_requires_the_declared_uri_to_be_readable() {
    // The demo only lists demo://status; a subscription probe pointed at
    // an unlisted URI must fail at the read step with resource-unreadable.
    let manifest = r#"{"version":1,"probes":[{"id":"subscribes","probe":"resource-subscription","access":"read_only","uri":"demo://other","trigger_tool":"publish_status","trigger_arguments":{},"max_wait_seconds":2}]}"#;
    let output = run_spec_probe(manifest, &[]);
    assert!(!output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("resource-unreadable"), "{stdout}");
}

#[test]
fn json_report_carries_the_new_probe_labels() {
    let dir = home();
    let manifest = dir.join("m.json");
    std::fs::write(
        &manifest,
        r#"{"version":1,"probes":[
            {"id":"negotiates","probe":"protocol-negotiation","access":"read_only","bogus_version":"1999-12-31"},
            {"id":"samples","probe":"sampling","tool":"sampled_read","access":"read_only","arguments":{},"max_requests":3},
            {"id":"elicits","probe":"elicitation","tool":"elicited_read","access":"read_only","arguments":{},"max_requests":3,"respond":"decline"},
            {"id":"subscribes","probe":"resource-subscription","access":"read_only","uri":"demo://status","trigger_tool":"publish_status","trigger_arguments":{},"max_wait_seconds":3}
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
            "--format",
            "json",
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
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["passed"], true);
    assert_eq!(report["readiness"]["score"], 100);
    let probes: Vec<&str> = report["cases"]
        .as_array()
        .unwrap()
        .iter()
        .map(|case| case["probe"].as_str().unwrap())
        .collect();
    assert!(probes.contains(&"protocol-negotiation"));
    assert!(probes.contains(&"sampling"));
    assert!(probes.contains(&"elicitation"));
    assert!(probes.contains(&"resource-subscription"));
}

#[test]
fn broken_personalities_fail_the_new_probes_via_the_demo() {
    let cases: &[(&str, &str, &str)] = &[
        (
            "negotiation",
            r#"{"id":"n","probe":"protocol-negotiation","access":"read_only","bogus_version":"1999-12-31"}"#,
            "negotiation-echoed-unknown",
        ),
        (
            "sampling",
            r#"{"id":"s","probe":"sampling","tool":"sampled_read","access":"read_only","arguments":{},"max_requests":3}"#,
            "sampling-invalid-request",
        ),
        (
            "elicitation",
            r#"{"id":"e","probe":"elicitation","tool":"elicited_read","access":"read_only","arguments":{},"max_requests":3,"respond":"accept"}"#,
            "elicitation-invalid-request",
        ),
        (
            "subscription",
            r#"{"id":"r","probe":"resource-subscription","access":"read_only","uri":"demo://status","trigger_tool":"publish_status","trigger_arguments":{},"max_wait_seconds":2}"#,
            "subscription-notification-missing",
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
