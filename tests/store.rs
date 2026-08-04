use mcpeval::record::{error_info, CallRecord, ErrorInfo};
use mcpeval::store::Store;
use serde_json::json;
use std::collections::HashSet;
use std::sync::{Arc, Barrier};

fn sample(seq: u64) -> CallRecord {
    CallRecord {
        ts: "2026-08-04T12:00:00Z".into(),
        session: "11111111-1111-4111-8111-111111111111".into(),
        seq,
        server: "demo".into(),
        method: "tools/call".into(),
        tool: Some("click".into()),
        args: Some(json!({ "sessionId": "uuid" })),
        latency_ms: Some(12),
        outcome: "ok".into(),
        error: None,
        shim_self_us: 40,
        kind: "real".into(),
    }
}

#[test]
fn appends_one_json_line_per_record() {
    let dir = tempdir();
    let mut store = Store::open(Some(dir.clone())).unwrap();
    store.append(&sample(1)).unwrap();
    store.append(&sample(2)).unwrap();

    let files: Vec<_> = std::fs::read_dir(dir.join("store")).unwrap().collect();
    assert_eq!(files.len(), 1, "one daily file expected");

    let path = files.into_iter().next().unwrap().unwrap().path();
    let body = std::fs::read_to_string(path).unwrap();
    let lines: Vec<&str> = body.lines().collect();
    assert_eq!(lines.len(), 2);
    let first: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
    assert_eq!(first["seq"], 1);
    assert_eq!(first["tool"], "click");

    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn concurrent_store_writers_produce_complete_json_lines() {
    const WRITERS: usize = 16;
    const RECORDS_PER_WRITER: usize = 1_000;

    let dir = tempdir();
    let barrier = Arc::new(Barrier::new(WRITERS));
    let handles: Vec<_> = (0..WRITERS)
        .map(|writer| {
            let dir = dir.clone();
            let barrier = Arc::clone(&barrier);
            std::thread::spawn(move || {
                let mut store = Store::open(Some(dir)).unwrap();
                barrier.wait();
                for offset in 0..RECORDS_PER_WRITER {
                    let seq = (writer * RECORDS_PER_WRITER + offset) as u64;
                    store.append(&sample(seq)).unwrap();
                }
            })
        })
        .collect();

    for handle in handles {
        handle.join().unwrap();
    }

    let path = std::fs::read_dir(dir.join("store"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let body = std::fs::read_to_string(path).unwrap();
    let records: Vec<CallRecord> = body
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(records.len(), WRITERS * RECORDS_PER_WRITER);
    let sequences: HashSet<_> = records.into_iter().map(|record| record.seq).collect();
    assert_eq!(sequences.len(), WRITERS * RECORDS_PER_WRITER);

    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn error_info_buckets_strings_and_drops_unapproved_keys() {
    let payload = sensitive_error_payload();
    let info = error_info(&payload);

    assert_eq!(info.code, Some(json!("str<32")));
    assert_eq!(info.layer.as_deref(), Some("str<32"));
    assert_eq!(info.retryable, Some(false));
    assert_eq!(info.kind.as_deref(), Some("str<32"));
    assert_eq!(info.template.as_deref(), Some("{message}"));

    let text = serde_json::to_string(&info).unwrap();
    assert_safe_error_text(&text);
    let serialized: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(serialized.as_object().unwrap().len(), 5);
}

#[test]
fn error_info_keeps_scalar_codes_and_reduces_composites_to_container_shape() {
    for scalar in [json!(null), json!(false), json!(404)] {
        let info = error_info(&json!({ "code": scalar.clone() }));
        assert_eq!(info.code, Some(scalar));
    }

    let array = error_info(&json!({
        "code": ["array-code-canary", { "nested-key-canary": "nested-value-canary" }]
    }));
    assert_eq!(array.code, Some(json!({ "array": 2 })));

    let object = error_info(&json!({
        "code": {
            "object-key-canary": "object-value-canary",
            "nested-object-canary": { "secret-key-canary": "secret-value-canary" }
        }
    }));
    assert_eq!(object.code, Some(json!({ "object": 2 })));

    let text = serde_json::to_string(&(array, object)).unwrap();
    for canary in [
        "array-code-canary",
        "nested-key-canary",
        "nested-value-canary",
        "object-key-canary",
        "object-value-canary",
        "nested-object-canary",
        "secret-key-canary",
        "secret-value-canary",
    ] {
        assert!(
            !text.contains(canary),
            "leaked composite-code canary: {canary}"
        );
    }
}

#[test]
fn persisted_call_record_round_trip_distinguishes_null_from_missing_error_code() {
    let dir = tempdir();
    let mut rec = sample(5);
    rec.error = Some(error_info(&json!({ "code": null })));

    let mut store = Store::open(Some(dir.clone())).unwrap();
    store.append(&rec).unwrap();

    let path = std::fs::read_dir(dir.join("store"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let body = std::fs::read_to_string(path).unwrap();
    let persisted: CallRecord = serde_json::from_str(body.trim_end()).unwrap();
    assert_eq!(persisted.error.unwrap().code, Some(serde_json::Value::Null));
    let missing: ErrorInfo = serde_json::from_str("{}").unwrap();
    assert_eq!(missing.code, None);

    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn jsonl_record_does_not_persist_error_canaries() {
    let dir = tempdir();
    let mut rec = sample(3);
    rec.outcome = "error".into();
    rec.error = Some(error_info(&sensitive_error_payload()));

    let mut store = Store::open(Some(dir.clone())).unwrap();
    store.append(&rec).unwrap();

    let path = std::fs::read_dir(dir.join("store"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let body = std::fs::read_to_string(path).unwrap();
    assert_safe_error_text(&body);

    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn serialization_sanitizes_directly_constructed_error_info() {
    let direct = ErrorInfo {
        code: Some(json!("directCodeCanary")),
        layer: Some("directLayerCanary".into()),
        retryable: Some(true),
        kind: Some("directKindCanary".into()),
        template: Some("directMessageCanary".into()),
    };

    let text = serde_json::to_string(&direct).unwrap();
    for canary in [
        "directCodeCanary",
        "directLayerCanary",
        "directKindCanary",
        "directMessageCanary",
    ] {
        assert!(
            !text.contains(canary),
            "leaked direct-construction canary: {canary}"
        );
    }
    let value: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(value["code"], "str<32");
    assert_eq!(value["layer"], "str<32");
    assert_eq!(value["kind"], "str<32");
    assert_eq!(value["template"], "{message}");

    let dir = tempdir();
    let mut rec = sample(4);
    rec.error = Some(direct);
    let mut store = Store::open(Some(dir.clone())).unwrap();
    store.append(&rec).unwrap();
    let path = std::fs::read_dir(dir.join("store"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let body = std::fs::read_to_string(path).unwrap();
    for canary in [
        "directCodeCanary",
        "directLayerCanary",
        "directKindCanary",
        "directMessageCanary",
    ] {
        assert!(
            !body.contains(canary),
            "persisted direct-construction canary: {canary}"
        );
    }
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn store_sanitizes_directly_constructed_identifier_fields() {
    let dir = tempdir();
    let canary = "CANARY /Users/private?token=secret";
    let mut rec = sample(9);
    rec.session = canary.into();
    rec.server = canary.into();
    rec.method = canary.into();
    rec.tool = Some(canary.into());
    let mut store = Store::open(Some(dir.clone())).unwrap();
    store.append(&rec).unwrap();

    let path = std::fs::read_dir(dir.join("store"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let body = std::fs::read_to_string(path).unwrap();
    assert!(!body.contains("CANARY") && !body.contains("private") && !body.contains("token"));
    let stored: serde_json::Value = serde_json::from_str(body.trim()).unwrap();
    assert!(stored["session"].as_str().unwrap().starts_with("session:"));
    assert_eq!(stored["server"], "invalid");
    assert_eq!(stored["method"], "unparsed/metadata");
    assert!(stored.get("tool").is_none());
    std::fs::remove_dir_all(dir).unwrap();
}

fn sensitive_error_payload() -> serde_json::Value {
    json!({
        "error": {
            "code": "browserCommandFailed",
            "layer": "driverCanary",
            "retryable": false,
            "kind": "transportCanary",
            "message": "message-canary 0be9b59c-af70-47b0-9169-d9de92330600",
            "correlationId": "correlation-canary-9c27f579",
            "stack": "stack-canary-secret-internals"
        }
    })
}

fn assert_safe_error_text(text: &str) {
    for canary in [
        "browserCommandFailed",
        "driverCanary",
        "transportCanary",
        "message-canary",
        "0be9b59c-af70-47b0-9169-d9de92330600",
        "correlation-canary-9c27f579",
        "stack-canary-secret-internals",
    ] {
        assert!(!text.contains(canary), "leaked error canary: {canary}");
    }
    assert!(!text.contains("correlationId"));
    assert!(!text.contains("stack"));
}

fn tempdir() -> std::path::PathBuf {
    let base = std::env::temp_dir().join(format!("mcpeval-test-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&base).unwrap();
    base
}
