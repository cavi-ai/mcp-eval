use mcpeval::correlate::Correlator;
use serde_json::json;

#[test]
fn matches_a_response_to_its_request_and_measures_latency() {
    let mut c = Correlator::new("demo".into(), "sess".into());
    c.on_outbound(
        &json!({ "jsonrpc": "2.0", "id": 7, "method": "tools/call",
                 "params": { "name": "click", "arguments": { "note": "hello" } } }),
        1_000,
    );
    let rec = c
        .on_inbound(
            &json!({ "jsonrpc": "2.0", "id": 7, "result": { "ok": true } }),
            1_250,
        )
        .expect("a matched response emits a record");

    assert_eq!(rec.method, "tools/call");
    assert_eq!(rec.tool.as_deref(), Some("click"));
    assert_eq!(rec.latency_ms, Some(250));
    assert_eq!(rec.outcome, "ok");
    assert_eq!(rec.args.unwrap()["note"], "str<8");
}

#[test]
fn an_error_response_records_code_and_template_only() {
    const SENSITIVE_MESSAGE: &str = "session 0be9b59c-af70-47b0-9169-d9de92330600 gone";

    let mut c = Correlator::new("demo".into(), "sess".into());
    c.on_outbound(
        &json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call",
                 "params": { "name": "navigate", "arguments": {} } }),
        0,
    );
    let rec = c
        .on_inbound(
            &json!({ "jsonrpc": "2.0", "id": 1,
                     "error": { "code": -32000, "message": SENSITIVE_MESSAGE } }),
            10,
        )
        .unwrap();

    assert_eq!(rec.outcome, "error");
    let err = rec.error.as_ref().unwrap();
    assert_eq!(err.code.as_ref().unwrap(), &json!(-32000));
    assert_eq!(err.template.as_deref(), Some("{message}"));

    let serialized = serde_json::to_string(&rec).unwrap();
    assert!(!serialized.contains(SENSITIVE_MESSAGE));
}

#[test]
fn tools_list_teaches_enums_used_by_later_calls() {
    let mut c = Correlator::new("demo".into(), "sess".into());
    c.on_outbound(
        &json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list" }),
        0,
    );
    c.on_inbound(
        &json!({ "jsonrpc": "2.0", "id": 1, "result": { "tools": [
            { "name": "navigate", "inputSchema": { "type": "object", "properties": {
                "waitUntil": { "type": "string", "enum": ["commit", "networkIdle"] } } } }
        ] } }),
        5,
    );

    c.on_outbound(
        &json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/call",
                 "params": { "name": "navigate", "arguments": { "waitUntil": "networkIdle" } } }),
        6,
    );
    let rec = c
        .on_inbound(&json!({ "jsonrpc": "2.0", "id": 2, "result": {} }), 7)
        .unwrap();
    assert_eq!(rec.args.unwrap()["waitUntil"], "enum:networkIdle");
}

#[test]
fn notifications_emit_immediately() {
    let mut c = Correlator::new("demo".into(), "sess".into());
    let rec = c
        .on_inbound(
            &json!({ "jsonrpc": "2.0", "method": "notifications/message" }),
            3,
        )
        .expect("a notification emits its own record");
    assert_eq!(rec.outcome, "notification");
    assert_eq!(rec.method, "notifications/message");
}

#[test]
fn sequence_numbers_increase_per_record() {
    let mut c = Correlator::new("demo".into(), "sess".into());
    c.on_outbound(&json!({ "jsonrpc": "2.0", "id": 1, "method": "ping" }), 0);
    let a = c
        .on_inbound(&json!({ "jsonrpc": "2.0", "id": 1, "result": {} }), 1)
        .unwrap();
    c.on_outbound(&json!({ "jsonrpc": "2.0", "id": 2, "method": "ping" }), 2);
    let b = c
        .on_inbound(&json!({ "jsonrpc": "2.0", "id": 2, "result": {} }), 3)
        .unwrap();
    assert!(b.seq > a.seq);
}
