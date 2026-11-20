use std::path::Path;
use std::process::Command;

use serde_json::{json, Value};

#[test]
fn fleet_scores_every_config_server_and_reports_session_cost_and_name_collisions() {
    let dir = std::env::temp_dir().join(format!("mcpeval-fleet-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let config = dir.join(".mcp.json");
    std::fs::write(
        &config,
        json!({"mcpServers": {
            "notes": {"command": "python3", "args": [fixtures.join("poisoned_catalog_server.py")]},
            "notes-env": {
                "command": "python3",
                "args": ["-c", "import os, runpy; runpy.run_path(os.environ['NOTES_SERVER'], run_name='__main__')"],
                "env": {"NOTES_SERVER": "${FLEET_FIXTURES}/poisoned_catalog_server.py"}
            },
            "legacy": {"type": "sse", "url": "http://127.0.0.1:9/sse"}
        }})
        .to_string(),
    )
    .unwrap();
    let fleet = |format: &str| {
        Command::new(env!("CARGO_BIN_EXE_mcpeval"))
            .args(["fleet", "--format", format, "--config"])
            .arg(&config)
            .env("FLEET_FIXTURES", &fixtures)
            .env("MCPEVAL_HOME", dir.join("home"))
            .output()
            .unwrap()
    };

    let output = fleet("json");
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(
        !stdout.contains(fixtures.to_str().unwrap()),
        "commands and environment values must not be reported"
    );
    let mut report: Value = serde_json::from_str(&stdout).unwrap();
    for server in report["servers"].as_array_mut().unwrap() {
        assert!(server["readiness"].is_u64(), "{server}");
        server.as_object_mut().unwrap().remove("readiness");
    }
    let tokens = report["servers"][0]["catalog_tokens"].as_u64().unwrap();
    assert_eq!(
        report,
        json!({
            "schema": "mcpeval.fleet/v1",
            "servers": [
                {"server": "notes", "tools": 2, "catalog_tokens": tokens},
                {"server": "notes-env", "tools": 2, "catalog_tokens": tokens}
            ],
            "skipped": [{"server": "legacy", "reason": "sse-transport"}],
            "tools": 4,
            "catalog_tokens": tokens * 2,
            "collisions": [
                {"tool": "draft_note", "servers": ["notes", "notes-env"]},
                {"tool": "list_contacts", "servers": ["notes", "notes-env"]}
            ]
        })
    );

    let text = fleet("text");
    assert_eq!(text.status.code(), Some(0), "{text:?}");
    let text = String::from_utf8(text.stdout).unwrap();
    assert!(
        text.starts_with(&format!(
            "fleet: 2 servers scored, 1 skipped; 4 tools, {} catalog tokens per session\n",
            tokens * 2
        )),
        "{text}"
    );
    assert!(
        text.ends_with(
            "  skipped legacy reason=sse-transport\n  collision draft_note: notes, notes-env\n  collision list_contacts: notes, notes-env\n"
        ),
        "{text}"
    );
}
