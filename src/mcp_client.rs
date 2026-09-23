use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{bail, Context};
use serde_json::{json, Value};

use crate::privacy;

// Generous by design: cold CI runners (first python spawn, antivirus
// scans) can exceed a few seconds before the first response. A genuinely
// hung server still fails; it just takes longer to be declared dead.
// The HTTP transport's five-second network timeouts are separate and
// deliberately stay tight — they bound network I/O, not process startup.
// A manifest's `timeout_ms` replaces both.
pub const DEFAULT_RESPONSE_TIMEOUT: Duration = Duration::from_secs(30);

/// A transport failure: the server stopped answering or went away. Both
/// clients put it in the error chain so probes can tell a lost server
/// from a protocol defect without matching message text.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TransportFailure {
    /// No response within the response timeout.
    Timeout,
    /// The server closed the stream or the connection, or was unreachable.
    Closed,
}

impl std::fmt::Display for TransportFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Timeout => "MCP response timed out",
            Self::Closed => "MCP server closed the connection",
        })
    }
}

impl std::error::Error for TransportFailure {}

impl TransportFailure {
    /// The transport failure in `error`, as its root or as a context layer.
    pub fn of(error: &anyhow::Error) -> Option<Self> {
        error.downcast_ref::<Self>().copied()
    }
}

fn closed_stdout() -> anyhow::Error {
    anyhow::Error::new(TransportFailure::Closed).context("MCP server closed stdout")
}

#[derive(Debug)]
pub enum ToolResponse {
    Success(Value),
    Error { code: i64, payload: Value },
}

#[derive(Debug)]
pub struct ToolDefinition {
    pub name: String,
    pub input_schema: Value,
    /// Encoded size of the complete `tools/list` entry for this tool
    /// (name, description, schema, annotations). Measured in memory only.
    pub entry_bytes: usize,
    /// The tool's declared `outputSchema`, when present. Structural
    /// metadata only; never persisted.
    pub output_schema: Option<Value>,
    /// The `readOnlyHint` and `destructiveHint` tool annotations, when
    /// declared as booleans. Structural metadata only; never persisted.
    pub read_only_hint: Option<bool>,
    pub destructive_hint: Option<bool>,
}

impl ToolDefinition {
    pub fn declared_output_schema(&self) -> Option<&Value> {
        self.output_schema.as_ref()
    }
}

/// A boolean hint from a `tools/list` entry's `annotations` object.
pub(crate) fn annotation_hint(entry: &Value, hint: &str) -> Option<bool> {
    entry.get("annotations")?.get(hint)?.as_bool()
}

#[derive(Debug)]
pub struct ToolCatalog {
    pub tools: Vec<ToolDefinition>,
    pub encoded_bytes: usize,
}

/// How a server resolved a cancelled request id.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CancellationOutcome {
    /// The request id was never resolved, or the server answered with the
    /// structured "Request cancelled" error (-32800): the cancellation was
    /// observed and honored.
    Honored,
    /// The server completed the work and returned the full result as if
    /// the cancellation had never been sent.
    Ignored,
    /// The server answered the cancelled request with an error that shows
    /// no cancellation awareness.
    Errored,
}

pub struct McpClient {
    child: Child,
    stdin: Option<ChildStdin>,
    lines: Receiver<std::io::Result<Vec<u8>>>,
    next_id: u64,
    response_timeout: Duration,
    /// Requests given up on at their timeout. A late response to one of
    /// them is discarded instead of being mistaken for a misrouted reply.
    abandoned: std::collections::HashSet<u64>,
    /// The server's advertised capabilities from `initialize`.
    capabilities: Option<Value>,
    /// The protocol version the server answered the handshake with.
    protocol_version: Option<String>,
    /// Server notifications observed while waiting for request responses;
    /// `wait_for_resource_update` drains this buffer first so a
    /// notification that raced ahead of an interleaved response is never
    /// lost.
    notifications: Vec<Value>,
}

