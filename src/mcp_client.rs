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
const RESPONSE_TIMEOUT: Duration = Duration::from_secs(30);

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
}

impl ToolDefinition {
    pub fn declared_output_schema(&self) -> Option<&Value> {
        self.output_schema.as_ref()
    }
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
            capabilities: None,
            protocol_version: None,
            notifications: Vec::new(),
        })
    }

    /// The protocol version the server answered `initialize` with.
    pub fn protocol_version(&self) -> Option<String> {
        self.protocol_version.clone()
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
            let raw =
                self.lines
                    .recv_timeout(RESPONSE_TIMEOUT)
                    .map_err(|error| match error {
                        mpsc::RecvTimeoutError::Timeout => {
                            anyhow::anyhow!("MCP response timed out")
                        }
                        mpsc::RecvTimeoutError::Disconnected => {
                            anyhow::anyhow!("MCP server closed stdout")
                        }
                    })??;
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
                if object.get("id").and_then(Value::as_u64) != Some(id) {
                    bail!("MCP response id does not match request");
                }
                let response = if let Some(result) = object.get("result") {
                    ToolResponse::Success(result.clone())
                } else {
                    let error = object
                        .get("error")
                        .context("response has neither result nor error")?;
                    let code = error
                        .get("code")
                        .and_then(Value::as_i64)
                        .context("tools/call error has no integer code")?;
                    ToolResponse::Error {
                        code,
                        payload: error.clone(),
                    }
                };
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
        if let Some(result) = response.get("result") {
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
                Ok(raw) => raw,
                Err(mpsc::RecvTimeoutError::Timeout) => return Ok(CancellationOutcome::Honored),
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    bail!("MCP server closed stdout")
                }
            }?;
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

    fn request(&mut self, method: &str, params: Value) -> anyhow::Result<Value> {
        let id = self.next_id;
        self.next_id += 1;
        self.write(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}))?;
        let deadline = Instant::now() + RESPONSE_TIMEOUT;
        loop {
            let raw = match self
                .lines
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            {
                Ok(raw) => raw,
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    bail!("MCP response timed out")
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    bail!("MCP server closed stdout")
                }
            }?;
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

    fn write(&mut self, value: &Value) -> anyhow::Result<()> {
        let stdin = self.stdin.as_mut().context("MCP server stdin is closed")?;
        serde_json::to_writer(&mut *stdin, value)?;
        stdin.write_all(b"\n")?;
        stdin.flush()?;
        Ok(())
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
