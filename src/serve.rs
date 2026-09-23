//! Serve findings and trends to agents over Streamable HTTP MCP.
//!
//! A minimal loopback-only MCP server exposing sanitized, share-safe data:
//! `list_findings`, `get_finding`, and `get_readiness_trends`, plus the
//! agent-loop tools `run_probe` and `scaffold` and the write-side
//! `record_annotation`. It serves only what `mcpeval findings --format
//! json` and `mcpeval trends` would print, and writes only what
//! `mcpeval annotate` would write — the same validated, hashed, bounded
//! record. Single-shot JSON responses (no SSE), one request per
//! connection, the same bounded-IO posture as the capture proxy.
//!
//! Binding to loopback does not keep a browser out: a web page can POST to
//! 127.0.0.1 directly or through a rebound DNS name. Every request must
//! therefore carry a loopback `Host`, a loopback `Origin` when it carries
//! one at all, and `Content-Type: application/json`, which a cross-origin
//! page cannot send without a CORS preflight this server never grants.
//! `run_probe` and `scaffold` launch the process an agent names, so they
//! are listed and callable only when the operator passes `--allow-spawn`.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::time::{Duration, Instant};

use anyhow::{bail, Context};
use serde_json::{json, Value};

use crate::loopback::Provenance;

const IO_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_BODY_BYTES: usize = 1024 * 1024;
const MAX_HEADER_BYTES: usize = 16 * 1024;

const SPAWN_DISABLED: &str = "launches server processes and is disabled; restart `mcpeval serve` with --allow-spawn to enable it";

pub fn run(listen: String, allow_spawn: bool) -> anyhow::Result<()> {
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
    if !allow_spawn {
        eprintln!(
            "mcpeval serve: run_probe and scaffold are disabled; pass --allow-spawn to let agents launch server processes"
        );
    }
    for connection in listener.incoming() {
        let mut stream = connection.context("accepting connection failed")?;
        stream.set_read_timeout(Some(IO_TIMEOUT)).ok();
        stream.set_write_timeout(Some(IO_TIMEOUT)).ok();
        let _ = handle_connection(&mut stream, &root, allow_spawn);
    }
    Ok(())
}

fn handle_connection(
    stream: &mut TcpStream,
    root: &std::path::Path,
    allow_spawn: bool,
) -> anyhow::Result<()> {
    let request = match read_request(stream) {
        Ok(request) => request,
        Err(_) => return write_http(stream, 400, &json!({"error": "invalid request"})),
    };
    if let Some((status, error)) = request.provenance.rejection() {
        return write_http(stream, status, &json!({"error": error}));
    }
    if request.method != "POST" {
        return write_http(stream, 405, &json!({"error": "POST only"}));
    }
    if !request.provenance.is_json() {
        return write_http(
            stream,
            415,
            &json!({"error": "Content-Type must be application/json"}),
        );
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
            let mut tools = vec![
                list_findings_tool(),
                get_finding_tool(),
                readiness_trends_tool(),
            ];
            if allow_spawn {
                tools.push(run_probe_tool());
                tools.push(scaffold_tool());
            }
            tools.push(record_annotation_tool());
            json!({"jsonrpc": "2.0", "id": id, "result": {"tools": tools}})
        }
        Some("tools/call") => match handle_call(&message, root, allow_spawn) {
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
         `mcpeval probe` runs, oldest first with the newest last. Each point \
         carries the SHA-256 of the manifest it ran; a score delta is shown \
         only against the previous run of the same manifest.",
        None,
    )
}

fn list_findings_tool() -> Value {
    tool(
        "list_findings",
        "List promoted findings: privacy-safe failure metadata (server, \
         tool, state, severity, class, evidence counts). Run `mcpeval index` \
         and `mcpeval promote` first.",
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
         its class, the server-side fix hint, and the shape-level repro.",
        Some(json!({
            "type": "object",
            "properties": {
                "finding_id": {"type": "string", "description": "finding-* identifier"}
            },
            "required": ["finding_id"]
        })),
    )
}

