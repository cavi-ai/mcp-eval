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
