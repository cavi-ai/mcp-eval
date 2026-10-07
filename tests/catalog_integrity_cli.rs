use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};

use serde_json::{json, Value};

const MODES: [&str; 4] = [
    "catalog-error",
    "catalog-invalid-page",
    "catalog-invalid-entry",
    "catalog-stalled",
];

struct Target {
    root: PathBuf,
    mode: &'static str,
    server: Option<Child>,
    url: Option<String>,
}

impl Target {
    fn new(mode: &'static str, http: bool) -> Self {
        let root = std::env::temp_dir().join(format!("mcpeval-catalog-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let mut target = Self {
            root,
            mode,
            server: None,
            url: None,
        };
        if http {
            let mut server = Command::new("python3")
                .arg(fixture())
                .args([mode, "http"])
                .env("WORKFLOW_CALL_LOG", target.log())
                .stdout(Stdio::piped())
                .spawn()
                .unwrap();
            let mut url = String::new();
            BufReader::new(server.stdout.take().unwrap())
                .read_line(&mut url)
                .unwrap();
            target.server = Some(server);
            target.url = Some(url.trim().into());
        }
        target
    }

    fn log(&self) -> PathBuf {
        self.root.join("calls.jsonl")
    }

    fn run(&self, args: &[&str]) -> Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_mcpeval"));
        command
            .args(args)
            .env("MCPEVAL_HOME", &self.root)
            .env("WORKFLOW_CALL_LOG", self.log());
        if let Some(url) = &self.url {
            command.args(["--url", url]);
        } else {
            command
                .args(["--", "python3"])
                .arg(fixture())
                .arg(self.mode);
        }
        command.output().unwrap()
    }

    fn assert_no_calls_or_payloads(&self, output: &Output) {
        let events: Vec<Value> = std::fs::read_to_string(self.log())
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert!(!events.iter().any(|event| event["method"] == "tools/call"));
        assert!(!String::from_utf8_lossy(&output.stdout).contains("CANARY"));
        assert!(!String::from_utf8_lossy(&output.stderr).contains("CANARY"));
    }
}

