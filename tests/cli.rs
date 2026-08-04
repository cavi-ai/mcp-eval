use std::process::Command;

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_mcpeval")
}

#[test]
fn prints_version() {
    let out = Command::new(bin()).arg("--version").output().unwrap();
    assert!(out.status.success());
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(
        text.contains("mcpeval"),
        "unexpected version output: {text}"
    );
}

#[test]
fn shim_requires_a_server_name_and_command() {
    let out = Command::new(bin()).arg("shim").output().unwrap();
    assert!(!out.status.success(), "shim with no args must fail");
}

#[test]
fn shim_rejects_server_labels_that_could_carry_content() {
    let out = Command::new(bin())
        .args(["shim", "--server", "CANARY/path?token=x", "--", "true"])
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(!String::from_utf8_lossy(&out.stderr).contains("CANARY"));
}
