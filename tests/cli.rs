use std::process::Command;

use mcpeval::index;
use mcpeval::record::{CallRecord, ErrorInfo};
use mcpeval::store::Store;
use serde_json::json;

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
fn help_orders_the_battery_before_capture_and_shows_start_here() {
    let out = Command::new(bin()).arg("--help").output().unwrap();
    assert!(out.status.success());
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(
        text.starts_with(
            "Evaluate MCP servers: a deterministic probe battery for CI and privacy-safe \
             friction capture from real sessions"
        ),
        "unexpected about line: {text}"
    );

    let init = text.find("\n  init ").expect("init in Commands section");
    let probe = text.find("\n  probe ").expect("probe in Commands section");
    let shim = text.find("\n  shim ").expect("shim in Commands section");
    let doctor = text
        .find("\n  doctor ")
        .expect("doctor in Commands section");
    let share = text.find("\n  share ").expect("share in Commands section");
    assert!(init < probe, "init must precede probe");
    assert!(probe < shim, "probe must precede shim");
    assert!(shim < doctor, "shim must precede doctor");
    assert!(doctor < share, "doctor must precede share; share is last");

    assert!(text.contains("Start here:"), "missing Start here: block");
    assert!(text.contains(
        "mcpeval init --server NAME --confirm-read-only -- your-mcp-server   # scaffold a manifest"
    ));
    assert!(text.contains(
        "mcpeval probe --server NAME -- your-mcp-server                     # run the battery"
    ));
    assert!(text.contains(
        "mcpeval shim --server NAME -- your-mcp-server                      # capture real sessions"
    ));
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

fn promotion_home() -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("mcpeval-cli-{}", uuid::Uuid::new_v4()));
    let mut store = Store::open(Some(dir.clone())).unwrap();
    let ts = chrono::Utc::now().to_rfc3339();
    for (session, seq) in [("s1", 1), ("s2", 1)] {
        store
            .append(&CallRecord {
                ts: ts.clone(),
                session: session.into(),
                seq,
                server: "demo".into(),
                method: "tools/call".into(),
                tool: Some("click".into()),
                args: None,
                latency_ms: Some(1),
                outcome: "error".into(),
                error: Some(ErrorInfo {
                    code: Some(json!("blocked")),
                    layer: None,
                    retryable: Some(false),
                    kind: None,
                    template: Some("raw".into()),
                    template_id: Some("aaaaaaaaaaaaaaaa".into()),
                }),
                shim_self_us: 1,
                kind: "real".into(),
            })
            .unwrap();
    }
    index::build(&dir).unwrap();
    dir
}

#[test]
fn promote_cli_uses_explicit_threshold_and_prints_counts() {
    let dir = promotion_home();
    let out = Command::new(bin())
        .args(["promote", "--threshold", "0"])
        .env("MCPEVAL_HOME", &dir)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        String::from_utf8(out.stdout).unwrap(),
        "promoted 1 of 1 issues\n"
    );
}

#[test]
fn promote_cli_override_takes_precedence_over_config_file() {
    let dir = promotion_home();
    std::fs::write(
        dir.join("config.json"),
        r#"{"promotion_threshold":999999.0}"#,
    )
    .unwrap();
    let out = Command::new(bin())
        .args(["promote", "--threshold", "0"])
        .env("MCPEVAL_HOME", &dir)
        .output()
        .unwrap();
    assert!(out.status.success());
    assert_eq!(
        String::from_utf8(out.stdout).unwrap(),
        "promoted 1 of 1 issues\n"
    );
}

#[test]
fn promote_cli_uses_the_calibrated_default_when_config_is_absent() {
    let dir = promotion_home();
    let out = Command::new(bin())
        .arg("promote")
        .env("MCPEVAL_HOME", &dir)
        .output()
        .unwrap();
    assert!(out.status.success());
    assert_eq!(
        String::from_utf8(out.stdout).unwrap(),
        "promoted 1 of 1 issues\n"
    );
}

#[test]
fn promote_cli_uses_a_valid_configured_threshold() {
    let dir = promotion_home();
    std::fs::write(dir.join("config.json"), r#"{"promotion_threshold":0}"#).unwrap();
    let out = Command::new(bin())
        .arg("promote")
        .env("MCPEVAL_HOME", &dir)
        .output()
        .unwrap();
    assert!(out.status.success());
    assert_eq!(
        String::from_utf8(out.stdout).unwrap(),
        "promoted 1 of 1 issues\n"
    );
}

#[test]
fn promote_cli_rejects_invalid_configured_threshold() {
    let dir = promotion_home();
    std::fs::write(dir.join("config.json"), r#"{"promotion_threshold":-1}"#).unwrap();
    let out = Command::new(bin())
        .arg("promote")
        .env("MCPEVAL_HOME", &dir)
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("finite and non-negative"));
}

#[test]
fn promote_cli_rejects_non_finite_explicit_threshold() {
    let dir = promotion_home();
    let out = Command::new(bin())
        .args(["promote", "--threshold", "NaN"])
        .env("MCPEVAL_HOME", &dir)
        .output()
        .unwrap();
    assert!(!out.status.success());
}

#[test]
fn findings_cli_renders_the_selected_format() {
    let dir = promotion_home();
    let promoted = Command::new(bin())
        .args(["promote", "--threshold", "0"])
        .env("MCPEVAL_HOME", &dir)
        .output()
        .unwrap();
    assert!(promoted.status.success());

    let out = Command::new(bin())
        .args(["findings", "--format", "json"])
        .env("MCPEVAL_HOME", &dir)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let value: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(value.as_array().unwrap().len(), 1);
}

#[test]
fn promote_cli_says_why_issues_were_not_promoted_and_findings_explains_an_empty_list() {
    let dir = std::env::temp_dir().join(format!("mcpeval-cli-{}", uuid::Uuid::new_v4()));
    let mut store = Store::open(Some(dir.clone())).unwrap();
    store
        .append(&CallRecord {
            ts: chrono::Utc::now().to_rfc3339(),
            session: "only".into(),
            seq: 1,
            server: "demo".into(),
            method: "tools/call".into(),
            tool: Some("click".into()),
            args: None,
            latency_ms: Some(1),
            outcome: "error".into(),
            error: Some(ErrorInfo {
                code: Some(json!("blocked")),
                layer: None,
                retryable: Some(false),
                kind: None,
                template: None,
                template_id: Some("aaaaaaaaaaaaaaaa".into()),
            }),
            shim_self_us: 1,
            kind: "real".into(),
        })
        .unwrap();
    index::build(&dir).unwrap();
    let promoted = Command::new(bin())
        .args(["promote", "--threshold", "0"])
        .env("MCPEVAL_HOME", &dir)
        .output()
        .unwrap();
    assert!(promoted.status.success());
    assert_eq!(
        String::from_utf8_lossy(&promoted.stdout),
        "promoted 0 of 1 issues (1 seen in one session only)\n"
    );

    for format in ["agent", "md", "json"] {
        let out = Command::new(bin())
            .args(["findings", "--format", format])
            .env("MCPEVAL_HOME", &dir)
            .output()
            .unwrap();
        assert!(out.status.success(), "{format}");
        assert_eq!(
            String::from_utf8_lossy(&out.stderr),
            "no promoted findings; run mcpeval promote --threshold 0 to see every issue\n",
            "{format}"
        );
        if format == "agent" {
            assert!(out.stdout.is_empty());
        }
    }
}
