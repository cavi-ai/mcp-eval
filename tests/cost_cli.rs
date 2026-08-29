use std::process::Command;

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_mcpeval")
}

fn demo() -> &'static str {
    env!("CARGO_BIN_EXE_mcpeval-demo")
}

fn home() -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!("mcpeval-cost-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&path).unwrap();
    path
}

fn probe(format: &str, price: Option<&str>) -> std::process::Output {
    let dir = home();
    let manifest = dir.join("m.json");
    std::fs::write(
        &manifest,
        r#"{"version":1,"probes":[{"id":"t","probe":"token-cost","access":"read_only","max_total_tokens":100000}]}"#,
    )
    .unwrap();
    let mut command = Command::new(bin());
    command.args([
        "probe",
        "--server",
        "demo",
        "--manifest",
        manifest.to_str().unwrap(),
        "--format",
        format,
    ]);
    if let Some(price) = price {
        command.args(["--price-per-mtok", price]);
    }
    command.args(["--", demo()]).env("MCPEVAL_HOME", &dir);
    command.output().unwrap()
}

#[test]
fn price_flag_translates_tokens_into_session_costs() {
    let output = probe("text", Some("3"));
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(
        stdout.contains("tokens of every session before the first tool call"),
        "{stdout}"
    );
    assert!(stdout.contains("per session at $3.00/Mtok"), "{stdout}");
    assert!(stdout.contains("per 1,000 sessions"), "{stdout}");

    let markdown = probe("markdown", Some("0.5"));
    assert!(markdown.status.success());
    let body = String::from_utf8(markdown.stdout).unwrap();
    assert!(body.contains("**Session cost:**"), "{body}");
    assert!(body.contains("at $0.50/Mtok"), "{body}");
}

#[test]
fn without_price_the_reports_stay_untranslated() {
    let output = probe("text", None);
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(!stdout.contains("per session"), "{stdout}");
    assert!(stdout.contains("total_tokens="), "{stdout}");

    let markdown = probe("markdown", None);
    let body = String::from_utf8(markdown.stdout).unwrap();
    assert!(!body.contains("Session cost"), "{body}");
    assert!(body.contains("catalog cost estimate"), "{body}");
}

#[test]
fn json_report_stays_price_free_for_deterministic_baselines() {
    let with_price = probe("json", Some("3"));
    let without_price = probe("json", None);
    assert!(with_price.status.success() && without_price.status.success());
    let a = String::from_utf8(with_price.stdout).unwrap();
    let b = String::from_utf8(without_price.stdout).unwrap();
    assert_eq!(a, b, "price must not leak into the deterministic document");
    assert!(!a.contains("price"), "{a}");
}
