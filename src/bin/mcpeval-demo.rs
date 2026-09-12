//! A tiny, self-contained MCP stdio server for trying `mcpeval` end to end
//! without any other infrastructure.
//!
//! `mcpeval-demo` speaks newline-delimited JSON-RPC and ships two personalities:
//!
//! - **clean** (default): every probe passes; use it with `mcpeval init` to
//!   see a green battery in under a minute.
//! - **`--broken <aspect>`**: reproduces one specific defect so the matching
//!   probe fails with its fixed reason label. Aspects: `schema`, `fidelity`,
//!   `unstable-errors`, `bloated`, `duplicate-page`, `stalled-cursor`,
//!   `slow`, `negotiation`, `sampling`, `elicitation`, `subscription`.
//!
//! The server is a demo fixture, not production code: state lives in a
//! counter, nothing persists, and stderr is free-form.

use std::io::{BufRead, Write};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

fn main() {
    let mut broken: Option<String> = None;
    for argument in std::env::args().skip(1) {
        match argument.as_str() {
            "--broken" => broken = Some(String::new()),
            value if broken.as_deref() == Some("") => broken = Some(value.to_owned()),
            other => {
                eprintln!("unknown argument {other}; usage: mcpeval-demo [--broken <aspect>]");
                std::process::exit(2);
            }
        }
    }
    let broken = match broken {
        Some(aspect) if aspect.is_empty() => {
            eprintln!("--broken requires an aspect: schema, fidelity, unstable-errors, bloated, duplicate-page, stalled-cursor, slow, cancellation, negotiation, sampling, elicitation, subscription, completion");
            std::process::exit(2);
        }
        other => other,
    };
    if let Err(error) = serve(broken.as_deref()) {
        eprintln!("mcpeval-demo: {error}");
        std::process::exit(1);
    }
}

