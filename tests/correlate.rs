use mcpeval::correlate::Correlator;
use mcpeval::fingerprint::Salt;
use serde_json::json;

#[test]
fn matches_a_response_to_its_request_and_measures_latency() {
    let mut c = Correlator::new("demo".into(), "sess".into(), Salt::for_tests());
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
    // "click" satisfies the identifier grammar, so it is kept even though
    // no tools/list ever declared it (Task 3: gate is grammar, not declaration).
    assert_eq!(rec.tool.as_deref(), Some("click"));
    assert_eq!(rec.latency_ms, Some(250));
    assert_eq!(rec.outcome, "ok");
    assert_eq!(rec.args.unwrap()["note"], "str<8");
}

#[test]
fn sessions_are_stable_opaque_tokens_and_unlisted_tools_are_not_persisted() {
    let mut first = Correlator::new(
        "demo".into(),
        "session secret /Users/a".into(),
        Salt::for_tests(),
    );
    let mut second = Correlator::new(
        "demo".into(),
        "session secret /Users/a".into(),
        Salt::for_tests(),
    );
    for c in [&mut first, &mut second] {
        c.on_outbound(&json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"CANARY?token=x","arguments":{}}}), 0);
    }
    let a = first
        .on_inbound(&json!({"jsonrpc":"2.0","id":1,"result":{}}), 1)
        .unwrap();
    let b = second
        .on_inbound(&json!({"jsonrpc":"2.0","id":1,"result":{}}), 1)
        .unwrap();
    assert_eq!(a.session, b.session);
    assert!(a.session.starts_with("session:"));
    assert!(!a.session.contains("secret"));
    assert_eq!(a.tool.as_deref(), Some("unlisted"));
}

#[test]
fn completed_call_carries_outbound_parse_and_forward_overhead() {
    let mut c = Correlator::new("demo".into(), "session".into(), Salt::for_tests());
    c.on_outbound_with_overhead(&json!({"jsonrpc":"2.0","id":1,"method":"ping"}), 0, 777);
    let record = c
        .on_inbound(&json!({"jsonrpc":"2.0","id":1,"result":{}}), 1)
        .unwrap();
    assert!(record.shim_self_us >= 777);
}

