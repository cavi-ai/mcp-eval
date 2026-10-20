use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::Value;

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_mcpeval")
}

fn demo() -> &'static str {
    env!("CARGO_BIN_EXE_mcpeval-demo")
}

fn home() -> PathBuf {
    let path = std::env::temp_dir().join(format!("mcpeval-formats-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&path).unwrap();
    path
}

fn run(dir: &Path, args: &[&str]) -> Output {
    Command::new(bin())
        .args(args)
        .current_dir(dir)
        .env("MCPEVAL_HOME", dir)
        .output()
        .unwrap()
}

fn manifest(dir: &Path) {
    let output = run(
        dir,
        &[
            "init",
            "--server",
            "demo",
            "--confirm-read-only",
            "--output",
            "mcp-eval.manifest.json",
            "--",
            demo(),
        ],
    );
    assert_eq!(output.status.code(), Some(0));
}

/// Tests, failures, and errors of the root and of each suite, read with
/// Python's XML parser so the document is checked as well-formed XML.
fn junit_counts(dir: &Path, xml: &[u8]) -> String {
    let path = dir.join("report.xml");
    std::fs::write(&path, xml).unwrap();
    let output = Command::new("python3")
        .args([
            "-c",
            "import sys, xml.etree.ElementTree as E\n\
             root = E.parse(sys.argv[1]).getroot()\n\
             print(root.tag, root.get('tests'), root.get('failures'), root.get('errors'))\n\
             for suite in root:\n\
             \x20   print(suite.get('name'), suite.get('tests'), suite.get('failures'), suite.get('errors'))\n\
             \x20   for case in suite.iter('testcase'):\n\
             \x20       outcome = [child.tag + ':' + child.get('type') for child in case]\n\
             \x20       if outcome: print(' ', case.get('name'), *outcome)",
            path.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    String::from_utf8(output.stdout).unwrap()
}

#[test]
fn sarif_reports_lost_readiness_points_and_floor_shortfalls() {
    let dir = home();
    manifest(&dir);
    let output = run(
        &dir,
        &[
            "probe",
            "--server",
            "demo",
            "--format",
            "json",
            "--min-readiness",
            "85",
            "--",
            demo(),
            "--broken",
            "lying-errors",
        ],
    );
    assert_eq!(output.status.code(), Some(1));
    std::fs::write(dir.join("lying.json"), &output.stdout).unwrap();
    let output = run(&dir, &["report", "lying.json", "--format", "sarif"]);
    assert_eq!(output.status.code(), Some(1));
    let sarif: Value = serde_json::from_slice(&output.stdout).unwrap();
    let run = &sarif["runs"][0];
    let results = run["results"].as_array().unwrap();
    let readiness: Vec<&Value> = results
        .iter()
        .filter(|result| result["ruleId"].as_str().unwrap().starts_with("readiness/"))
        .collect();
    let document: Value =
        serde_json::from_slice(&std::fs::read(dir.join("lying.json")).unwrap()).unwrap();
    let lost = document["readiness"]["areas"]
        .as_array()
        .unwrap()
        .iter()
        .map(|area| area["checks"].as_array().unwrap().len())
        .sum::<usize>();
    assert_eq!(readiness.len(), lost + 1, "every lost point plus the floor");
    let floor = readiness
        .iter()
        .find(|result| result["ruleId"] == "readiness/floor")
        .unwrap();
    assert_eq!(floor["level"], "error");
    assert!(floor["message"]["text"]
        .as_str()
        .unwrap()
        .contains("score 76 < 85"));
    let honesty = readiness
        .iter()
        .find(|result| {
            result["ruleId"]
                .as_str()
                .unwrap()
                .starts_with("readiness/honesty-")
        })
        .unwrap();
    assert_eq!(honesty["level"], "warning");
    assert_eq!(
        honesty["locations"][0]["physicalLocation"]["artifactLocation"]["uri"],
        "mcp-eval.manifest.json"
    );
    assert_eq!(
        honesty["locations"][0]["physicalLocation"]["region"]["startLine"],
        1
    );
    let rules = run["tool"]["driver"]["rules"].as_array().unwrap();
    let rule = rules
        .iter()
        .find(|rule| rule["id"] == honesty["ruleId"])
        .unwrap();
    assert!(!rule["help"]["text"].as_str().unwrap().is_empty());
    let fingerprints: std::collections::BTreeSet<String> = readiness
        .iter()
        .map(|result| result["partialFingerprints"].to_string())
        .collect();
    assert_eq!(
        fingerprints.len(),
        readiness.len(),
        "fingerprints are unique"
    );
}

#[test]
fn junit_carries_cases_floors_and_unmeasured_readiness() {
    let dir = home();
    manifest(&dir);
    let output = run(
        &dir,
        &[
            "probe",
            "--server",
            "demo",
            "--format",
            "junit",
            "--min-readiness",
            "85",
            "--min-area",
            "protocol=90",
            "--",
            demo(),
            "--broken",
            "lying-errors",
        ],
    );
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        junit_counts(&dir, &output.stdout),
        "testsuites 28 1 0\n\
         demo.manifest 25 0 0\n\
         demo.readiness 3 1 0\n  \
         floor score >= 85 failure:readiness-floor\n"
    );
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(
        text.contains("<property name=\"score\" value=\"76\"/>"),
        "{text}"
    );
    assert!(text.contains("<system-out>lost "), "{text}");

    let output = run(
        &dir,
        &[
            "probe",
            "--server",
            "demo",
            "--format",
            "junit",
            "--",
            demo(),
            "--broken",
            "stalled-cursor",
        ],
    );
    assert_eq!(output.status.code(), Some(3));
    let counts = junit_counts(&dir, &output.stdout);
    assert!(
        counts.contains("pagination-stalled-cursor") || counts.contains("failure:"),
        "{counts}"
    );
    assert!(
        counts.contains("readiness error:transport-error"),
        "{counts}"
    );

    // A re-rendered document gives the same XML.
    let output = run(
        &dir,
        &[
            "probe",
            "--server",
            "demo",
            "--format",
            "json",
            "--",
            demo(),
        ],
    );
    std::fs::write(dir.join("clean.json"), &output.stdout).unwrap();
    let rendered = run(&dir, &["report", "clean.json", "--format", "junit"]);
    let direct = run(
        &dir,
        &[
            "probe",
            "--server",
            "demo",
            "--format",
            "junit",
            "--",
            demo(),
        ],
    );
    assert_eq!(rendered.status.code(), Some(0));
    let strip = |output: &Output| {
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .filter(|line| !line.contains("latency"))
            .collect::<Vec<_>>()
            .join("\n")
    };
    assert_eq!(strip(&rendered), strip(&direct));
}
