use std::process::Command;

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_mcpeval")
}

fn tempdir() -> std::path::PathBuf {
    let base = std::env::temp_dir().join(format!("mcpeval-doctor-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(base.join("store")).unwrap();
    base
}

#[test]
fn passes_on_a_clean_store() {
    let home = tempdir();
    std::fs::write(
        home.join("store").join("calls-2026-08-04.jsonl"),
        "{\"ts\":\"2026-08-04T00:00:00Z\",\"session\":\"session:ab\",\"seq\":1,\"server\":\"demo\",\"method\":\"tools/call\",\"outcome\":\"ok\",\"shim_self_us\":1,\"kind\":\"real\"}\n",
    )
    .unwrap();
    let out = Command::new(bin())
        .args(["doctor", "--check-redaction"])
        .env("MCPEVAL_HOME", &home)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
}

#[test]
fn fails_when_a_store_file_contains_content() {
    let home = tempdir();
    std::fs::write(
        home.join("store").join("calls-2026-08-04.jsonl"),
        "{\"ts\":\"2026-08-04T00:00:00Z\",\"note\":\"mail me at someone@example.com\"}\n",
    )
    .unwrap();
    let out = Command::new(bin())
        .args(["doctor", "--check-redaction"])
        .env("MCPEVAL_HOME", &home)
        .output()
        .unwrap();
    assert!(!out.status.success(), "an email address must be reported");
}

#[test]
fn a_legitimate_note_containing_an_at_sign_is_not_flagged() {
    let home = tempdir();
    std::fs::write(
        home.join("store").join("annotations-2026-08-04.jsonl"),
        "{\"ts\":\"2026-08-04T00:00:00Z\",\"session\":\"session:ab\",\"seq\":1,\"kind\":\"workaround\",\"note\":\"reach me at someone@example.com if this repeats\"}\n",
    )
    .unwrap();
    let out = Command::new(bin())
        .args(["doctor", "--check-redaction"])
        .env("MCPEVAL_HOME", &home)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "note is free-form prose and must be exempt: {}",
        String::from_utf8_lossy(&out.stdout)
    );
}

#[test]
fn a_leak_in_a_non_note_annotation_field_is_still_flagged() {
    let home = tempdir();
    std::fs::write(
        home.join("store").join("annotations-2026-08-04.jsonl"),
        "{\"ts\":\"2026-08-04T00:00:00Z\",\"session\":\"someone@example.com\",\"seq\":1,\"kind\":\"workaround\",\"note\":\"nothing sensitive here\"}\n",
    )
    .unwrap();
    let out = Command::new(bin())
        .args(["doctor", "--check-redaction"])
        .env("MCPEVAL_HOME", &home)
        .output()
        .unwrap();
    assert!(
        !out.status.success(),
        "a leak outside note must still be reported"
    );
}

#[test]
fn bare_doctor_with_no_flag_still_runs_the_redaction_check() {
    // M7: a mistyped or omitted flag must not read as a silent pass.
    let home = tempdir();
    std::fs::write(
        home.join("store").join("calls-2026-08-04.jsonl"),
        "{\"ts\":\"2026-08-04T00:00:00Z\",\"note\":\"mail me at someone@example.com\"}\n",
    )
    .unwrap();
    let out = Command::new(bin())
        .args(["doctor"])
        .env("MCPEVAL_HOME", &home)
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        !out.status.success(),
        "bare `doctor` must still run the redaction check: {stdout}"
    );
    assert!(
        stdout.contains("calls-2026-08-04.jsonl"),
        "bare `doctor` produced no findings output: {stdout}"
    );
}

#[test]
fn doctor_names_the_salt_path_as_a_must_not_share_item() {
    let home = tempdir();
    std::fs::write(
        home.join("store").join("calls-2026-08-04.jsonl"),
        "{\"ts\":\"2026-08-04T00:00:00Z\",\"session\":\"session:ab\",\"seq\":1,\"server\":\"demo\",\"method\":\"tools/call\",\"outcome\":\"ok\",\"shim_self_us\":1,\"kind\":\"real\"}\n",
    )
    .unwrap();
    let out = Command::new(bin())
        .args(["doctor", "--check-redaction"])
        .env("MCPEVAL_HOME", &home)
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{stdout}");
    let salt_path = home.join(".salt");
    assert!(
        stdout.contains(&salt_path.display().to_string()),
        "doctor must name the salt path as a must-not-share item: {stdout}"
    );
}

#[test]
fn annotation_notes_are_counted_for_review_but_do_not_fail_the_check() {
    let home = tempdir();
    std::fs::write(
        home.join("store").join("annotations-2026-08-04.jsonl"),
        concat!(
            "{\"ts\":\"2026-08-04T00:00:00Z\",\"session\":\"session:ab\",\"seq\":1,\"kind\":\"workaround\",\"note\":\"asked db-internal-07 for help\"}\n",
            "{\"ts\":\"2026-08-04T00:00:01Z\",\"session\":\"session:ab\",\"seq\":2,\"kind\":\"workaround\",\"note\":\"\"}\n",
        ),
    )
    .unwrap();
    let out = Command::new(bin())
        .args(["doctor", "--check-redaction"])
        .env("MCPEVAL_HOME", &home)
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "notes must not fail the check: {stdout}"
    );
    assert!(
        stdout.contains("1 annotation notes contain agent prose; review before sharing"),
        "must report exactly one note requiring review: {stdout}"
    );
}

#[test]
fn a_long_string_beginning_with_a_shape_token_prefix_is_still_flagged() {
    // D3: `is_shape_token` must match the closed forms exactly, not by
    // prefix — a string that merely starts with "str<8" must not bypass
    // the oversized-string detector at arbitrary length.
    let home = tempdir();
    let payload = format!("str<8{}", "x".repeat(200));
    let line = serde_json::json!({ "junk": payload }).to_string();
    std::fs::write(
        home.join("store").join("calls-2026-08-04.jsonl"),
        format!("{line}\n"),
    )
    .unwrap();
    let report = mcpeval::doctor::check_redaction(&home).unwrap();
    assert_eq!(
        report.findings.len(),
        1,
        "a str<8-prefixed oversized string must still be flagged: {:?}",
        report.findings
    );
}