impl Drop for Target {
    fn drop(&mut self) {
        if let Some(server) = self.server.as_mut() {
            let _ = server.kill();
            let _ = server.wait();
        }
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/workflow_server.py")
}

fn manifest(root: &Path) -> PathBuf {
    let path = root.join("manifest.json");
    std::fs::write(
        &path,
        json!({"version":1,"probes":[
            {"id":"read","probe":"degradation-over-n","tool":"read_status",
                "access":"read_only","arguments":{},"max_attempts":3},
            {"id":"pages","probe":"pagination","access":"read_only","max_pages":3}
        ]})
        .to_string(),
    )
    .unwrap();
    path
}

#[test]
fn incomplete_discovery_never_replaces_a_manifest_or_calls_tools() {
    for mode in MODES {
        for http in [false, true] {
            let target = Target::new(mode, http);
            let output = target.root.join("generated.json");
            std::fs::write(&output, "keep existing manifest").unwrap();
            let result = target.run(&[
                "init",
                "--server",
                "fixture",
                "--confirm-read-only",
                "--force",
                "--output",
                output.to_str().unwrap(),
            ]);
            assert_eq!(
                result.status.code(),
                Some(3),
                "{mode} http={http}: {} {}",
                String::from_utf8_lossy(&result.stdout),
                String::from_utf8_lossy(&result.stderr)
            );
            assert_eq!(
                std::fs::read_to_string(output).unwrap(),
                "keep existing manifest"
            );
            target.assert_no_calls_or_payloads(&result);
        }
    }
}

#[test]
fn incomplete_discovery_errors_tool_cases_but_preserves_pagination_diagnostics() {
    for mode in MODES {
        for http in [false, true] {
            let target = Target::new(mode, http);
            let path = manifest(&target.root);
            let result = target.run(&[
                "probe",
                "--server",
                "fixture",
                "--manifest",
                path.to_str().unwrap(),
                "--format",
                "json",
            ]);
            assert_eq!(
                result.status.code(),
                Some(3),
                "{mode} http={http}: {} {}",
                String::from_utf8_lossy(&result.stdout),
                String::from_utf8_lossy(&result.stderr)
            );
            let report: Value = serde_json::from_slice(&result.stdout).unwrap();
            assert_eq!(report["cases"][0]["reason"], "transport-error");
            assert_eq!(report["cases"][0]["attempts"], 0);
            assert_eq!(report["cases"][0]["first_failure"], Value::Null);
            assert_eq!(
                report["cases"][1]["reason"],
                if mode == "catalog-stalled" {
                    "pagination-stalled-cursor"
                } else {
                    "pagination-invalid-entry"
                }
            );
            assert_eq!(report["gate"]["passed"], 0);
            target.assert_no_calls_or_payloads(&result);

            let diagnostic = target.run(&[
                "probe",
                "--server",
                "fixture",
                "--manifest",
                path.to_str().unwrap(),
                "--format",
                "json",
                "--probe",
                "pagination",
            ]);
            assert_eq!(diagnostic.status.code(), Some(1));
            let only: Value = serde_json::from_slice(&diagnostic.stdout).unwrap();
            assert_eq!(only["cases"].as_array().unwrap().len(), 1);
            assert_eq!(only["cases"][0]["reason"], report["cases"][1]["reason"]);
            target.assert_no_calls_or_payloads(&diagnostic);
        }
    }
}

#[test]
fn incomplete_discovery_cannot_publish_readiness_or_add_verification_credit() {
    for mode in MODES {
        for http in [false, true] {
            let target = Target::new(mode, http);
            let path = manifest(&target.root);
            let mut store = mcpeval::store::Store::open(Some(target.root.clone())).unwrap();
            for session in ["first", "second"] {
                store.append(&serde_json::from_value(json!({"ts":"2026-08-05T00:00:01Z", "session":session,
                "seq":1,"server":"fixture","method":"tools/call","tool":"read_status","args":{},
                "latency_ms":1,"outcome":"error","shim_self_us":1,"kind":"real",
                "error":{"code":"broken","retryable":false,"template_id":"aaaaaaaaaaaaaaaa"}
            })).unwrap()).unwrap();
            }
            mcpeval::index::build(&target.root).unwrap();
            mcpeval::promote::promote(
                &target.root,
                mcpeval::promote::PromotionConfig {
                    threshold: 0.0,
                    now: chrono::Utc::now(),
                },
            )
            .unwrap();
            let db = rusqlite::Connection::open(target.root.join("index.db")).unwrap();
            let finding: String = db
                .query_row("SELECT finding_id FROM findings", [], |row| row.get(0))
                .unwrap();
            let result = target.run(&[
                "verify",
                "--finding",
                &finding,
                "--case",
                "read",
                "--manifest",
                path.to_str().unwrap(),
            ]);
            assert_eq!(
                result.status.code(),
                Some(3),
                "{mode} http={http}: {} {}",
                String::from_utf8_lossy(&result.stdout),
                String::from_utf8_lossy(&result.stderr)
            );
            target.assert_no_calls_or_payloads(&result);
            let evidence = rusqlite::Connection::open(target.root.join("lifecycle.db")).unwrap();
            let state: (String, i64, Option<String>) = evidence.query_row(
            "SELECT state,consecutive_passes,probe_id FROM finding_lifecycle WHERE finding_id=?1",
            [&finding], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?))).unwrap();
            assert_eq!(state, ("open".into(), 0, None));
            let history: i64 = evidence
                .query_row("SELECT COUNT(*) FROM probe_history", [], |row| row.get(0))
                .unwrap();
            assert_eq!(history, 0);

            let score = target.run(&["score", "--server", "fixture", "--format", "json"]);
            assert_eq!(score.status.code(), Some(3));
            let report: Value = serde_json::from_slice(&score.stdout).unwrap();
            assert!(report["readiness"].is_null());
            assert_eq!(report["readiness_error"], "transport-error");
            target.assert_no_calls_or_payloads(&score);
        }
    }
}