fn serve(broken: Option<&str>) -> anyhow::Result<()> {
    let mut stdout = std::io::stdout().lock();
    let mut calls = 0u64;
    let mut flaky_calls = 0u64;
    let mut broken_state = false;
    // Shared flag for the in-flight cancellable call. A dedicated reader
    // thread owns stdin: it parses every frame, records cancellation
    // notifications into the flag, and forwards requests to the dispatch
    // loop over a channel. Without the separate reader, a slow tool would
    // block the only stdin reader and no mid-flight cancellation could
    // ever be observed.
    let cancelled_flag: std::sync::Arc<std::sync::Mutex<std::collections::HashSet<u64>>> =
        Default::default();
    let (sender, receiver) = std::sync::mpsc::channel::<Value>();
    let reader_broken = broken.is_some_and(|aspect| aspect == "cancellation");
    let reader_flag = std::sync::Arc::clone(&cancelled_flag);
    std::thread::spawn(move || {
        let stdin = std::io::stdin();
        for line in stdin.lock().lines() {
            let Ok(line) = line else { break };
            if line.trim().is_empty() {
                continue;
            }
            let Ok(request) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            let method = request.get("method").and_then(Value::as_str).unwrap_or("");
            if method == "notifications/cancelled" {
                if let Some(request_id) = request
                    .get("params")
                    .and_then(|params| params.get("requestId"))
                    .and_then(Value::as_u64)
                {
                    if reader_broken {
                        // The defect under test: the notification is dropped.
                        continue;
                    }
                    reader_flag
                        .lock()
                        .expect("cancellation flag lock")
                        .insert(request_id);
                }
                continue;
            }
            if method.starts_with("notifications/") {
                continue;
            }
            if sender.send(request).is_err() {
                break;
            }
        }
    });
    // Ids of sub-requests (sampling/createMessage, elicitation/create)
    // this server issued mid-call. The client's reply to one arrives as
    // the next inbound frame and must be consumed as a reply, not
    // dispatched as a new request.
    let mut outstanding_sub_ids: std::collections::HashSet<u64> = Default::default();
    while let Ok(request) = receiver.recv() {
        let request_id_value = request.get("id").cloned();
        if let Some(reply_id) = request_id_value.as_ref().and_then(Value::as_u64) {
            if outstanding_sub_ids.remove(&reply_id) && request.get("method").is_none() {
                // A reply to a mid-call sub-request: consume it.
                continue;
            }
        }
        let method = request
            .get("method")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned();
        let id = request
            .get("id")
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("request is missing an id"))?;
        let params = request.get("params").cloned().unwrap_or(json!({}));
        let response_id = id.as_u64();
        let response = match method.as_str() {
            "initialize" => {
                let offered = params
                    .get("protocolVersion")
                    .and_then(Value::as_str)
                    .unwrap_or("2025-06-18");
                let negotiated = if broken == Some("negotiation") {
                    // The defect under test: echo the offered version even
                    // when the probe offered one the server never
                    // supported.
                    offered.to_owned()
                } else {
                    "2025-06-18".to_owned()
                };
                Ok(json!({
                    "protocolVersion": negotiated,
                    "capabilities": {
                        "tools": {},
                        "resources": {
                            "subscribe": broken == Some("subscription") || broken.is_none()
                        },
                        "prompts": {},
                        "completions": {}
                    },
                    "serverInfo": {"name": "mcpeval-demo", "version": env!("CARGO_PKG_VERSION")}
                }))
            }
            "tools/list" => tools_page(&params, broken, &mut calls),
            "tools/call" => {
                let outcome = call_tool(ToolCall {
                    params: &params,
                    broken,
                    calls: &mut calls,
                    flaky_calls: &mut flaky_calls,
                    broken_state: &mut broken_state,
                    cancelled_flag: &cancelled_flag,
                    request_id: id.as_u64(),
                    stdout: &mut stdout,
                });
                if let Some(sub_id) = id.as_u64() {
                    // The handler wrote one mid-call sub-request using
                    // this id scheme; expect a reply for it next.
                    let issued = match broken {
                        Some("sampling") | Some("elicitation") | None => {
                            tool_written_sub_request(&params)
                        }
                        _ => false,
                    };
                    if issued {
                        outstanding_sub_ids.insert(sub_id + 1000);
                        outstanding_sub_ids.insert(sub_id + 100);
                    }
                }
                outcome
            }
            "resources/list" => {
                if broken == Some("surface") {
                    Ok(json!({"unexpected": true}))
                } else {
                    Ok(json!({"resources": [
                        {"uri": "demo://status", "name": "status"}
                    ]}))
                }
            }
            "resources/read" => {
                let uri = params.get("uri").and_then(Value::as_str).unwrap_or("");
                if uri == "demo://status" {
                    Ok(json!({"contents": [
                        {"uri": "demo://status", "text": "ready"}
                    ]}))
                } else {
                    // Honest bounded behavior: an unlisted URI is a
                    // structured error, not a fabricated read.
                    Err((-32012, "resource not found".into(), false))
                }
            }
            "resources/subscribe" | "resources/unsubscribe" => {
                if broken == Some("subscription") {
                    // Subscribe itself still succeeds; the defect is the
                    // missing update notification.
                    Ok(json!({}))
                } else {
                    Ok(json!({}))
                }
            }
            "sampling/createMessage" => Ok(json!({
                "role": "assistant",
                "content": {"type": "text", "text": "demo sample"},
                "model": "demo-stub"
            })),
            "elicitation/create" => Ok(json!({"action": "accept"})),
            "prompts/list" => {
                if broken == Some("surface") {
                    // A declared surface that never answers: the probe
                    // treats a transport failure as an invalid envelope.
                    Err((-32005, "prompts listing unavailable".into(), false))
                } else {
                    Ok(json!({"prompts": [
                        {"name": "welcome", "description": "Greeting prompt",
                         "arguments": [{"name": "language", "description": "Greeting language",
                                        "required": false}]}
                    ]}))
                }
            }
            "completion/complete" => {
                let reference = params.get("ref").cloned().unwrap_or(json!({}));
                let argument = params.get("argument").cloned().unwrap_or(json!({}));
                let name = argument.get("name").and_then(Value::as_str).unwrap_or("");
                let prefix = argument.get("value").and_then(Value::as_str).unwrap_or("");
                let prompt_name = reference.get("name").and_then(Value::as_str).unwrap_or("");
                if reference.get("type").and_then(Value::as_str) == Some("ref/prompt")
                    && prompt_name == "welcome"
                    && name == "language"
                {
                    if broken == Some("completion") {
                        // The defect under test: completion values that are
                        // not strings — a malformed envelope from a server
                        // that declares the capability.
                        Ok(json!({"completion": {"values": [1, 2, 3]}}))
                    } else {
                        let matched: Vec<Value> = ["en", "es", "fr"]
                            .iter()
                            .filter(|value| value.starts_with(prefix))
                            .map(|value| json!(value))
                            .collect();
                        Ok(json!({"completion": {"values": matched}}))
                    }
                } else {
                    // An argument the prompt does not declare: a structured
                    // error naming the argument, per the completion contract.
                    Err((-32602, format!("unknown argument {name:?}"), false))
                }
            }
            _ => Ok(json!({})),
        };
        // A cancelled request is answered with nothing: the marker error
        // (-32004) from the slow tool signals silence for this id. The
        // cancellation verdict lives in the shared flag the reader thread
        // updates.
        let cancelled_marker = matches!(&response, Err((code, _, _)) if *code == -32004)
            && response_id
                .map(|rid| {
                    cancelled_flag
                        .lock()
                        .expect("cancellation flag lock")
                        .contains(&rid)
                })
                .unwrap_or(false);
        if cancelled_marker {
            continue;
        }
        match response {
            Ok(result) => write_message(
                &mut stdout,
                json!({
                    "jsonrpc": "2.0", "id": id, "result": result
                }),
            )?,
            Err((code, message, retryable)) => write_message(
                &mut stdout,
                json!({
                    "jsonrpc": "2.0", "id": id,
                    "error": {"code": code, "message": message, "retryable": retryable}
                }),
            )?,
        }
    }
    Ok(())
}

