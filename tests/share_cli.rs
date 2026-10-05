use std::process::Command;

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_mcpeval")
}

fn demo() -> &'static str {
    env!("CARGO_BIN_EXE_mcpeval-demo")
}

fn home() -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!("mcpeval-share-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&path).unwrap();
    path
}

fn record_a_battery(home: &std::path::Path) {
    let manifest = home.join("m.json");
    std::fs::write(
        &manifest,
        r#"{"version":1,"probes":[{"id":"r","probe":"degradation-over-n","tool":"read_counter","access":"read_only","arguments":{},"max_attempts":3}]}"#,
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
        .env("MCPEVAL_HOME", home)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn share_packages_the_store_and_refuses_to_overwrite() {
    let dir = home();
    record_a_battery(&dir);
    let out = dir.join("envelope");

    let first = Command::new(bin())
        .args(["share", "--dir"])
        .arg(&out)
        .env("MCPEVAL_HOME", &dir)
        .output()
        .unwrap();
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    let stdout = String::from_utf8(first.stdout).unwrap();
    assert!(stdout.contains("share envelope"), "{stdout}");

    // The envelope contains the store copy and the manifest note, and the
    // salt never lands inside it.
    assert!(out.join("SHARE.md").is_file());
    let store_copy = out.join("store");
    assert!(store_copy.is_dir());
    let shared = std::fs::read_dir(&store_copy)
        .unwrap()
        .filter_map(Result::ok)
        .count();
    assert!(shared >= 1);
    let walk = collect_files(&out);
    assert!(!walk.iter().any(|path| path.ends_with(".salt")));

    // A second share into the populated envelope is refused without force.
    let denied = Command::new(bin())
        .args(["share", "--dir"])
        .arg(&out)
        .env("MCPEVAL_HOME", &dir)
        .output()
        .unwrap();
    assert!(!denied.status.success());
    assert!(String::from_utf8_lossy(&denied.stderr).contains("--force"));

    let forced = Command::new(bin())
        .args(["share", "--dir"])
        .arg(&out)
        .arg("--force")
        .env("MCPEVAL_HOME", &dir)
        .output()
        .unwrap();
    assert!(forced.status.success());
}

#[test]
fn share_excludes_trend_history_unless_requested() {
    let dir = home();
    record_a_battery(&dir);
    let minimal = dir.join("minimal");
    let full = dir.join("full");

    Command::new(bin())
        .args(["share", "--dir"])
        .arg(&minimal)
        .env("MCPEVAL_HOME", &dir)
        .output()
        .unwrap()
        .status
        .success()
        .then_some(())
        .expect("minimal share succeeds");
    assert!(!minimal.join("store").join("probes").exists());

    Command::new(bin())
        .args(["share", "--dir"])
        .arg(&full)
        .arg("--include-probe-history")
        .env("MCPEVAL_HOME", &dir)
        .output()
        .unwrap()
        .status
        .success()
        .then_some(())
        .expect("full share succeeds");
    assert!(full
        .join("store")
        .join("probes")
        .join("history.jsonl")
        .is_file());
}

#[test]
fn share_refuses_an_empty_store() {
    let dir = home();
    let out = dir.join("envelope");
    let denied = Command::new(bin())
        .args(["share", "--dir"])
        .arg(&out)
        .env("MCPEVAL_HOME", &dir)
        .output()
        .unwrap();
    assert!(!denied.status.success());
    assert!(String::from_utf8_lossy(&denied.stderr).contains("nothing to share"));
}

#[test]
fn share_refuses_a_flagged_store_with_a_verdict_exit() {
    let dir = home();
    std::fs::create_dir_all(dir.join("store")).unwrap();
    std::fs::write(
        dir.join("store").join("calls-2026-08-04.jsonl"),
        "{\"ts\":\"2026-08-04T00:00:00Z\",\"note\":\"mail me at someone@example.com\"}\n",
    )
    .unwrap();
    let out = dir.join("envelope");
    let refused = Command::new(bin())
        .args(["share", "--dir"])
        .arg(&out)
        .env("MCPEVAL_HOME", &dir)
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&refused.stderr);
    assert_eq!(refused.status.code(), Some(1), "{stderr}");
    assert!(
        stderr.contains("redaction sweep flagged 1 file(s)"),
        "{stderr}"
    );
    assert!(!out.join("store").exists());
    assert!(!out.join("SHARE.md").exists());
}

fn collect_files(root: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut files = Vec::new();
    for entry in std::fs::read_dir(root).unwrap().flatten() {
        let path = entry.path();
        if path.is_dir() {
            files.extend(collect_files(&path));
        } else {
            files.push(path);
        }
    }
    files
}

#[test]
fn share_checks_nested_bytes_and_leaves_no_partial_envelope() {
    let dir = home();
    record_a_battery(&dir);
    let nested = dir.join("store/nested");
    std::fs::create_dir_all(&nested).unwrap();
    std::fs::write(
        nested.join("canary.jsonl"),
        "{\"synthetic\":\"token=FICTITIOUS_CANARY\"}\n",
    )
    .unwrap();
    let out = dir.join("envelope");
    let result = Command::new(bin())
        .args(["share", "--dir"])
        .arg(&out)
        .env("MCPEVAL_HOME", &dir)
        .output()
        .unwrap();
    assert_eq!(result.status.code(), Some(1));
    assert!(!out.exists());
    assert!(!String::from_utf8_lossy(&result.stderr).contains("FICTITIOUS_CANARY"));
}

#[test]
fn share_omits_annotation_prose_by_default_and_includes_it_only_on_request() {
    let dir = home();
    record_a_battery(&dir);
    std::fs::write(dir.join("store/annotations-2026-10-05.jsonl"),
        "{\"ts\":\"2026-10-05T00:00:00Z\",\"session\":\"session:synthetic\",\"seq\":1,\"kind\":\"workaround\",\"note\":\"FICTITIOUS_PRIVATE_PROSE\"}\n").unwrap();
    for include in [false, true] {
        let out = dir.join(if include { "included" } else { "default" });
        let mut cmd = Command::new(bin());
        cmd.args(["share", "--dir"])
            .arg(&out)
            .env("MCPEVAL_HOME", &dir);
        if include {
            cmd.arg("--include-annotation-notes");
        }
        let result = cmd.output().unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        let bytes =
            std::fs::read_to_string(out.join("store/annotations-2026-10-05.jsonl")).unwrap();
        assert_eq!(bytes.contains("FICTITIOUS_PRIVATE_PROSE"), include);
    }
}

#[test]
fn force_replaces_the_whole_envelope_and_preserves_it_on_refusal() {
    let dir = home();
    record_a_battery(&dir);
    let out = dir.join("envelope");
    let share = || {
        Command::new(bin())
            .args(["share", "--dir"])
            .arg(&out)
            .arg("--force")
            .env("MCPEVAL_HOME", &dir)
            .output()
            .unwrap()
    };
    assert!(share().status.success());
    std::fs::write(out.join("stale.txt"), "old content").unwrap();
    assert!(share().status.success());
    assert!(!out.join("stale.txt").exists());
    std::fs::write(out.join("preserve.txt"), "existing envelope").unwrap();
    std::fs::write(
        dir.join("store/canary.jsonl"),
        "{\"synthetic\":\"token=FICTITIOUS\"}\n",
    )
    .unwrap();
    assert_eq!(share().status.code(), Some(1));
    assert_eq!(
        std::fs::read_to_string(out.join("preserve.txt")).unwrap(),
        "existing envelope"
    );
}

#[test]
fn share_rejects_an_output_inside_the_capture_store() {
    let dir = home();
    record_a_battery(&dir);
    let out = dir.join("store/envelope");
    let result = Command::new(bin())
        .args(["share", "--dir"])
        .arg(&out)
        .env("MCPEVAL_HOME", &dir)
        .output()
        .unwrap();
    assert!(!result.status.success());
    assert!(!out.exists());
}

#[cfg(unix)]
#[test]
fn share_rejects_symlinked_source_files_and_output_directories() {
    let dir = home();
    record_a_battery(&dir);
    let outside = dir.join("outside.jsonl");
    std::fs::write(&outside, "{}\n").unwrap();
    let link = dir.join("store/link.jsonl");
    std::os::unix::fs::symlink(&outside, &link).unwrap();
    let out = dir.join("envelope");
    let result = Command::new(bin())
        .args(["share", "--dir"])
        .arg(&out)
        .env("MCPEVAL_HOME", &dir)
        .output()
        .unwrap();
    assert!(!result.status.success());
    assert!(!out.exists());
    std::fs::remove_file(link).unwrap();
    let target = dir.join("target");
    std::fs::create_dir(&target).unwrap();
    std::fs::write(target.join("preserve.txt"), "existing").unwrap();
    std::os::unix::fs::symlink(&target, &out).unwrap();
    let result = Command::new(bin())
        .args(["share", "--dir"])
        .arg(&out)
        .arg("--force")
        .env("MCPEVAL_HOME", &dir)
        .output()
        .unwrap();
    assert!(!result.status.success());
    assert!(target.join("preserve.txt").exists());
}
