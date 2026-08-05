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
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stdout));
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