#[test]
fn an_error_response_records_code_and_template_only() {
    const SENSITIVE_MESSAGE: &str = "session 0be9b59c-af70-47b0-9169-d9de92330600 gone";

    let mut c = Correlator::new("demo".into(), "sess".into(), Salt::for_tests());
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
    let mut c = Correlator::new("demo".into(), "sess".into(), Salt::for_tests());
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
    let mut c = Correlator::new("demo".into(), "sess".into(), Salt::for_tests());
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
    let mut c = Correlator::new("demo".into(), "sess".into(), Salt::for_tests());
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

#[test]
fn inbound_server_request_with_colliding_id_does_not_consume_pending() {
    let mut c = Correlator::new("demo".into(), "sess".into(), Salt::for_tests());
    c.on_outbound(
        &json!({ "jsonrpc": "2.0", "id": 7, "method": "tools/call",
                 "params": { "name": "click", "arguments": {} } }),
        100,
    );

    let server_request = c.on_inbound(
        &json!({ "jsonrpc": "2.0", "id": 7, "method": "sampling/createMessage",
                 "params": { "messages": [] } }),
        110,
    );
    assert!(server_request.is_none());

    let response = c
        .on_inbound(
            &json!({ "jsonrpc": "2.0", "id": 7, "result": { "ok": true } }),
            125,
        )
        .expect("the actual response still matches the pending request");
    assert_eq!(response.method, "tools/call");
    assert_eq!(response.latency_ms, Some(25));
    assert_eq!(response.seq, 1);
}

#[test]
fn outbound_response_does_not_create_a_ghost_pending_request() {
    let mut c = Correlator::new("demo".into(), "sess".into(), Salt::for_tests());
    c.on_outbound(
        &json!({ "jsonrpc": "2.0", "id": 7, "result": { "accepted": true } }),
        10,
    );

    assert!(c
        .on_inbound(
            &json!({ "jsonrpc": "2.0", "id": 7, "result": { "ok": true } }),
            20,
        )
        .is_none());
}

#[test]
fn outbound_response_does_not_overwrite_a_pending_request() {
    let mut c = Correlator::new("demo".into(), "sess".into(), Salt::for_tests());
    c.on_outbound(&json!({ "jsonrpc": "2.0", "id": 7, "method": "ping" }), 10);
    c.on_outbound(
        &json!({ "jsonrpc": "2.0", "id": 7, "result": { "accepted": true } }),
        15,
    );

    let response = c
        .on_inbound(
            &json!({ "jsonrpc": "2.0", "id": 7, "result": { "ok": true } }),
            30,
        )
        .expect("the original request remains pending");
    assert_eq!(response.method, "ping");
    assert_eq!(response.latency_ms, Some(20));
}

#[test]
fn numeric_and_string_ids_do_not_collide() {
    let mut c = Correlator::new("demo".into(), "sess".into(), Salt::for_tests());
    c.on_outbound(
        &json!({ "jsonrpc": "2.0", "id": 7, "method": "numeric" }),
        10,
    );
    c.on_outbound(
        &json!({ "jsonrpc": "2.0", "id": "7", "method": "string" }),
        20,
    );

    let string_response = c
        .on_inbound(&json!({ "jsonrpc": "2.0", "id": "7", "result": {} }), 30)
        .unwrap();
    assert_eq!(string_response.method, "string");
    assert_eq!(string_response.latency_ms, Some(10));

    let numeric_response = c
        .on_inbound(&json!({ "jsonrpc": "2.0", "id": 7, "result": {} }), 40)
        .unwrap();
    assert_eq!(numeric_response.method, "numeric");
    assert_eq!(numeric_response.latency_ms, Some(30));
}

#[test]
fn an_undeclared_but_identifier_shaped_tool_name_is_kept() {
    let mut c = Correlator::new(
        "demo".into(),
        "sess".into(),
        mcpeval::fingerprint::Salt::for_tests(),
    );
    c.on_outbound(
        &serde_json::json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call",
                             "params": { "name": "never_listed_tool", "arguments": {} } }),
        0,
    );
    let rec = c
        .on_inbound(
            &serde_json::json!({ "jsonrpc": "2.0", "id": 1, "result": {} }),
            1,
        )
        .unwrap();
    assert_eq!(rec.tool.as_deref(), Some("never_listed_tool"));
}

#[test]
fn a_prose_shaped_tool_name_becomes_unlisted() {
    let mut c = Correlator::new(
        "demo".into(),
        "sess".into(),
        mcpeval::fingerprint::Salt::for_tests(),
    );
    c.on_outbound(
        &serde_json::json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call",
                             "params": { "name": "upload /Users/someone/private.pdf", "arguments": {} } }),
        0,
    );
    let rec = c
        .on_inbound(
            &serde_json::json!({ "jsonrpc": "2.0", "id": 1, "result": {} }),
            1,
        )
        .unwrap();
    assert_eq!(rec.tool.as_deref(), Some("unlisted"));
}

#[test]
fn unmatched_response_does_not_affect_pending_state_or_sequence() {
    let mut c = Correlator::new("demo".into(), "sess".into(), Salt::for_tests());
    c.on_outbound(&json!({ "jsonrpc": "2.0", "id": 7, "method": "ping" }), 10);

    assert!(c
        .on_inbound(&json!({ "jsonrpc": "2.0", "id": 99, "result": {} }), 20)
        .is_none());

    let matched = c
        .on_inbound(&json!({ "jsonrpc": "2.0", "id": 7, "result": {} }), 30)
        .expect("the original request remains pending");
    assert_eq!(matched.method, "ping");
    assert_eq!(matched.seq, 1);
    assert_eq!(matched.latency_ms, Some(20));
}
