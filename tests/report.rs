use chrono::{TimeZone, Utc};
use mcpeval::index;
use mcpeval::promote::{promote, PromotionConfig};
use mcpeval::record::{AnnotationRecord, CallRecord, ErrorInfo};
use mcpeval::report::{render, ReportFormat};
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
    assert!(render(&dir, ReportFormat::Json)
        .unwrap_err()
        .to_string()
        .contains("mcpeval promote"));
}

#[test]
fn report_json_is_deterministic_and_contains_actionable_safe_metrics() {
    let dir = promoted_home();
    let text = render(&dir, ReportFormat::Json).unwrap();
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
        let text = render(&dir, format).unwrap();
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
    let agent = render(&dir, ReportFormat::Agent).unwrap();
    let markdown = render(&dir, ReportFormat::Md).unwrap();
    assert!(agent.contains("finding demo/click"));
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
fn zero_threshold_uses_the_mathematical_relative_bands() {
    let dir = promoted_home();
    promote(
        &dir,
        PromotionConfig {
            threshold: 0.0,
            now: Utc.with_ymd_and_hms(2026, 8, 5, 1, 0, 0).unwrap(),
        },
    )
    .unwrap();
    let agent = render(&dir, ReportFormat::Agent).unwrap();
    assert!(agent.lines().any(|line| line.contains("severity=high")));
}
