//! Serve findings and trends to agents over Streamable HTTP MCP.
//!
//! A minimal loopback-only MCP server exposing sanitized, share-safe data:
//! `list_findings`, `get_finding`, and `get_readiness_trends`. It never
//! persists anything and serves only what `mcpeval findings --format json`
//! and `mcpeval trends` would print. Single-shot JSON responses (no SSE),
//! one request per connection, the same bounded-IO posture as the capture
//! proxy.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::time::{Duration, Instant};

use anyhow::{bail, Context};
use serde_json::{json, Value};

const IO_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_BODY_BYTES: usize = 1024 * 1024;
const MAX_HEADER_BYTES: usize = 16 * 1024;

pub fn run(listen: String) -> anyhow::Result<()> {
    let address: SocketAddr = listen.parse().context("listen address is invalid")?;
    if !address.ip().is_loopback() {
        bail!("findings servers must use a loopback address");
    }
    let root = crate::store::Store::resolve_root(None);
    let listener = TcpListener::bind(address).context("binding findings server failed")?;
    eprintln!(
        "mcpeval serve: serving findings from {} at http://{address}/mcp",
        root.display()
    );
    for connection in listener.incoming() {
        let mut stream = connection.context("accepting connection failed")?;
        stream.set_read_timeout(Some(IO_TIMEOUT)).ok();
        stream.set_write_timeout(Some(IO_TIMEOUT)).ok();
        let _ = handle_connection(&mut stream, &root);
    }
    Ok(())
}

fn handle_connection(stream: &mut TcpStream, root: &std::path::Path) -> anyhow::Result<()> {
    let request = match read_request(stream) {
        Ok(request) => request,
        Err(_) => return write_http(stream, 400, &json!({"error": "invalid request"})),
    };
    if request.method != "POST" {
        return write_http(stream, 405, &json!({"error": "POST only"}));
    }
    let message: Value = match serde_json::from_slice(&request.body) {
        Ok(value) => value,
        Err(_) => return write_http(stream, 400, &json!({"error": "invalid JSON body"})),
    };
    if message.get("id").is_none() {
        // Notifications (e.g. notifications/initialized) are acknowledged.
        return write_http(stream, 202, &Value::Null);
    }
    let response = match message.get("method").and_then(Value::as_str) {
        Some("initialize") => json!({
            "jsonrpc": "2.0",
            "id": message["id"],
            "result": {
                "protocolVersion": "2025-06-18",
                "capabilities": {"tools": {}},
                "serverInfo": {"name": "mcpeval", "version": env!("CARGO_PKG_VERSION")}
            }
        }),
        Some("tools/list") => {
            let id = message["id"].clone();
            let tools = vec![
                list_findings_tool(),
                get_finding_tool(),
                readiness_trends_tool(),
            ];
            json!({"jsonrpc": "2.0", "id": id, "result": {"tools": tools}})
        }
        Some("tools/call") => match handle_call(&message, root) {
            Ok(result) => json!({"jsonrpc": "2.0", "id": message["id"], "result": result}),
            Err(error) => json!({
                "jsonrpc": "2.0",
                "id": message["id"],
                "error": {"code": -32000, "message": error.to_string()}
            }),
        },
        Some(method) => json!({
            "jsonrpc": "2.0",
            "id": message["id"],
            "error": {"code": -32601, "message": format!("unknown method {method}")}
        }),
        None => json!({
            "jsonrpc": "2.0",
            "id": message["id"],
            "error": {"code": -32600, "message": "invalid request"}
        }),
    };
    write_http(stream, 200, &response)
}

fn readiness_trends_tool() -> Value {
    tool(
        "get_readiness_trends",
        "Readiness-score history per server recorded by full-battery \
         `mcpeval probe` runs, oldest first with the newest last.",
        None,
    )
}