fn write_message(stdout: &mut impl Write, message: Value) -> anyhow::Result<()> {
    serde_json::to_writer(&mut *stdout, &message)?;
    stdout.write_all(b"\n")?;
    stdout.flush()?;
    Ok(())
}

/// Whether the tool named by a `tools/call` params object writes one
/// mid-call sub-request to stdout.
fn tool_written_sub_request(params: &Value) -> bool {
    matches!(
        params.get("name").and_then(Value::as_str),
        Some("sampled_read") | Some("elicited_read")
    )
}

fn tool_entry(name: &str, description: &str, properties: Value) -> Value {
    json!({
        "name": name,
        "description": description,
        "inputSchema": {
            "type": "object",
            "properties": properties,
        }
    })
}

fn catalog(broken: Option<&str>) -> Vec<Value> {
    let mut entries = vec![
        tool_entry(
            "describe_status",
            "Return the service status. Read-only.",
            json!({}),
        ),
        tool_entry(
            "read_counter",
            "Read and increment the call counter. Read-only.",
            json!({}),
        ),
        tool_entry(
            "shared_read",
            "Read shared state concurrently. Read-only.",
            json!({"port": {"type": "integer", "description": "ignored port hint"}}),
        ),
        tool_entry(
            "flaky_read",
            "Fails twice then succeeds; exercises retry behavior. Read-only.",
            json!({}),
        ),
        tool_entry(
            "slow_read",
            "A deliberately slow read (200 ms). Read-only.",
            json!({}),
        ),
        tool_entry(
            "break_session",
            "Force the session into a broken state.",
            json!({}),
        ),
        tool_entry(
            "recover_session",
            "Recover the session from the broken state.",
            json!({}),
        ),
        tool_entry(
            "session_status",
            "Report session health; false while broken. Read-only.",
            json!({}),
        ),
        tool_entry(
            "report_weather",
            "Return a structured weather reading. Read-only.",
            json!({"city": {"type": "string", "description": "City name"}}),
        ),
        tool_entry(
            "sampled_read",
            "Reads with model sampling: issues one sampling/createMessage. Read-only.",
            json!({}),
        ),
        tool_entry(
            "elicited_read",
            "Reads with user elicitation: issues one elicitation/create. Read-only.",
            json!({}),
        ),
        tool_entry(
            "publish_status",
            "Republishes demo://status, notifying subscribers. Read-only.",
            json!({}),
        ),
    ];
    if broken == Some("output-schema") {
        // report_weather declares an outputSchema but responds without
        // structuredContent: exactly the contract break the probe checks.
        if let Some(entry) = entries
            .iter_mut()
            .find(|entry| entry["name"] == "report_weather")
        {
            entry["outputSchema"] = json!({
                "type": "object",
                "properties": {
                    "temperature": {"type": "number"},
                    "conditions": {"type": "string"}
                },
                "required": ["temperature", "conditions"]
            });
        }
    }
    if broken == Some("schema") {
        // Declares a required field the naive {} call cannot supply, and
        // never lists the property: incoherent schema.
        entries[0] = json!({
            "name": "describe_status",
            "description": "Return the service status. Read-only.",
            "inputSchema": {
                "type": "object",
                "properties": {},
                "required": ["missing"]
            }
        });
    }
    if broken == Some("bloated") {
        for entry in &mut entries {
            let long = "context padding ".repeat(120);
            entry["description"] = json!(format!(
                "{}. The remainder exists to inflate catalog size: {long}",
                entry["description"].as_str().unwrap_or_default()
            ));
        }
        entries.push(tool_entry(
            "extra_padding_tool",
            "Exists only to add catalog weight: filler filler filler",
            json!({}),
        ));
    }
    entries
}