impl McpClient {
    pub fn capabilities(&self) -> Option<Value> {
        self.capabilities.clone()
    }
    pub fn spawn(command: &[String]) -> anyhow::Result<Self> {
        let (program, args) = command
            .split_first()
            .ok_or_else(|| anyhow::anyhow!("server command must not be empty"))?;
        let mut child = Command::new(program)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .context("spawning MCP server")?;
        let stdin = child.stdin.take().context("opening MCP server stdin")?;
        let stdout = child.stdout.take().context("opening MCP server stdout")?;
        let (sender, lines) = mpsc::channel();
        thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            loop {
                let mut line = Vec::new();
                match reader.read_until(b'\n', &mut line) {
                    Ok(0) => break,
                    Ok(_) => {
                        if sender.send(Ok(line)).is_err() {
                            break;
                        }
                    }
                    Err(error) => {
                        let _ = sender.send(Err(error));
                        break;
                    }
                }
            }
        });
        Ok(Self {
            child,
            stdin: Some(stdin),
            lines,
            next_id: 1,
            response_timeout: DEFAULT_RESPONSE_TIMEOUT,
            abandoned: std::collections::HashSet::new(),
            capabilities: None,
            protocol_version: None,
            notifications: Vec::new(),
        })
    }

    /// The protocol version the server answered `initialize` with.
    pub fn protocol_version(&self) -> Option<String> {
        self.protocol_version.clone()
    }

    /// How long a request waits for its response; `None` restores
    /// [`DEFAULT_RESPONSE_TIMEOUT`].
    pub fn set_response_timeout(&mut self, timeout: Option<Duration>) {
        self.response_timeout = timeout.unwrap_or(DEFAULT_RESPONSE_TIMEOUT);
    }

    /// Run one full `initialize` handshake with the given protocol version
    /// and return the server's reply verbatim. Used by the
    /// protocol-negotiation probe to observe version selection directly;
    /// unlike `initialize` it neither sends `notifications/initialized`
    /// nor records capabilities, so it can run repeatedly against a
    /// server that expects a fresh handshake each time.
    pub fn initialize_raw(&mut self, protocol_version: &str) -> anyhow::Result<Value> {
        self.request(
            "initialize",
            json!({
                "protocolVersion": protocol_version,
                "capabilities": {},
                "clientInfo": {"name": "mcpeval", "version": env!("CARGO_PKG_VERSION")}
            }),
        )
    }

    /// Issue a `tools/call` and answer any server→client request (for
    /// example `sampling/createMessage` or `elicitation/create`) through
    /// `respond`. Returns the tool outcome and how many server→client
    /// requests arrived. Unparseable and notification frames are skipped;
    /// the session is sequential, so a mismatched id fails fast.
    pub fn call_tool_observing(
        &mut self,
        tool: &str,
        arguments: &Value,
        respond: &mut dyn FnMut(&str, &Value) -> Option<Value>,
        max_server_requests: u64,
    ) -> anyhow::Result<(ToolResponse, u64)> {
        let id = self.next_id;
        self.next_id += 1;
        self.write(&json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "tools/call",
            "params": {"name": tool, "arguments": arguments}
        }))?;
        let mut server_requests = 0u64;
        loop {
            let raw = match self.lines.recv_timeout(self.response_timeout) {
                Ok(Ok(raw)) => raw,
                Ok(Err(_)) | Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return Err(closed_stdout())
                }
                Err(mpsc::RecvTimeoutError::Timeout) => return Err(self.abandon(id)),
            };
            let frame: Value = match serde_json::from_slice(&raw) {
                Ok(value) => value,
                Err(_) => continue,
            };
            let Some(object) = frame.as_object() else {
                continue;
            };
            let is_response = object.contains_key("id")
                && (object.contains_key("result") || object.contains_key("error"));
            if is_response {
                if self.is_abandoned_reply(object) {
                    continue;
                }
                if object.get("id").and_then(Value::as_u64) != Some(id) {
                    bail!("MCP response id does not match request");
                }
                let response = classify_tool_response(&frame)?;
                return Ok((response, server_requests));
            }
            // Server→client request: answer it through the observer and
            // bound the flood.
            let Some(method) = object.get("method").and_then(Value::as_str) else {
                continue;
            };
            if method.starts_with("notifications/") {
                continue;
            }
            let Some(request_id) = object.get("id") else {
                continue;
            };
            server_requests += 1;
            if server_requests > max_server_requests {
                bail!("server issued more sub-requests than the declared bound");
            }
            let reply = respond(method, object.get("params").unwrap_or(&Value::Null));
            let reply = match reply {
                Some(result) => json!({"jsonrpc": "2.0", "id": request_id, "result": result}),
                None => json!({"jsonrpc": "2.0", "id": request_id,
                    "error": {"code": -32601, "message": "method unavailable"}}),
            };
            self.write(&reply)?;
        }
    }

    pub fn initialize(&mut self) -> anyhow::Result<()> {
        let response = self.request(
            "initialize",
            json!({
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": {"name": "mcpeval", "version": env!("CARGO_PKG_VERSION")}
            }),
        )?;
        self.capabilities = response
            .get("result")
            .and_then(|result| result.get("capabilities"))
            .cloned();
        self.protocol_version = response
            .get("result")
            .and_then(|result| result.get("protocolVersion"))
            .and_then(Value::as_str)
            .map(str::to_owned);
        self.notify("notifications/initialized", json!({}))
    }

    pub fn list_tools(&mut self) -> anyhow::Result<Vec<String>> {
        Ok(self
            .list_tools_catalog()?
            .tools
            .into_iter()
            .map(|tool| tool.name)
            .collect())
    }

    pub fn list_tools_catalog(&mut self) -> anyhow::Result<ToolCatalog> {
        let response = self.request("tools/list", json!({}))?;
        let tools = response
            .get("result")
            .and_then(|result| result.get("tools"))
            .and_then(Value::as_array)
            .context("tools/list response is missing tools")?;
        let encoded_bytes = serde_json::to_vec(tools)?.len();
        let tools = tools
            .iter()
            .map(|tool| {
                let name = tool
                    .get("name")
                    .and_then(Value::as_str)
                    .context("tool entry is missing a name")?;
                if !privacy::valid_tool(name) {
                    bail!("tool entry has an invalid name");
                }
                let input_schema = tool
                    .get("inputSchema")
                    .cloned()
                    .context("tool entry is missing inputSchema")?;
                if !input_schema.is_object() {
                    bail!("tool inputSchema is not an object");
                }
                Ok(ToolDefinition {
                    name: name.to_owned(),
                    input_schema,
                    entry_bytes: serde_json::to_vec(tool)?.len(),
                    output_schema: tool
                        .get("outputSchema")
                        .filter(|schema| schema.is_object())
                        .cloned(),
                    read_only_hint: annotation_hint(tool, "readOnlyHint"),
                    destructive_hint: annotation_hint(tool, "destructiveHint"),
                })
            })
            .collect::<anyhow::Result<Vec<_>>>()?;
        Ok(ToolCatalog {
            tools,
            encoded_bytes,
        })
    }

    pub fn call_tool(&mut self, tool: &str, arguments: &Value) -> anyhow::Result<ToolResponse> {
        let response = self.request("tools/call", json!({"name": tool, "arguments": arguments}))?;
        classify_tool_response(&response)
    }

    /// Raw JSON-RPC request for probes that inspect envelope structure
    /// (for example pagination cursors) rather than tool semantics.
    pub fn raw_request(&mut self, method: &str, params: Value) -> anyhow::Result<Value> {
        self.request(method, params)
    }

    /// Issue a `tools/call` and immediately cancel it, then classify how
    /// the server resolved the cancelled request id within the grace
    /// window. The probe contract: a cancellation-honoring server sends
    /// no response for the id.
    pub fn cancel_tool_call(
        &mut self,
        tool: &str,
        arguments: &Value,
        reason: &str,
        grace: Duration,
    ) -> anyhow::Result<CancellationOutcome> {
        let id = self.next_id;
        self.next_id += 1;
        self.write(&json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "tools/call",
            "params": {"name": tool, "arguments": arguments}
        }))?;
        self.notify(
            "notifications/cancelled",
            json!({"requestId": id, "reason": reason}),
        )?;
        // The only frames that can still arrive for this id are the
        // server's resolution. Anything without our id is skipped (the
        // session is otherwise idle, so there should be nothing else).
        let deadline = Instant::now() + grace;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Ok(CancellationOutcome::Honored);
            }
            let raw = match self.lines.recv_timeout(remaining) {
                Ok(Ok(raw)) => raw,
                Err(mpsc::RecvTimeoutError::Timeout) => return Ok(CancellationOutcome::Honored),
                Ok(Err(_)) | Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return Err(closed_stdout())
                }
            };
            let response: Value = match serde_json::from_slice(&raw) {
                Ok(value) => value,
                Err(_) => continue,
            };
            let Some(object) = response.as_object() else {
                continue;
            };
            if object.get("id").and_then(Value::as_u64) != Some(id) {
                continue;
            }
            if object.contains_key("error") {
                // The structured "Request cancelled" error (-32800) is how
                // production servers acknowledge a cancellation while
                // keeping the envelope well-formed: it counts as honored.
                let acknowledged = object
                    .get("error")
                    .and_then(|error| error.get("code"))
                    .and_then(Value::as_i64)
                    == Some(-32800);
                return Ok(if acknowledged {
                    CancellationOutcome::Honored
                } else {
                    CancellationOutcome::Errored
                });
            }
            return Ok(CancellationOutcome::Ignored);
        }
    }

    fn notify(&mut self, method: &str, params: Value) -> anyhow::Result<()> {
        self.write(&json!({"jsonrpc": "2.0", "method": method, "params": params}))
    }

    /// Wait for the server's `notifications/resources/updated` for `uri`
    /// within `wait`. The subscription itself is issued by the caller (the
    /// probe subscribes before firing its trigger, so the notification
    /// cannot race ahead of the subscription). Notifications observed
    /// while waiting for earlier responses are drained first — a
    /// notification the server emitted before answering the trigger call
    /// still counts. Ok(()) when the notification arrived, Err(()) on
    /// timeout or transport end.
    pub fn wait_for_resource_update(&mut self, uri: &str, wait: Duration) -> bool {
        let matches_uri = |frame: &Value| -> bool {
            frame.get("method").and_then(Value::as_str) == Some("notifications/resources/updated")
                && frame
                    .get("params")
                    .and_then(|params| params.get("uri"))
                    .and_then(Value::as_str)
                    == Some(uri)
        };
        if let Some(index) = self.notifications.iter().position(matches_uri) {
            self.notifications.remove(index);
            return true;
        }
        let deadline = Instant::now() + wait;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return false;
            }
            let raw = match self.lines.recv_timeout(remaining) {
                Ok(raw) => raw,
                Err(mpsc::RecvTimeoutError::Timeout)
                | Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return false;
                }
            };
            let raw = match raw {
                Ok(raw) => raw,
                Err(_) => return false,
            };
            let Ok(frame) = serde_json::from_slice::<Value>(&raw) else {
                continue;
            };
            if matches_uri(&frame) {
                return true;
            }
            if frame.get("method").and_then(Value::as_str).is_some() {
                self.notifications.push(frame);
            }
        }
    }

    pub fn unsubscribe(&mut self, uri: &str) -> anyhow::Result<Value> {
        self.raw_request("resources/unsubscribe", json!({"uri": uri}))
    }

    /// Read a resource by URI.
    pub fn read_resource(&mut self, uri: &str) -> anyhow::Result<Value> {
        self.raw_request("resources/read", json!({"uri": uri}))
    }

    /// Issue one `completion/complete` request and return the response
    /// envelope verbatim for the completion probe to classify.
    pub fn complete(
        &mut self,
        ref_type: &str,
        ref_uri: &str,
        argument_name: &str,
        argument_value: &str,
    ) -> anyhow::Result<Value> {
        let reference = if ref_type == "ref/resource" {
            json!({"type": ref_type, "uri": ref_uri})
        } else {
            json!({"type": ref_type, "name": ref_uri})
        };
        self.raw_request(
            "completion/complete",
            json!({
                "ref": reference,
                "argument": {"name": argument_name, "value": argument_value}
            }),
        )
    }

    fn request(&mut self, method: &str, params: Value) -> anyhow::Result<Value> {
        let id = self.next_id;
        self.next_id += 1;
        self.write(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}))?;
        let deadline = Instant::now() + self.response_timeout;
        loop {
            let raw = match self
                .lines
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            {
                Ok(Ok(raw)) => raw,
                Err(mpsc::RecvTimeoutError::Timeout) => return Err(self.abandon(id)),
                Ok(Err(_)) | Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return Err(closed_stdout())
                }
            };
            // Real servers print human-readable banners on stdout and
            // interleave unsolicited notifications. Neither is a response:
            // unparseable lines are skipped; notifications are buffered
            // for `wait_for_resource_update`, which drains the buffer
            // before touching the wire.
            let response: Value = match serde_json::from_slice(&raw) {
                Ok(value) => value,
                Err(_) => continue,
            };
            let Some(object) = response.as_object() else {
                continue;
            };
            if object.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
                continue;
            }
            if !object.contains_key("id") {
                if object.get("method").and_then(Value::as_str).is_some() {
                    self.notifications.push(response);
                }
                continue;
            }
            if self.is_abandoned_reply(object) {
                continue;
            }
            // The client is strictly sequential, so a frame that carries an
            // id other than the outstanding request's is a server defect —
            // stale, duplicated, or misrouted. Fail fast with a content-free
            // error instead of waiting out the timeout.
            if object.get("id").and_then(Value::as_u64) != Some(id) {
                bail!("MCP response id does not match request");
            }
            if object.contains_key("result") == object.contains_key("error") {
                bail!("MCP response must contain exactly one result or error");
            }
            return Ok(response);
        }
    }

    /// Give up on request `id` at its timeout.
    fn abandon(&mut self, id: u64) -> anyhow::Error {
        self.abandoned.insert(id);
        TransportFailure::Timeout.into()
    }

    /// Whether `frame` is the late response to an abandoned request; the
    /// id is forgotten once its reply has been discarded.
    fn is_abandoned_reply(&mut self, frame: &serde_json::Map<String, Value>) -> bool {
        frame
            .get("id")
            .and_then(Value::as_u64)
            .is_some_and(|id| self.abandoned.remove(&id))
    }

    fn write(&mut self, value: &Value) -> anyhow::Result<()> {
        let stdin = self.stdin.as_mut().ok_or_else(closed_stdout)?;
        let mut frame = serde_json::to_vec(value)?;
        frame.push(b'\n');
        stdin
            .write_all(&frame)
            .and_then(|()| stdin.flush())
            .map_err(|error| {
                anyhow::Error::new(error)
                    .context(TransportFailure::Closed)
                    .context("writing to the MCP server")
            })
    }
}