fn run_probe_tool() -> Value {
    tool(
        "run_probe",
        "Run the deterministic, read-only probe battery against an MCP \
         server and return the full mcpeval.probe-report/v1 document: \
         per-case verdicts with fixed failure reasons and remediation \
         hints, measurements, and the readiness score. Mutation is never \
         authorized through this tool. Provide `manifest` as an inline \
         JSON manifest object and target the server with `command` (stdio, \
         split into arguments) or `url` (Streamable HTTP).",
        Some(json!({
            "type": "object",
            "properties": {
                "manifest": {
                    "type": "object",
                    "description": "Inline mcp-eval manifest (version 1) with the probe cases to run."
                },
                "command": {
                    "type": "array",
                    "items": {"type": "string"},
                    "description": "stdio server command split into program and arguments."
                },
                "url": {"type": "string", "description": "Streamable HTTP endpoint to probe instead of a stdio command."},
                "server_label": {
                    "type": "string",
                    "description": "Server label for the report; defaults to 'probed'."
                }
            },
            "required": ["manifest"]
        })),
    )
}

fn scaffold_tool() -> Value {
    tool(
        "scaffold",
        "Introspect a live MCP server and derive a starter manifest: \
         discovery/token budgets from measured sizes, catalog pagination, \
         protocol negotiation, and surface listing when resources or \
         prompts are declared. Zero-required tools annotated readOnlyHint \
         are candidates; with `confirm_read_only` true (an attestation that \
         unannotated tools are read-only) unannotated zero-required tools \
         are too. Tools annotated destructiveHint or readOnlyHint=false are \
         never called. Every candidate observed to answer a naive `{}` call \
         gets schema-guessability, degradation-over-n, latency-budget (from \
         its measured latency), and output-schema when declared, plus one \
         contention and one payload-bounds case. Returns the manifest JSON; \
         nothing is written to disk.",
        Some(json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "array",
                    "items": {"type": "string"},
                    "description": "stdio server command split into program and arguments."
                },
                "url": {"type": "string", "description": "Streamable HTTP endpoint instead of a stdio command."},
                "server_label": {
                    "type": "string",
                    "description": "Server label; defaults to 'probed'."
                },
                "confirm_read_only": {
                    "type": "boolean",
                    "description": "Attest that every zero-required tool without a readOnlyHint annotation is read-only, so it is called and scaffolded too."
                }
            }
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

fn record_annotation_tool() -> Value {
    tool(
        "record_annotation",
        "Record an agent-authored observation about one captured call, \
         identified by (session, seq) from the shim's sanitized journal. \
         Kind is one of a fixed set; the note is free text bounded to 240 \
         characters with no control characters. This is the one deliberate \
         prose channel in the store; every other field is structured \
         metadata. Sessions are hashed before persistence, exactly as the \
         `mcpeval annotate` command does.",
        Some(json!({
            "type": "object",
            "properties": {
                "session": {
                    "type": "string",
                    "description": "The session identifier the annotated call belongs to. Stored as a stable session:<sha256> token; the raw value is never persisted."
                },
                "seq": {
                    "type": "integer",
                    "description": "The seq of the call within that session."
                },
                "kind": {
                    "type": "string",
                    "enum": crate::record::ANNOTATION_KINDS,
                    "description": "What the observation is: a documented path was blocked, a call reported success but changed nothing, and so on."
                },
                "note": {
                    "type": "string",
                    "description": "Free-text note, at most 240 characters, no control characters or newlines."
                }
            },
            "required": ["session", "seq", "kind", "note"]
        })),
    )
}

fn handle_call(
    message: &Value,
    root: &std::path::Path,
    allow_spawn: bool,
) -> anyhow::Result<Value> {
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
            Ok(text_result(&format_finding_detail(finding)))
        }
        "get_readiness_trends" => {
            let points = crate::trends::load(root, 10)?;
            Ok(json!({
                "content": [{"type": "text", "text": format_trends(&points)}],
                "structuredContent": {"points": points},
            }))
        }
        "run_probe" | "scaffold" if !allow_spawn => bail!("{name} {SPAWN_DISABLED}"),
        "run_probe" => run_probe_tool_call(&arguments),
        "scaffold" => scaffold_tool_call(&arguments),
        "record_annotation" => record_annotation_tool_call(&arguments),
        other => bail!("unknown tool {other}"),
    }
}

