use chrono::{TimeZone, Utc};
use mcpeval::index;
use mcpeval::promote::{promote, PromotionConfig};
use mcpeval::record::{AnnotationRecord, CallRecord, ErrorInfo};
use mcpeval::report::{export_issues, load_findings, render, ReportFormat};
use mcpeval::store::Store;
use serde_json::json;

fn tempdir() -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!("mcpeval-report-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&path).unwrap();
    path
}

fn failure(session: &str, seq: u64, tool: &str, id: &str) -> CallRecord {
    CallRecord {
        ts: format!("2026-08-05T00:00:{seq:02}Z"),
        session: session.into(),
        seq,
        server: "demo".into(),
        method: "tools/call".into(),
        tool: Some(tool.into()),
        args: Some(json!({"shape": {"target": "str<32"}})),
        latency_ms: Some(1),
        outcome: "error".into(),
        error: Some(ErrorInfo {
            code: Some(json!("blocked")),
            layer: None,
            retryable: Some(false),
            kind: None,
            template: Some("CANARY raw template /Users/private".into()),
            template_id: Some(id.into()),
        }),
        shim_self_us: 1,
        kind: "real".into(),
    }
}

fn promoted_home() -> std::path::PathBuf {
    let dir = tempdir();
    let mut store = Store::open(Some(dir.clone())).unwrap();
    for record in [
        failure("raw-session-one", 1, "click", "aaaaaaaaaaaaaaaa"),
        failure("raw-session-two", 1, "click", "aaaaaaaaaaaaaaaa"),
        failure("raw-session-one", 2, "type", "bbbbbbbbbbbbbbbb"),
        failure("raw-session-two", 2, "type", "bbbbbbbbbbbbbbbb"),
    ] {
        store.append(&record).unwrap();
    }
    store
        .append_annotation(&AnnotationRecord {
            ts: "2026-08-05T00:00:00Z".into(),
            session: "raw-session-one".into(),
            seq: 1,
            kind: "false-success".into(),
            note: "CANARY private annotation token=secret".into(),
        })
        .unwrap();
    index::build(&dir).unwrap();
    promote(
        &dir,
        PromotionConfig {
            threshold: 0.8,
            now: Utc.with_ymd_and_hms(2026, 8, 5, 1, 0, 0).unwrap(),
        },
    )
    .unwrap();
    dir
}

#[test]
fn report_requires_promotion_to_have_run() {
    let dir = tempdir();
    Store::open(Some(dir.clone())).unwrap();
    assert!(load_findings(&dir)
        .unwrap_err()
        .to_string()
        .contains("mcpeval promote"));
}

#[test]
fn report_json_is_deterministic_and_contains_actionable_safe_metrics() {
    let dir = promoted_home();
    let text = render(&load_findings(&dir).unwrap(), ReportFormat::Json).unwrap();
    let rows: serde_json::Value = serde_json::from_str(&text).unwrap();
    let rows = rows.as_array().unwrap();
    assert_eq!(rows.len(), 2);
    assert!(rows[0]["score"].as_f64().unwrap() >= rows[1]["score"].as_f64().unwrap());
    assert_eq!(rows[0]["server"], "demo");
    assert!(rows[0]["failures"].as_u64().unwrap() >= 2);
    assert!(rows[0]["cost"].as_f64().unwrap() >= 1.0);
    assert!(text.contains("str<32"));
}

#[test]
fn report_formats_never_emit_raw_templates_notes_sessions_or_paths() {
    let dir = promoted_home();
    for format in [ReportFormat::Agent, ReportFormat::Md, ReportFormat::Json] {
        let text = render(&load_findings(&dir).unwrap(), format).unwrap();
        for forbidden in [
            "CANARY",
            "private annotation",
            "raw-session",
            "/Users/private",
            "token=secret",
            "raw template",
        ] {
            assert!(
                !text.contains(forbidden),
                "{format:?} leaked {forbidden}: {text}"
            );
        }
    }
}

#[test]
fn report_agent_and_markdown_formats_are_focused_and_annotation_uplifts_severity() {
    let dir = promoted_home();
    let agent = render(&load_findings(&dir).unwrap(), ReportFormat::Agent).unwrap();
    let markdown = render(&load_findings(&dir).unwrap(), ReportFormat::Md).unwrap();
    assert!(agent.contains(" demo/click state=open probe=none"));
    assert!(agent.contains("severity=medium"));
    for field in ["rate=", "confidence=", "recency=", "threshold="] {
        assert!(
            agent.contains(field),
            "agent report omitted {field}: {agent}"
        );
    }
    assert!(markdown.contains("# MCP Evaluation Findings"));
    assert!(markdown.contains("## demo / click"));
    for field in [
        "Rate:",
        "Confidence:",
        "Recency:",
        "Error code:",
        "Template ID:",
    ] {
        assert!(
            markdown.contains(field),
            "Markdown report omitted {field}: {markdown}"
        );
    }
}