fn tools_page(
    params: &Value,
    broken: Option<&str>,
    calls: &mut u64,
) -> Result<Value, (i64, String, bool)> {
    *calls += 1;
    match broken {
        Some("duplicate-page") | Some("stalled-cursor") => {
            // Page the catalog with cursor "next"; duplicate-page repeats
            // the first tool on the second page, stalled-cursor never ends.
            let cursor = params.get("cursor").and_then(Value::as_str);
            let repeat = broken == Some("duplicate-page") && cursor == Some("next");
            let tools = if cursor.is_none() {
                catalog(None)
            } else if repeat {
                vec![catalog(None)[0].clone()]
            } else {
                Vec::new()
            };
            let mut result = json!({"tools": tools});
            if broken == Some("stalled-cursor") || cursor.is_none() {
                result["nextCursor"] = json!("next");
            }
            Ok(result)
        }
        _ => Ok(json!({"tools": catalog(broken)})),
    }
}

struct ToolCall<'a> {
    params: &'a Value,
    broken: Option<&'a str>,
    calls: &'a mut u64,
    flaky_calls: &'a mut u64,
    broken_state: &'a mut bool,
    cancelled_flag: &'a std::sync::Arc<std::sync::Mutex<std::collections::HashSet<u64>>>,
    request_id: Option<u64>,
    stdout: &'a mut dyn Write,
}