/// Shared targeting for the agent-loop tools: `command` (array of words)
/// or `url`, plus an optional server label.
struct AgentTarget {
    server: String,
    command: Vec<String>,
    http_url: Option<String>,
}

fn agent_target(arguments: &Value) -> anyhow::Result<AgentTarget> {
    let server = arguments
        .get("server_label")
        .and_then(Value::as_str)
        .unwrap_or("probed")
        .to_owned();
    let command = arguments
        .get("command")
        .and_then(Value::as_array)
        .map(|words| {
            words
                .iter()
                .map(|word| {
                    Ok(word
                        .as_str()
                        .context("command items must be strings")?
                        .to_owned())
                })
                .collect::<anyhow::Result<Vec<_>>>()
        })
        .transpose()?;
    let http_url = arguments
        .get("url")
        .and_then(Value::as_str)
        .map(str::to_owned);
    match (&http_url, &command) {
        (Some(_), Some(words)) if !words.is_empty() => {
            bail!("select a URL or a stdio command, not both")
        }
        (Some(_), None | Some(_)) => Ok(AgentTarget {
            server,
            command: Vec::new(),
            http_url,
        }),
        (None, Some(words)) if !words.is_empty() => Ok(AgentTarget {
            server,
            command: words.clone(),
            http_url: None,
        }),
        (None, _) => bail!("a command array or a url is required"),
    }
}

fn run_probe_tool_call(arguments: &Value) -> anyhow::Result<Value> {
    let target = agent_target(arguments)?;
    let manifest_body = arguments
        .get("manifest")
        .map(serde_json::to_string)
        .transpose()
        .context("serializing inline manifest")?
        .context("run_probe requires a manifest object")?;
    let mut store = crate::store::Store::open(None)?;
    let report = crate::probe::run(
        crate::probe::ProbeOptions {
            server: target.server.clone(),
            manifest_path: std::path::PathBuf::new(),
            manifest_inline: Some(manifest_body),
            selected_probe: None,
            selected_case: None,
            // The agent surface is read-only by construction: no flag or
            // manifest combination can authorize mutation through it.
            allow_mutation: false,
            command: target.command,
            http_url: target.http_url,
            allow_remote_http: false,
        },
        &mut store,
    )?;
    let document = report.to_json(&target.server);
    // The report document is the structured payload; the text block adds
    // the remediation hints that the raw document does not carry.
    let mut lines = Vec::new();
    for case in &report.cases {
        if let Some(reason) = case.reason {
            lines.push(format!(
                "{}: {} — {}",
                case.id,
                reason.as_str(),
                crate::remediation::hint(reason)
            ));
        }
    }
    let text = if lines.is_empty() {
        format!(
            "all cases passed; readiness {}/100",
            crate::score::readiness(&report).overall
        )
    } else {
        lines.join("\n")
    };
    Ok(json!({
        "content": [{"type": "text", "text": text}],
        "structuredContent": document,
    }))
}

fn scaffold_tool_call(arguments: &Value) -> anyhow::Result<Value> {
    let target = agent_target(arguments)?;
    let manifest = crate::init::probe_scaffold(crate::init::ScaffoldRequest {
        server: target.server,
        confirm_read_only: arguments
            .get("confirm_read_only")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        command: target.command,
        http_url: target.http_url,
        allow_remote_http: false,
    })?;
    Ok(text_result(&serde_json::to_string_pretty(&manifest)?))
}