fn list_findings_tool() -> Value {
    tool(
        "list_findings",
        "List promoted findings: privacy-safe failure metadata (server, \
         tool, state, severity, evidence counts). Run `mcpeval index` and \
         `mcpeval promote` first.",
        Some(json!({
            "type": "object",
            "properties": {
                "state": {
                    "type": "string",
                    "description": "Optional lifecycle filter: open, fix-claimed, verifying, or closed."
                }
            }
        })),
    )
}

fn get_finding_tool() -> Value {
    tool(
        "get_finding",
        "Get one promoted finding by its finding-* identifier, including \
         the shape-level repro.",
        Some(json!({
            "type": "object",
            "properties": {
                "finding_id": {"type": "string", "description": "finding-* identifier"}
            },
            "required": ["finding_id"]
        })),
    )
}

fn tool(name: &str, description: &str, input_schema: Option<Value>) -> Value {
    let mut entry = json!({"name": name, "inputSchema": {"type": "object", "properties": {}}});
    if let Some(schema) = input_schema {
        entry["inputSchema"] = schema;
    }
    entry["description"] = json!(description);
    entry
}

fn handle_call(message: &Value, root: &std::path::Path) -> anyhow::Result<Value> {
    let name = message
        .get("params")
        .and_then(|params| params.get("name"))
        .and_then(Value::as_str)
        .context("tools/call is missing a tool name")?;
    let arguments = message
        .get("params")
        .and_then(|params| params.get("arguments"))
        .cloned()
        .unwrap_or(Value::Object(Default::default()));
    match name {
        "list_findings" => {
            let state_filter = arguments.get("state").and_then(Value::as_str);
            let findings = crate::report::load_findings(root).unwrap_or_default();
            let payload: Vec<Value> = findings
                .iter()
                .map(serde_json::to_value)
                .collect::<Result<_, _>>()?;
            let selected: Vec<Value> = payload
                .into_iter()
                .filter(|finding| match state_filter {
                    Some(state) => finding_state(finding) == state,
                    None => true,
                })
                .collect();
            Ok(text_result(&format_findings(&selected)))
        }
        "get_finding" => {
            let wanted = arguments
                .get("finding_id")
                .and_then(Value::as_str)
                .context("get_finding requires finding_id")?;
            let findings = match crate::report::load_findings(root) {
                Ok(findings) => findings,
                Err(_) => bail!("no such finding"),
            };
            let payload: Vec<Value> = findings
                .iter()
                .map(serde_json::to_value)
                .collect::<Result<_, _>>()?;
            let Some(finding) = payload
                .iter()
                .find(|finding| finding_id_of(finding) == wanted)
            else {
                bail!("no such finding");
            };
            Ok(text_result(&format_finding(finding)))
        }
        "get_readiness_trends" => {
            let points = crate::trends::load(root, 10)?;
            Ok(text_result(&format_trends(&points)))
        }
        other => bail!("unknown tool {other}"),
    }
}

fn finding_state(finding: &Value) -> &str {
    finding
        .get("state")
        .and_then(Value::as_str)
        .unwrap_or_default()
}

fn finding_id_of(finding: &Value) -> &str {
    finding
        .get("finding_id")
        .and_then(Value::as_str)
        .unwrap_or("")
}

fn text_result(text: &str) -> Value {
    json!({"content": [{"type": "text", "text": text}]})
}