#[test]
fn unprobeable_findings_are_capped_at_medium() {
    let dir = promoted_home();
    promote(
        &dir,
        PromotionConfig {
            threshold: 0.0,
            now: Utc.with_ymd_and_hms(2026, 8, 5, 1, 0, 0).unwrap(),
        },
    )
    .unwrap();
    let agent = render(&load_findings(&dir).unwrap(), ReportFormat::Agent).unwrap();
    assert!(agent.lines().all(|line| !line.contains("severity=high")));
    assert!(agent.lines().any(|line| line.contains("severity=medium")));
}

#[test]
fn every_format_carries_the_class_and_hint_and_issue_files_add_a_diagnosis() {
    let dir = promoted_home();
    let findings = load_findings(&dir).unwrap();
    let agent = render(&findings, ReportFormat::Agent).unwrap();
    let click = agent
        .lines()
        .find(|line| line.contains(" demo/click "))
        .unwrap();
    assert!(click.ends_with(" class=false-success"), "{click}");
    assert!(
        agent.contains(
            "\n  hint: the call reported success while the agent observed no effect; return a structured error when the operation did not happen\n"
        ),
        "{agent}"
    );
    assert!(agent
        .lines()
        .any(|line| line.contains(" demo/type ") && line.ends_with(" class=recurring-error")));
    assert!(!agent.contains("codes=["), "{agent}");

    let markdown = render(&findings, ReportFormat::Md).unwrap();
    assert!(
        markdown.contains("- Class: false-success\n- Hint: the call reported success"),
        "{markdown}"
    );
    assert!(markdown.contains("- Class: recurring-error\n- Hint: the same structured error"));
    assert!(!markdown.contains("- Error codes:"));

    let rows: serde_json::Value =
        serde_json::from_str(&render(&findings, ReportFormat::Json).unwrap()).unwrap();
    let mut classes: Vec<&str> = rows
        .as_array()
        .unwrap()
        .iter()
        .map(|row| {
            assert_eq!(row["err_codes"], json!(["blocked"]));
            assert!(!row["hint"].as_str().unwrap().is_empty());
            row["class"].as_str().unwrap()
        })
        .collect();
    classes.sort_unstable();
    assert_eq!(classes, ["false-success", "recurring-error"]);

    let issues = tempdir().join("issues");
    assert_eq!(export_issues(&dir, &issues, false, false).unwrap(), 2);
    for entry in std::fs::read_dir(&issues).unwrap() {
        let body = std::fs::read_to_string(entry.unwrap().path()).unwrap();
        let diagnosis = body.find("\n## Diagnosis\n\n- Class: `").unwrap();
        let hint = body.find("\n- Hint: ").unwrap();
        let next = body.find("\n## Suggested next steps").unwrap();
        assert!(diagnosis < hint && hint < next, "{body}");
    }
}

#[test]
fn several_codes_render_in_every_format() {
    let dir = tempdir();
    let mut store = Store::open(Some(dir.clone())).unwrap();
    for (session, code) in [("s1", -32002), ("s2", -32001)] {
        let mut record = failure(session, 1, "click", "aaaaaaaaaaaaaaaa");
        record.error.as_mut().unwrap().code = Some(json!(code));
        store.append(&record).unwrap();
    }
    index::build(&dir).unwrap();
    promote(
        &dir,
        PromotionConfig {
            threshold: 0.0,
            now: Utc.with_ymd_and_hms(2026, 8, 5, 1, 0, 0).unwrap(),
        },
    )
    .unwrap();
    let findings = load_findings(&dir).unwrap();
    let agent = render(&findings, ReportFormat::Agent).unwrap();
    assert!(
        agent.contains(
            "\n  cause code=-32001 codes=[-32001,-32002] template_id=aaaaaaaaaaaaaaaa last_seen="
        ),
        "{agent}"
    );
    assert!(agent
        .lines()
        .next()
        .unwrap()
        .ends_with(" class=unstable-error-code"));
    let markdown = render(&findings, ReportFormat::Md).unwrap();
    assert!(
        markdown.contains("- Error codes: [-32001,-32002]\n- Class: unstable-error-code\n"),
        "{markdown}"
    );
    let rows: serde_json::Value =
        serde_json::from_str(&render(&findings, ReportFormat::Json).unwrap()).unwrap();
    assert_eq!(rows[0]["err_code"], json!(-32001));
    assert_eq!(rows[0]["err_codes"], json!([-32001, -32002]));
}