impl Drop for McpClient {
    fn drop(&mut self) {
        self.stdin.take();
        match self.child.try_wait() {
            Ok(Some(_)) => {}
            Ok(None) | Err(_) => {
                let _ = self.child.kill();
                let _ = self.child.wait();
            }
        }
    }
}

/// Classify a `tools/call` response envelope into a ToolResponse. The
/// protocol's second error channel — a successful envelope whose
/// `result.isError` is true — is a tool failure: production servers,
/// including the official reference server, report missing or invalid
/// arguments this way, and probes must honor it or a broken tool passes.
pub fn classify_tool_response(response: &Value) -> anyhow::Result<ToolResponse> {
    if let Some(result) = response.get("result") {
        if result.get("isError").and_then(Value::as_bool) == Some(true) {
            let message = result
                .get("content")
                .and_then(Value::as_array)
                .and_then(|content| content.first())
                .and_then(|item| item.get("text"))
                .and_then(Value::as_str)
                .unwrap_or("tool error")
                .to_owned();
            return Ok(ToolResponse::Error {
                code: -32000,
                // The shim's journal synthesizes the same constant for
                // isError results, so promotion groups one defect seen
                // through the shim with the same defect seen through a
                // probe (error_info carries scalar or identifier codes).
                payload: json!({"code": "tool-error", "message": message}),
            });
        }
        return Ok(ToolResponse::Success(result.clone()));
    }
    let error = response
        .get("error")
        .and_then(Value::as_object)
        .context("tools/call response has no result or error")?;
    let code = error
        .get("code")
        .and_then(Value::as_i64)
        .context("tools/call error has no integer code")?;
    Ok(ToolResponse::Error {
        code,
        payload: Value::Object(error.clone()),
    })
}
