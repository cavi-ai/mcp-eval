use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

use serde_json::Value;

struct Server {
    child: Child,
    endpoint: String,
    events: PathBuf,
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn server(mode: &str, dir: &Path) -> Server {
    let events = dir.join(format!("{mode}.events"));
    let mut child = Command::new("python3")
        .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/auth_http_server.py"))
        .arg(mode)
        .arg(&events)
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut port = String::new();
    BufReader::new(child.stdout.take().unwrap())
        .read_line(&mut port)
        .unwrap();
    Server {
        child,
        endpoint: format!("http://127.0.0.1:{}/mcp", port.trim()),
        events,
    }
}

fn score(server: &Server, dir: &Path) -> (Value, String) {
    let output = Command::new(env!("CARGO_BIN_EXE_mcpeval"))
        .args(["score", "--server", "auth", "--format", "json", "--url"])
        .arg(&server.endpoint)
        .env("MCPEVAL_HOME", dir)
        .env("MCPEVAL_HTTP_AUTHORIZATION", "Bearer t0ken")
        .env("MCPEVAL_HTTP_HEADERS", "X-Tenant: acme")
        .output()
        .unwrap();
    let stdout = String::from_utf8(output.stdout).unwrap();
    let report = serde_json::from_str(&stdout)
        .unwrap_or_else(|_| panic!("{}", String::from_utf8_lossy(&output.stderr)));
    (report, stdout)
}

fn protocol_losses(report: &Value) -> Vec<(String, String)> {
    report["readiness"]["areas"]
        .as_array()
        .unwrap()
        .iter()
        .find(|area| area["name"] == "protocol")
        .unwrap()["checks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|check| {
            (
                check["id"].as_str().unwrap().to_owned(),
                check["reason"].as_str().unwrap().to_owned(),
            )
        })
        .collect()
}

#[test]
fn http_servers_must_refuse_foreign_origins_and_challenge_missing_credentials() {
    let dir = std::env::temp_dir().join(format!("mcpeval-http-auth-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();

    let strict = server("strict", &dir);
    let (report, stdout) = score(&strict, &dir);
    assert_eq!(protocol_losses(&report), vec![], "{report:#}");
    let events = std::fs::read_to_string(&strict.events).unwrap();
    for event in ["foreign-origin", "uncredentialed", "metadata"] {
        assert!(
            events.lines().any(|line| line == event),
            "{event}: {events}"
        );
    }
    assert!(!stdout.contains("t0ken") && !stdout.contains("acme"));

    let lax = server("lax", &dir);
    let (report, _) = score(&lax, &dir);
    assert_eq!(
        protocol_losses(&report),
        [
            (
                "protocol.origin".to_owned(),
                "protocol-origin-accepted".to_owned()
            ),
            (
                "protocol.auth-challenge".to_owned(),
                "protocol-auth-no-challenge".to_owned()
            ),
        ],
        "{report:#}"
    );
    let schema: Value = serde_json::from_str(
        &std::fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/mcp-eval.probe-report.schema.json"),
        )
        .unwrap(),
    )
    .unwrap();
    let errors: Vec<String> = jsonschema::validator_for(&schema)
        .unwrap()
        .iter_errors(&report)
        .map(|error| format!("{error} at {}", error.instance_path()))
        .collect();
    assert!(errors.is_empty(), "{errors:#?}");
}
