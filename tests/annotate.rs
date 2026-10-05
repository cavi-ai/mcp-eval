use std::process::Command;

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_mcpeval")
}

fn tempdir() -> std::path::PathBuf {
    let base = std::env::temp_dir().join(format!("mcpeval-annotate-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&base).unwrap();
    base
}

#[test]
fn writes_an_annotation_record() {
    let home = tempdir();
    let out = Command::new(bin())
        .args([
            "annotate",
            "--session",
            "s1",
            "--seq",
            "7",
            "--kind",
            "false-success",
            "--note",
            "click reported success, nothing changed",
        ])
        .env("MCPEVAL_HOME", &home)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );

    let mut found = None;
    for entry in std::fs::read_dir(home.join("store")).unwrap() {
        let path = entry.unwrap().path();
        if path
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .starts_with("annotations-")
        {
            found = Some(std::fs::read_to_string(path).unwrap());
        }
    }
    let body = found.expect("an annotations file must exist");
    let value: serde_json::Value = serde_json::from_str(body.lines().next().unwrap()).unwrap();
    assert_eq!(value["kind"], "false-success");
    assert_eq!(value["seq"], 7);
    assert!(
        value["session"].as_str().unwrap().starts_with("session:"),
        "session must be hashed, got {}",
        value["session"]
    );
}

#[test]
fn event_target_is_persisted_without_legacy_coordinates_and_invalid_targets_do_not_write() {
    let home = tempdir();
    let event = uuid::Uuid::new_v4().to_string();
    let out = Command::new(bin())
        .args([
            "annotate",
            "--event-id",
            &event,
            "--kind",
            "workaround",
            "--note",
            "used fallback",
        ])
        .env("MCPEVAL_HOME", &home)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let path = std::fs::read_dir(home.join("store"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let body = std::fs::read_to_string(&path).unwrap();
    let record: serde_json::Value = serde_json::from_str(body.trim()).unwrap();
    assert_eq!(record["event_id"], event);
    assert!(record.get("session").is_none());
    assert!(record.get("seq").is_none());

    for target in [
        vec!["--event-id", &event, "--session", "s", "--seq", "1"],
        vec!["--session", "s"],
        vec!["--seq", "1"],
        vec!["--event-id", "not-a-uuid"],
        vec!["--event-id", "00000000-0000-0000-0000-000000000000"],
        vec!["--event-id", "00000000-0000-4000-0000-000000000000"],
    ] {
        let out = Command::new(bin())
            .arg("annotate")
            .args(target)
            .args(["--kind", "workaround", "--note", "n"])
            .env("MCPEVAL_HOME", &home)
            .output()
            .unwrap();
        assert!(!out.status.success());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), body);
    }
}

#[test]
fn rejects_an_unknown_kind() {
    let home = tempdir();
    let out = Command::new(bin())
        .args([
            "annotate",
            "--session",
            "s1",
            "--seq",
            "1",
            "--kind",
            "vibes",
            "--note",
            "n",
        ])
        .env("MCPEVAL_HOME", &home)
        .output()
        .unwrap();
    assert!(!out.status.success());
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains("false-success"),
        "error must list valid kinds: {err}"
    );
}

#[test]
fn rejects_an_overlong_or_multiline_note() {
    let home = tempdir();
    for note in [&"x".repeat(241), "line one\nline two"] {
        let out = Command::new(bin())
            .args([
                "annotate",
                "--session",
                "s1",
                "--seq",
                "1",
                "--kind",
                "workaround",
                "--note",
                note,
            ])
            .env("MCPEVAL_HOME", &home)
            .output()
            .unwrap();
        assert!(!out.status.success(), "note should be rejected: {note:?}");
    }
}

#[test]
fn store_coerces_an_unvalidated_unknown_kind_to_a_sentinel() {
    use mcpeval::record::AnnotationRecord;
    use mcpeval::store::Store;

    // `Store::append` re-validates `server`, `method`, and `tool` at the
    // store layer precisely because upstream validation is not trusted.
    // `kind` is the one field on `AnnotationRecord` that is not free-form by
    // design, so a direct `append_annotation` call that skips
    // `AnnotationRecord::validate` (as the CLI path always calls it first)
    // must not persist an unrecognized kind verbatim.
    let home = tempdir();
    let mut store = Store::open(Some(home.clone())).unwrap();
    store
        .append_annotation(&AnnotationRecord {
            ts: "2026-08-04T12:00:00Z".into(),
            event_id: None,
            session: Some("s1".into()),
            seq: Some(1),
            kind: "not-a-real-kind".into(),
            note: "n".into(),
        })
        .unwrap();

    let mut found = None;
    for entry in std::fs::read_dir(home.join("store")).unwrap() {
        let path = entry.unwrap().path();
        if path
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .starts_with("annotations-")
        {
            found = Some(std::fs::read_to_string(path).unwrap());
        }
    }
    let body = found.expect("an annotations file must exist");
    let value: serde_json::Value = serde_json::from_str(body.lines().next().unwrap()).unwrap();
    assert_eq!(
        value["kind"], "invalid",
        "an unrecognized kind must be coerced to a sentinel, not stored verbatim"
    );
}

#[test]
fn index_loads_annotations_and_links_them_to_calls() {
    use mcpeval::record::{AnnotationRecord, CallRecord};
    use mcpeval::store::Store;

    let home = tempdir();
    let mut store = Store::open(Some(home.clone())).unwrap();
    store
        .append(&CallRecord {
            identity: None,
            ts: "2026-08-04T12:00:00Z".into(),
            session: "s1".into(),
            seq: 7,
            server: "demo".into(),
            method: "tools/call".into(),
            tool: Some("click".into()),
            args: None,
            latency_ms: Some(5),
            outcome: "ok".into(),
            error: None,
            shim_self_us: 1,
            kind: "real".into(),
        })
        .unwrap();
    store
        .append_annotation(&AnnotationRecord {
            ts: "2026-08-04T12:00:01Z".into(),
            event_id: None,
            session: Some("s1".into()),
            seq: Some(7),
            kind: "false-success".into(),
            note: "reported success, nothing changed".into(),
        })
        .unwrap();

    mcpeval::index::build(&home).unwrap();
    let db = rusqlite::Connection::open(home.join("index.db")).unwrap();
    let linked: i64 = db
        .query_row(
            "SELECT COUNT(*) FROM annotations a JOIN calls c
             ON c.id = a.call_id",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(linked, 1);
}
