use mcpeval::fingerprint::Salt;
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
    let info = error_info(&payload, &Salt::for_tests());

    // "browserCommandFailed" is an identifier-shaped code, so it is kept
    // verbatim rather than bucketed; see identifier_error_codes_are_kept_and_prose_codes_are_bucketed.
    assert_eq!(info.code, Some(json!("browserCommandFailed")));
    assert_eq!(info.layer.as_deref(), Some("str<32"));
    assert_eq!(info.retryable, Some(false));
    assert_eq!(info.kind.as_deref(), Some("str<32"));
    assert_eq!(info.template.as_deref(), Some("{message}"));
    assert_eq!(info.template_id.as_deref().map(str::len), Some(16));

    let text = serde_json::to_string(&info).unwrap();
    assert_safe_error_text(&text);
    let serialized: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(serialized.as_object().unwrap().len(), 6);
}

#[test]
fn error_info_keeps_scalar_codes_and_reduces_composites_to_container_shape() {
    for scalar in [json!(null), json!(false), json!(404)] {
        let info = error_info(&json!({ "code": scalar.clone() }), &Salt::for_tests());
        assert_eq!(info.code, Some(scalar));
    }

    let array = error_info(
        &json!({
            "code": ["array-code-canary", { "nested-key-canary": "nested-value-canary" }]
        }),
        &Salt::for_tests(),
    );
    assert_eq!(array.code, Some(json!({ "array": 2 })));

    let object = error_info(
        &json!({
            "code": {
                "object-key-canary": "object-value-canary",
                "nested-object-canary": { "secret-key-canary": "secret-value-canary" }
            }
        }),
        &Salt::for_tests(),
    );
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
    rec.error = Some(error_info(&json!({ "code": null }), &Salt::for_tests()));

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
    rec.error = Some(error_info(&sensitive_error_payload(), &Salt::for_tests()));

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
        template_id: None,
    };

    let text = serde_json::to_string(&direct).unwrap();
    // "directCodeCanary" is identifier-shaped, so it is expected to survive
    // serialization verbatim; only the non-identifier fields must be scrubbed.
    for canary in [
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
    assert_eq!(value["code"], "directCodeCanary");
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
fn template_id_survives_serialization_only_when_it_is_a_valid_fingerprint() {
    let valid = ErrorInfo {
        template_id: Some("0123456789abcdef".into()),
        ..Default::default()
    };
    let text = serde_json::to_string(&valid).unwrap();
    let value: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(value["template_id"], "0123456789abcdef");

    for bogus in [
        "short",
        "0123456789abcdeg",  // wrong length is fine, but this char isn't hex
        "0123456789ABCDEF",  // uppercase hex is not "lowercase hex"
        "0123456789abcdef0", // 17 chars, too long
        "the raw message leaked here!!!",
    ] {
        let direct = ErrorInfo {
            template_id: Some(bogus.into()),
            ..Default::default()
        };
        let text = serde_json::to_string(&direct).unwrap();
        let value: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert!(
            value["template_id"].is_null(),
            "invalid template_id was not dropped: {bogus}"
        );
        assert!(
            !text.contains(bogus),
            "leaked invalid template_id text: {bogus}"
        );
    }
}

#[test]
fn a_rejected_template_id_is_omitted_not_serialized_as_explicit_null() {
    // M8: `value["template_id"].is_null()` is also true when the key is
    // simply absent, so that assertion alone can't tell "omitted" from
    // "present as null". Every other dropped field is omitted, not written
    // as `null`; this must match.
    let direct = ErrorInfo {
        template_id: Some("the raw message leaked here!!!".into()),
        ..Default::default()
    };
    let text = serde_json::to_string(&direct).unwrap();
    assert!(
        !text.contains("template_id"),
        "an invalid template_id must be omitted entirely, not serialized as null: {text}"
    );
    let value: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert!(
        !value.as_object().unwrap().contains_key("template_id"),
        "template_id key must be absent, not present with a null value"
    );
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

#[test]
fn store_sanitizes_every_directly_constructed_persistence_field() {
    let dir = tempdir();
    let canary = "CANARY /Users/private?token=secret";
    let mut rec = sample(10);
    rec.ts = canary.into();
    rec.args = Some(json!({"path": canary, "header": canary, "count": 42}));
    rec.outcome = canary.into();
    rec.kind = canary.into();
    let mut store = Store::open(Some(dir.clone())).unwrap();
    store.append(&rec).unwrap();

    let paths: Vec<_> = std::fs::read_dir(dir.join("store"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    assert_eq!(paths.len(), 1);
    assert_eq!(paths[0].file_name().unwrap(), "calls-unknown.jsonl");
    let body = std::fs::read_to_string(&paths[0]).unwrap();
    assert!(!body.contains("CANARY"));
    assert!(!body.contains("/Users/private"));
    assert!(!body.contains("token=secret"));
    let stored: serde_json::Value = serde_json::from_str(body.trim()).unwrap();
    assert_eq!(stored["ts"], "unknown");
    assert_eq!(stored["args"]["path"], "str<128");
    assert_eq!(stored["args"]["count"], "num:42");
    assert_eq!(stored["outcome"], "unknown");
    assert_eq!(stored["kind"], "unparsed");
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn synthetic_probe_records_keep_their_kind_without_persisting_canaries() {
    let dir = tempdir();
    let mut rec = sample(11);
    rec.kind = "synthetic".into();
    rec.args = Some(json!({"secret": "CANARY-argument"}));
    rec.error = Some(ErrorInfo {
        code: Some(json!("CANARY error prose")),
        layer: Some("CANARY-layer".into()),
        retryable: Some(false),
        kind: Some("CANARY-kind".into()),
        template: Some("CANARY-template".into()),
        template_id: Some("not-a-fingerprint".into()),
    });
    let mut store = Store::open(Some(dir.clone())).unwrap();
    store.append(&rec).unwrap();

    let path = std::fs::read_dir(dir.join("store"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let body = std::fs::read_to_string(path).unwrap();
    assert!(!body.contains("CANARY"), "synthetic record leaked: {body}");
    let stored: serde_json::Value = serde_json::from_str(body.trim()).unwrap();
    assert_eq!(stored["kind"], "synthetic");
    assert_eq!(stored["args"]["secret"], "str<32");
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn identifier_error_codes_are_kept_and_prose_codes_are_bucketed() {
    use mcpeval::fingerprint::Salt;
    use serde_json::json;

    let salt = Salt::for_tests();
    let identifier = mcpeval::record::error_info(
        &json!({ "code": "browserCommandFailed", "message": "boom" }),
        &salt,
    );
    assert_eq!(identifier.code.unwrap(), json!("browserCommandFailed"));

    let prose = mcpeval::record::error_info(
        &json!({ "code": "Cannot upload /Users/someone/private.pdf", "message": "boom" }),
        &salt,
    );
    let stored = serde_json::to_string(&prose).unwrap();
    assert!(!stored.contains("private"), "prose code leaked: {stored}");
    assert!(!stored.contains("Users"), "prose code leaked: {stored}");
}

#[test]
fn error_info_carries_a_fingerprint_and_still_hides_the_message() {
    use mcpeval::fingerprint::Salt;
    use serde_json::json;

    let info = mcpeval::record::error_info(
        &json!({ "code": -32000, "message": "session 0be9b59c-af70-47b0-9169-d9de92330600 gone" }),
        &Salt::for_tests(),
    );
    assert_eq!(info.template.as_deref(), Some("{message}"));

    // Serialize while `info` is still whole; `template_id.unwrap()` below
    // would otherwise partially move it out from under this borrow.
    let stored = serde_json::to_string(&info).unwrap();
    assert!(!stored.contains("gone"));
    assert!(!stored.contains("0be9b59c"));

    let id = info.template_id.unwrap();
    assert_eq!(id.len(), 16);
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
    // "browserCommandFailed" is deliberately excluded: it is an
    // identifier-shaped code, which this task keeps verbatim rather than
    // bucketing (see identifier_error_codes_are_kept_and_prose_codes_are_bucketed).
    for canary in [
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
