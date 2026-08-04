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
