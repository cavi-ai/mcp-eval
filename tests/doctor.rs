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

#[test]
fn nested_journals_report_sorted_paths_and_physical_lines_without_content() {
    let home = tempdir();
    let nested = home.join("store/probes/server");
    std::fs::create_dir_all(&nested).unwrap();
    let annotation = nested.join("annotations-day.jsonl");
    std::fs::write(
        &annotation,
        concat!(
            "{\"note\":\"someone@example.com\"}\r\n",
            "{\"note\":\"review me\",\"session\":\"someone@example.com\"}\r\n",
        ),
    )
    .unwrap();
    let calls = nested.join("calls-day.jsonl");
    // A malformed, unterminated tail is still in scope for the text scan.
    std::fs::write(&calls, "\r\n{}\r\nsomeone@example.com").unwrap();
    let report = mcpeval::doctor::check_redaction(&home).unwrap();
    assert_eq!(report.files, 2);
    assert_eq!(report.notes_requiring_review, 2);
    assert_eq!(
        report.findings,
        vec![
            format!("{}:2", annotation.display()),
            format!("{}:3", calls.display())
        ]
    );
    let out = Command::new(bin())
        .arg("doctor")
        .env("MCPEVAL_HOME", &home)
        .output()
        .unwrap();
    assert!(!out.status.success());
    let output = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(output.contains("calls-day.jsonl:3"));
    assert!(!output.contains("someone@example.com"));
    assert!(!output.contains("review me"));
}

#[test]
fn physical_lines_are_limited_including_the_newline() {
    const LIMIT: usize = 4 * 1024 * 1024;
    let home = tempdir();
    let path = home.join("store/calls-day.jsonl");
    let mut line = vec![b' '; LIMIT - 1];
    line.push(b'\n');
    std::fs::write(&path, &line).unwrap();
    assert!(mcpeval::doctor::check_redaction(&home)
        .unwrap()
        .findings
        .is_empty());
    line.insert(0, b' ');
    std::fs::write(&path, &line).unwrap();
    let error = mcpeval::doctor::check_redaction(&home)
        .err()
        .expect("oversized record must fail");
    let message = format!("{error:#}");
    assert!(message.contains("exceeds 4 MiB"), "{message}");
    assert!(
        message.contains("calls-day.jsonl") && message.contains("line 1"),
        "{message}"
    );
}

#[test]
fn invalid_utf8_fails_with_a_location_without_disclosing_content() {
    let home = tempdir();
    std::fs::write(
        home.join("store/calls-day.jsonl"),
        b"{}\nprivate-canary\xff\n",
    )
    .unwrap();
    let error = mcpeval::doctor::check_redaction(&home)
        .err()
        .expect("invalid UTF-8 must fail");
    let message = format!("{error:#}");
    assert!(
        message.contains("calls-day.jsonl") && message.contains("line 2"),
        "{message}"
    );
    assert!(!message.contains("private-canary"));
}

#[test]
fn doctor_waits_for_the_journal_writer_lock() {
    use std::sync::mpsc;
    use std::time::Duration;
    let home = tempdir();
    let path = home.join("store/calls-day.jsonl");
    std::fs::write(&path, "{}\n").unwrap();
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&path)
        .unwrap();
    file.lock().unwrap();
    let (started_tx, started_rx) = mpsc::channel();
    let (done_tx, done_rx) = mpsc::channel();
    let worker = std::thread::spawn(move || {
        started_tx.send(()).unwrap();
        done_tx
            .send(mcpeval::doctor::check_redaction(&home).unwrap().files)
            .unwrap();
    });
    started_rx.recv().unwrap();
    let blocked = matches!(
        done_rx.recv_timeout(Duration::from_millis(150)),
        Err(mpsc::RecvTimeoutError::Timeout)
    );
    file.unlock().unwrap();
    if blocked {
        assert_eq!(done_rx.recv_timeout(Duration::from_secs(5)).unwrap(), 1);
    }
    worker.join().unwrap();
    assert!(blocked, "doctor ignored the journal writer lock");
}

#[cfg(unix)]
#[test]
fn symlinked_store_files_directories_and_store_root_are_refused() {
    for kind in ["file", "directory", "root"] {
        let home = tempdir();
        let outside = home.join("outside");
        std::fs::create_dir(&outside).unwrap();
        let file = outside.join("calls-day.jsonl");
        std::fs::write(&file, "{}\n").unwrap();
        match kind {
            "file" => {
                std::os::unix::fs::symlink(&file, home.join("store/calls-day.jsonl")).unwrap()
            }
            "directory" => std::os::unix::fs::symlink(&outside, home.join("store/nested")).unwrap(),
            "root" => {
                std::fs::remove_dir(home.join("store")).unwrap();
                std::os::unix::fs::symlink(&outside, home.join("store")).unwrap();
            }
            _ => unreachable!(),
        }
        let error = mcpeval::doctor::check_redaction(&home)
            .err()
            .expect("symlinks must fail");
        assert!(format!("{error:#}").contains("symlink"), "{error:#}");
    }
}