/// The write-side agent tool. Builds the same `AnnotationRecord` the
/// `mcpeval annotate` command builds, validates it with the same
/// `validate()` (kind against the fixed set, note length and control
/// characters), and hands it to the same `append_annotation` (which
/// hashes the session and re-validates the kind). Nothing about the
/// record path is weaker than the CLI's.
fn record_annotation_tool_call(arguments: &Value) -> anyhow::Result<Value> {
    let session = arguments
        .get("session")
        .and_then(Value::as_str)
        .context("record_annotation requires session")?;
    let seq = arguments
        .get("seq")
        .and_then(Value::as_u64)
        .context("record_annotation requires seq")?;
    let kind = arguments
        .get("kind")
        .and_then(Value::as_str)
        .context("record_annotation requires kind")?;
    let note = arguments
        .get("note")
        .and_then(Value::as_str)
        .context("record_annotation requires note")?;
    if note.chars().any(char::is_control) {
        bail!("note must not contain control characters or newlines");
    }
    let record = crate::record::AnnotationRecord {
        ts: chrono::Utc::now()
            .format("%Y-%m-%dT%H:%M:%S%.3fZ")
            .to_string(),
        session: session.to_owned(),
        seq,
        kind: kind.to_owned(),
        note: note.to_owned(),
    };
    record.validate()?;
    let mut store = crate::store::Store::open(None)?;
    store.append_annotation(&record)?;
    Ok(text_result(&format!(
        "recorded {} annotation for session:<hashed> seq {seq}",
        record.kind
    )))
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
    let class = finding.get("class").and_then(Value::as_str).unwrap_or("?");
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
        "{id} {server}/{tool} state={state} severity={severity} class={class} probe={probe} failures={failures}/{calls} sessions={sessions}{repro}",
    )
}

fn format_finding_detail(finding: &Value) -> String {
    let hint = finding.get("hint").and_then(Value::as_str).unwrap_or("?");
    format!("{}\nhint: {hint}", format_finding(finding))
}

fn format_trends(points: &[crate::trends::TrendPoint]) -> String {
    if points.is_empty() {
        return "no trend history yet; run `mcpeval probe` first".into();
    }
    points
        .iter()
        .enumerate()
        .map(|(index, point)| {
            let previous = index
                .checked_sub(1)
                .map(|previous| &points[previous])
                .filter(|previous| previous.server == point.server);
            format!("{} {} {}", point.server, point.ts, point.summary(previous))
        })
        .collect::<Vec<_>>()
        .join("\n")
}

struct ServeRequest {
    method: String,
    provenance: Provenance,
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
    let mut provenance = Provenance::default();
    let mut header_bytes = request_line.len();
    loop {
        let line = read_line_bounded(
            &mut reader,
            MAX_HEADER_BYTES.saturating_sub(header_bytes),
            deadline,
        )?;
        header_bytes = header_bytes.saturating_add(line.len());
        if line == "\r\n" {
            break;
        }
        if let Some((name, value)) = line.trim_end_matches(['\r', '\n']).split_once(':') {
            if name.eq_ignore_ascii_case("content-length") {
                content_length = value.trim().parse().unwrap_or(0);
            } else {
                provenance.record(name, value);
            }
        }
    }
    if content_length > MAX_BODY_BYTES {
        bail!("HTTP body exceeded the size limit");
    }
    let mut body = vec![0; content_length];
    reader.read_exact(&mut body)?;
    Ok(ServeRequest {
        method,
        provenance,
        body,
    })
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
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        415 => "Unsupported Media Type",
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finding_text_names_the_class_and_the_detail_adds_the_hint() {
        let finding = json!({
            "finding_id": "finding-0123456789abcdef",
            "server": "demo",
            "tool": "flaky_read",
            "state": "open",
            "severity": "medium",
            "class": "unstable-error-code",
            "hint": "pick one code",
            "failures": 6,
            "calls": 18,
            "sessions": 3
        });
        let line = format_finding(&finding);
        assert_eq!(
            line,
            "finding-0123456789abcdef demo/flaky_read state=open severity=medium \
             class=unstable-error-code probe=none failures=6/18 sessions=3"
        );
        assert_eq!(
            format_finding_detail(&finding),
            format!("{line}\nhint: pick one code")
        );
    }
}