fn call_tool(
    ToolCall {
        params,
        broken,
        calls,
        flaky_calls,
        broken_state,
        cancelled_flag,
        request_id,
        mut stdout,
    }: ToolCall<'_>,
) -> Result<Value, (i64, String, bool)> {
    let name = params
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let arguments = params.get("arguments").cloned().unwrap_or(json!({}));
    *calls += 1;
    if name == "slow_read" {
        // A cancellable long-running call: sleep in short slices while
        // polling the cancellation flag that the reader loop updates from
        // notifications/cancelled. A cancelled request is never answered:
        // the caller signals suppression via the marker error.
        let deadline = Instant::now() + Duration::from_millis(400);
        while Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
            if let Some(id) = request_id {
                let cancelled = cancelled_flag
                    .lock()
                    .expect("cancellation flag lock")
                    .contains(&id);
                if cancelled {
                    return Err((-32004, "cancelled".into(), false));
                }
            }
        }
    }
    match name.as_str() {
        "describe_status" => {
            let status = match broken {
                Some("fidelity") => "degraded",
                _ => "ready",
            };
            Ok(json!({
                "content": [{"type": "text", "text": format!("status: {status}")}],
                "structuredContent": {"status": status},
                "status": status,
            }))
        }
        "read_counter" => Ok(json!({
            "content": [{"type": "text", "text": format!("count={}", calls)}],
            "structuredContent": {"count": *calls}
        })),
        "slow_read" => Ok(json!({
            "content": [{"type": "text", "text": "finally awake"}],
            "structuredContent": {"ok": true}
        })),
        "shared_read" => Ok(json!({
            "content": [{"type": "text", "text": "shared ok"}],
            "structuredContent": {"ok": true}
        })),
        "flaky_read" => {
            *flaky_calls += 1;
            if *flaky_calls <= 2 {
                let code = match broken {
                    Some("unstable-errors") => -32000 - *flaky_calls as i64,
                    _ => -32001,
                };
                // Retryable truthfully: it does succeed on the third call.
                return Err((code, "flaky failure".into(), true));
            }
            Ok(json!({
                "content": [{"type": "text", "text": "recovered"}],
                "structuredContent": {"ok": true}
            }))
        }
        "break_session" => {
            *broken_state = true;
            // Deliberately errors: the state-recovery probe requires an
            // observed failure before recovery.
            Err((-32002, "session forced into a broken state".into(), false))
        }
        "recover_session" => {
            *broken_state = false;
            Ok(json!({
                "content": [{"type": "text", "text": "recovered"}],
                "structuredContent": {"broken": false}
            }))
        }
        "session_status" => {
            if *broken_state {
                return Err((-32003, "session is broken".into(), false));
            }
            Ok(json!({
                "content": [{"type": "text", "text": "healthy"}],
                "structuredContent": {"healthy": true}
            }))
        }
        "report_weather" => {
            let city = arguments
                .get("city")
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            if broken == Some("output-schema") {
                // Declares outputSchema but omits structuredContent.
                Ok(json!({
                    "content": [{"type": "text", "text": format!("weather for {city}")}]
                }))
            } else {
                Ok(json!({
                    "content": [{"type": "text", "text": format!("weather for {city}")}],
                    "structuredContent": {"temperature": 21.0, "conditions": "clear"}
                }))
            }
        }
        "sampled_read" => {
            // Issue one sampling/createMessage mid-call. The clean
            // personality's request is well-formed; the sampling defect
            // sends a request missing the messages array entirely, so the
            // probe answers "method unavailable" and the call errors —
            // the server must accept the stub reply shape.
            let malformed = broken == Some("sampling");
            if let Some(request_id) = request_id {
                let sub = if malformed {
                    json!({
                        "jsonrpc": "2.0", "id": request_id + 1000,
                        "method": "sampling/createMessage",
                        "params": {"model": "demo", "messages": "not-an-array"}
                    })
                } else {
                    json!({
                        "jsonrpc": "2.0", "id": request_id + 100,
                        "method": "sampling/createMessage",
                        "params": {
                            "messages": [{"role": "user",
                                "content": {"type": "text", "text": "summarize the status"}}],
                            "maxTokens": 16
                        }
                    })
                };
                let _ = write_message(&mut stdout, sub);
            }
            let ok = json!({
                "content": [{"type": "text", "text": "sampled"}],
                "structuredContent": {"ok": true}
            });
            if broken == Some("sampling") {
                // The defect: the server rejects the stub reply's shape
                // and fails the call with a structured error.
                Err((-32010, "sampling reply rejected".into(), false))
            } else {
                Ok(ok)
            }
        }
        "elicited_read" => {
            // One elicitation/create mid-call; the elicitation defect
            // sends a request missing requestedSchema.
            if let Some(request_id) = request_id {
                let sub = if broken == Some("elicitation") {
                    json!({
                        "jsonrpc": "2.0", "id": request_id + 100,
                        "method": "elicitation/create",
                        "params": {"message": "Proceed without a schema?"}
                    })
                } else {
                    json!({
                        "jsonrpc": "2.0", "id": request_id + 100,
                        "method": "elicitation/create",
                        "params": {
                            "message": "Continue reading?",
                            "requestedSchema": {
                                "type": "object",
                                "properties": {"confirm": {"type": "boolean"}}
                            }
                        }
                    })
                };
                let _ = write_message(&mut stdout, sub);
            }
            let ok = json!({
                "content": [{"type": "text", "text": "elicited"}],
                "structuredContent": {"ok": true}
            });
            if broken == Some("elicitation") {
                // The defect: the server rejects the action reply to its
                // own malformed request and fails the call.
                Err((-32011, "elicitation flow broke".into(), false))
            } else {
                Ok(ok)
            }
        }
        "publish_status" => {
            // Republish the status resource: subscribers get
            // notifications/resources/updated for demo://status. The
            // subscription defect claims `subscribe` support but never
            // emits the notification.
            if let Some(_request_id) = request_id {
                // Notifications are session-scoped; the demo has a single
                // client session.
                if broken != Some("subscription") {
                    let _ = write_message(
                        &mut stdout,
                        json!({
                            "jsonrpc": "2.0",
                            "method": "notifications/resources/updated",
                            "params": {"uri": "demo://status"}
                        }),
                    );
                }
            }
            Ok(json!({
                "content": [{"type": "text", "text": "published"}],
                "structuredContent": {"published": true}
            }))
        }
        _ => Err((-32602, format!("unknown tool {name}"), false)),
    }
}
