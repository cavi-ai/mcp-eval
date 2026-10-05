use std::process::Command;

#[test]
fn provenance_corpus_places_only_observed_targets_and_rejects_inconsistent_population() {
    let home = std::env::temp_dir().join(format!("mcpeval-corpus-cli-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&home).unwrap();
    let fixture = include_str!("fixtures/corpus-v3.json");
    std::fs::write(home.join("corpus.json"), fixture).unwrap();
    let corpus = mcpeval::corpus::Corpus::load(&home.join("corpus.json")).unwrap();
    assert_eq!(corpus.placement(75).above, 1);
    assert_eq!(corpus.observations.len(), 1);
    let output = Command::new(env!("CARGO_BIN_EXE_mcpeval"))
        .args([
            "score",
            "--server",
            "fixture",
            "--",
            "python3",
            "tests/fixtures/probe_clean_server.py",
        ])
        .env("MCPEVAL_HOME", &home)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains(
        "standard corpus (mcpeval-standard/2): above 0, tied 0, below 1 of 1 observed servers"
    ));
    let mut valid: serde_json::Value = serde_json::from_str(fixture).unwrap();
    valid["observations"][0]["provenance"]["deployment_sha256"] = "a".repeat(64).into();
    valid["population"][1]["status"] = "errored".into();
    valid["population"][1]["reason"] = "deployment-mismatch".into();
    std::fs::write(home.join("corpus.json"), valid.to_string()).unwrap();
    assert!(mcpeval::corpus::Corpus::load(&home.join("corpus.json")).is_ok());
    valid["observations"][0]["provenance"]["state_checks"] =
        serde_json::json!([{ "name": "backend", "sha256": "b".repeat(64) }]);
    valid["observations"][0]["provenance"]["prerequisites"] = serde_json::json!(["state:backend"]);
    for reason in ["state-mismatch", "state-check-failed"] {
        valid["population"][1]["reason"] = reason.into();
        std::fs::write(home.join("corpus.json"), valid.to_string()).unwrap();
        assert!(mcpeval::corpus::Corpus::load(&home.join("corpus.json")).is_ok());
    }
    for mutation in [
        "population",
        "report",
        "calls",
        "version",
        "score",
        "deployment",
        "state-digest",
        "state-duplicate",
        "state-prerequisite",
        "state-missing",
    ] {
        let mut invalid = valid.clone();
        match mutation {
            "population" => invalid["population"][0]["status"] = "untested".into(),
            "report" => {
                invalid["observations"][0]["provenance"]["report_sha256"] = "missing".into()
            }
            "calls" => invalid["observations"][0]["calls"]["tool_errors"] = 0.into(),
            "version" => invalid["observations"][0]["provenance"]["version"] = "latest".into(),
            "score" => invalid["observations"][0]["score"] = 101.into(),
            "deployment" => {
                invalid["observations"][0]["provenance"]["deployment_sha256"] = "missing".into()
            }
            "state-digest" => {
                invalid["observations"][0]["provenance"]["state_checks"][0]["sha256"] =
                    "invalid".into()
            }
            "state-duplicate" => {
                invalid["observations"][0]["provenance"]["state_checks"] = serde_json::json!([
                    { "name": "backend", "sha256": "b".repeat(64) },
                    { "name": "backend", "sha256": "c".repeat(64) }
                ])
            }
            "state-prerequisite" => {
                invalid["observations"][0]["provenance"]["prerequisites"] = serde_json::json!([])
            }
            "state-missing" => {
                invalid["observations"][0]["provenance"]["state_checks"] = serde_json::json!([])
            }
            _ => unreachable!(),
        }
        std::fs::write(home.join("corpus.json"), invalid.to_string()).unwrap();
        assert!(
            mcpeval::corpus::Corpus::load(&home.join("corpus.json")).is_err(),
            "{mutation}"
        );
    }
    std::fs::remove_dir_all(home).unwrap();
}
