use std::path::Path;
use std::process::Command;

use mcpeval::record::{CallRecord, ErrorInfo};
use mcpeval::store::Store;
use serde_json::{json, Value};

fn call(session: &str, seq: u64, tool: &str, ok: bool) -> CallRecord {
    CallRecord {
        identity: None,
        ts: format!("2026-08-04T12:00:{seq:02}Z"),
        session: session.into(),
        seq,
        server: "demo".into(),
        method: "tools/call".into(),
        tool: Some(tool.into()),
        args: Some(json!({"shape": {"target": "str<32"}})),
        latency_ms: Some(5),
        outcome: if ok { "ok" } else { "error" }.into(),
        error: (!ok).then(|| ErrorInfo {
            code: Some(json!("blocked")),
            layer: None,
            retryable: Some(false),
            kind: None,
            template: Some("private raw template".into()),
            template_id: Some("aaaaaaaaaaaaaaaa".into()),
        }),
        shim_self_us: 1,
        kind: "real".into(),
    }
}

fn mcpeval(home: &Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_mcpeval"))
        .args(args)
        .env("MCPEVAL_HOME", home)
        .output()
        .unwrap()
}

#[test]
fn friction_counts_storms_corrections_and_wrong_tools_without_private_content() {
    let home = std::env::temp_dir().join(format!("mcpeval-friction-{}", uuid::Uuid::new_v4()));
    let mut store = Store::open(Some(home.clone())).unwrap();
    for record in [
        call("session-alpha", 1, "click", false),
        call("session-alpha", 2, "click", false),
        call("session-alpha", 3, "click", false),
        call("session-alpha", 4, "click", true),
        call("session-beta", 1, "press", false),
        call("session-beta", 2, "click", true),
    ] {
        store.append(&record).unwrap();
    }
    assert_eq!(mcpeval(&home, &["index"]).status.code(), Some(0));

    let output = mcpeval(&home, &["friction", "--format", "json"]);
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let stdout = String::from_utf8(output.stdout).unwrap();
    let document: Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(
        document,
        json!({
            "schema": "mcpeval.friction/v1",
            "storm_calls": 3,
            "servers": [{
                "server": "demo",
                "sessions": 2,
                "calls": 6,
                "failed_before_first_success": 4,
                "sessions_without_success": 0,
                "tools": [{
                    "tool": "click",
                    "retry_storms": 1,
                    "corrections": 1,
                    "correction_failures": 3
                }],
                "switches": [{"from": "press", "to": "click", "count": 1}]
            }]
        })
    );
    for private in ["session-alpha", "private raw template", "str<32"] {
        assert!(!stdout.contains(private), "{private}");
    }

    let text = mcpeval(&home, &["friction"]);
    assert_eq!(
        String::from_utf8(text.stdout).unwrap(),
        "demo: 2 sessions, 6 calls, 4 failed before the first success, 0 sessions without one\n  \
         click: 1 retry storms, 1 corrections after 3 failed calls\n  \
         press failed, then click succeeded: 1 times\n"
    );
}

#[test]
fn friction_needs_an_index() {
    let home = std::env::temp_dir().join(format!("mcpeval-friction-{}", uuid::Uuid::new_v4()));
    let output = mcpeval(&home, &["friction"]);
    assert_ne!(output.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&output.stderr).contains("run `mcpeval index` first"));
}