fn format_findings(findings: &[Value]) -> String {
    if findings.is_empty() {
        return "no findings; run `mcpeval index` and `mcpeval promote` first".into();
    }
    findings
        .iter()
        .map(format_finding)
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn format_finding(finding: &Value) -> String {
    let tool = finding
        .get("tool")
        .and_then(Value::as_str)
        .unwrap_or("unlisted");
    let state = finding.get("state").and_then(Value::as_str).unwrap_or("?");
    let severity = finding
        .get("severity")
        .and_then(Value::as_str)
        .unwrap_or("?");
    let id = finding
        .get("finding_id")
        .and_then(Value::as_str)
        .unwrap_or("?");
    let server = finding.get("server").and_then(Value::as_str).unwrap_or("?");
    let failures = finding.get("failures").and_then(Value::as_u64).unwrap_or(0);
    let calls = finding.get("calls").and_then(Value::as_u64).unwrap_or(0);
    let sessions = finding.get("sessions").and_then(Value::as_u64).unwrap_or(0);
    let probe = finding
        .get("probe_id")
        .and_then(Value::as_str)
        .unwrap_or("none");
    let repro = finding
        .get("repro")
        .map(|repro| format!(" repro={repro}"))
        .unwrap_or_default();
    format!(
        "{id} {server}/{tool} state={state} severity={severity} probe={probe} failures={failures}/{calls} sessions={sessions}{repro}",
    )
}

fn format_trends(points: &[crate::trends::TrendPoint]) -> String {
    if points.is_empty() {
        return "no trend history yet; run `mcpeval probe` first".into();
    }
    points
        .iter()
        .map(|point| {
            format!(
                "{} {} score={}/100 cases={}/{}{}",
                point.server,
                point.ts,
                point.score,
                point.cases_passed,
                point.cases_total,
                if point.passed { "" } else { " FAILING" }
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

struct ServeRequest {
    method: String,
    body: Vec<u8>,
}

fn read_request(stream: &mut TcpStream) -> anyhow::Result<ServeRequest> {
    let mut reader = BufReader::new(stream);
    let deadline = Instant::now() + IO_TIMEOUT;
    let request_line = read_line_bounded(&mut reader, MAX_HEADER_BYTES, deadline)?;
    let mut parts = request_line.split_whitespace();
    let method = parts.next().context("HTTP method is missing")?.to_owned();
    let _path = parts.next().context("HTTP path is missing")?;
    if parts.next() != Some("HTTP/1.1") {
        bail!("unsupported HTTP request line");
    }
    let mut content_length = 0usize;
    loop {
        let line = read_line_bounded(
            &mut reader,
            MAX_HEADER_BYTES.saturating_sub(content_length),
            deadline,
        )?;
        if line == "\r\n" {
            break;
        }
        if let Some((name, value)) = line.trim_end_matches(['\r', '\n']).split_once(':') {
            if name.eq_ignore_ascii_case("content-length") {
                content_length = value.trim().parse().unwrap_or(0);
            }
        }
    }
    if content_length > MAX_BODY_BYTES {
        bail!("HTTP body exceeded the size limit");
    }
    let mut body = vec![0; content_length];
    reader.read_exact(&mut body)?;
    Ok(ServeRequest { method, body })
}

fn read_line_bounded(
    reader: &mut BufReader<&mut TcpStream>,
    limit: usize,
    deadline: Instant,
) -> anyhow::Result<String> {
    if limit == 0 {
        bail!("HTTP headers exceeded the size limit");
    }
    let mut bytes = Vec::new();
    loop {
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .context("HTTP request deadline exceeded")?;
        reader.get_mut().set_read_timeout(Some(remaining)).ok();
        let available = reader.fill_buf()?;
        if available.is_empty() {
            bail!("HTTP request ended before the headers were complete");
        }
        let consumed = available
            .iter()
            .position(|byte| *byte == b'\n')
            .map_or(available.len(), |index| index + 1);
        if bytes.len().saturating_add(consumed) > limit {
            bail!("HTTP headers exceeded the size limit");
        }
        let complete = available.get(consumed.saturating_sub(1)) == Some(&b'\n');
        bytes.extend_from_slice(&available[..consumed]);
        reader.consume(consumed);
        if complete {
            return String::from_utf8(bytes).context("HTTP headers are not UTF-8");
        }
    }
}

fn write_http(stream: &mut TcpStream, status: u16, payload: &Value) -> anyhow::Result<()> {
    let body = serde_json::to_vec(payload)?;
    let reason = match status {
        200 => "OK",
        202 => "Accepted",
        400 => "Bad Request",
        404 => "Not Found",
        405 => "Method Not Allowed",
        _ => "Response",
    };
    write!(
        stream,
        "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    )?;
    stream.write_all(&body)?;
    stream.flush()?;
    Ok(())
}
