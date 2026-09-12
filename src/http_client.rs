use std::io::Read;
use std::net::IpAddr;
use std::time::Duration;

use anyhow::{bail, Context};
use serde_json::{json, Value};

use crate::mcp_client::{CancellationOutcome, ToolCatalog, ToolDefinition, ToolResponse};
use crate::privacy;

pub(crate) const PROTOCOL_VERSION: &str = "2025-06-18";
const MAX_RESPONSE_BYTES: usize = 8 * 1024 * 1024;

pub struct HttpMcpClient {
    agent: ureq::Agent,
    endpoint: String,
    session_id: Option<String>,
    next_id: u64,
    /// The server's advertised capabilities from `initialize`.
    capabilities: Option<Value>,
    /// The protocol version the server answered the handshake with.
    protocol_version: Option<String>,
}

/// One event collected from the standing GET SSE stream a Streamable HTTP
/// server keeps for its session: either a server→client request (to be
/// answered) or a notification.
#[derive(Debug)]
pub enum StreamEvent {
    /// A server→client request carrying method, params, and its id.
    Request {
        method: String,
        id: Value,
        params: Value,
    },
    /// A notification with its method and params.
    Notification { method: String, params: Value },
}

impl HttpMcpClient {
    pub fn capabilities(&self) -> Option<Value> {
        self.capabilities.clone()
    }
    pub fn connect(endpoint: &str, allow_remote: bool) -> anyhow::Result<Self> {
        let endpoint = validate_endpoint(endpoint, allow_remote)?;
        Ok(Self {
            agent: ureq::AgentBuilder::new()
                .redirects(0)
                .timeout_connect(Duration::from_secs(5))
                .timeout_read(Duration::from_secs(5))
                .timeout_write(Duration::from_secs(5))
                .build(),
            endpoint,
            session_id: None,
            next_id: 1,
            capabilities: None,
            protocol_version: None,
        })
    }

    /// The protocol version the server answered `initialize` with.
    pub fn protocol_version(&self) -> Option<String> {
        self.protocol_version.clone()
    }

    /// Run one `initialize` handshake with the given protocol version and
    /// return the server's reply verbatim. Mirrors the stdio client: the
    /// reply is returned without recording capabilities so the
    /// negotiation probe can run repeated handshakes against the same
    /// HTTP session.
    pub fn initialize_raw(&mut self, protocol_version: &str) -> anyhow::Result<Value> {
        let response = self.request(
            "initialize",
            json!({
                "protocolVersion": protocol_version,
                "capabilities": {},
                "clientInfo": {"name": "mcpeval", "version": env!("CARGO_PKG_VERSION")}
            }),
        )?;
        Ok(response)
    }

    /// Run one `initialize` handshake with the given protocol version on a
    /// fresh session and return the server's reply verbatim. Used by the
    /// protocol-negotiation probe; never touches this client's state.
    pub fn initialize_raw_on_fresh(
        endpoint: &str,
        allow_remote: bool,
        protocol_version: &str,
    ) -> anyhow::Result<Value> {
        let probe = Self::connect(endpoint, allow_remote)?;
        let message = json!({
            "jsonrpc": "2.0",
            "id": probe.next_id,
            "method": "initialize",
            "params": {
                "protocolVersion": protocol_version,
                "capabilities": {},
                "clientInfo": {"name": "mcpeval", "version": env!("CARGO_PKG_VERSION")}
            }
        });
        let response = probe.post(&message)?;
        if response.status() != 200 {
            bail!("MCP request returned an unexpected HTTP status");
        }
        let content_type = response
            .header("Content-Type")
            .and_then(|value| value.split(';').next())
            .unwrap_or("")
            .to_owned();
        let body = read_bounded(response.into_reader())?;
        let value: Value = match content_type.trim() {
            "application/json" => {
                serde_json::from_slice(&body).context("MCP HTTP response is not valid JSON")?
            }
            "text/event-stream" => parse_sse_response(&body, probe.next_id)?,
            _ => bail!("MCP HTTP response has an unsupported content type"),
        };
        Ok(value)
    }

    pub fn initialize(&mut self) -> anyhow::Result<()> {
        let response = self.request(
            "initialize",
            json!({
                "protocolVersion": PROTOCOL_VERSION,
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
        crate::mcp_client::classify_tool_response(&response)
    }

    /// Raw JSON-RPC request for probes that inspect envelope structure
    /// (for example pagination cursors) rather than tool semantics.
    pub fn raw_request(&mut self, method: &str, params: Value) -> anyhow::Result<Value> {
        self.request(method, params)
    }

    /// Issue a `tools/call` and answer any server→client request (for
    /// example `sampling/createMessage` or `elicitation/create`) through
    /// `respond`. On Streamable HTTP the server's sub-requests and the
    /// tool outcome can share the POST's SSE stream; the sub-requests are
    /// counted (bounded) and acknowledged, then the tool outcome is
    /// resolved from the same stream.
    pub fn call_tool_observing(
        &mut self,
        tool: &str,
        arguments: &Value,
        _respond: &mut dyn FnMut(&str, &Value) -> Option<Value>,
        max_server_requests: u64,
    ) -> anyhow::Result<(ToolResponse, u64)> {
        let id = self.next_id;
        self.next_id += 1;
        let message = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "tools/call",
            "params": {"name": tool, "arguments": arguments}
        });
        let response = self.post(&message)?;
        if response.status() != 200 {
            bail!("MCP request returned an unexpected HTTP status");
        }
        let content_type = response
            .header("Content-Type")
            .and_then(|value| value.split(';').next())
            .unwrap_or("")
            .to_owned();
        let body = read_bounded(response.into_reader())?;
        let mut server_requests = 0u64;
        let frames: Vec<Value> = match content_type.trim() {
            "application/json" => {
                vec![serde_json::from_slice(&body).context("MCP HTTP response is not valid JSON")?]
            }
            "text/event-stream" => sse_frames(&body)?,
            _ => bail!("MCP HTTP response has an unsupported content type"),
        };
        // First answer every server→client request in the stream, then
        // resolve the tool outcome.
        for frame in &frames {
            let Some(object) = frame.as_object() else {
                continue;
            };
            let is_tool_response = object.get("id").and_then(Value::as_u64) == Some(id)
                && (object.contains_key("result") || object.contains_key("error"));
            if is_tool_response {
                continue;
            }
            if let Some(method) = object.get("method").and_then(Value::as_str) {
                if method.starts_with("notifications/") || !object.contains_key("id") {
                    continue;
                }
                server_requests += 1;
                if server_requests > max_server_requests {
                    bail!("server issued more sub-requests than the declared bound");
                }
                // Sub-requests inside the stream cannot be answered on the
                // same POST; the spec routes replies through a separate
                // POST. Bound-exceeding or unanswerable requests are
                // counted and the tool result will carry the outcome.
                let _ = self.post(&json!({
                    "jsonrpc": "2.0", "id": object.get("id"),
                    "error": {"code": -32601, "message": "method unavailable"}
                }));
            }
        }
        for frame in &frames {
            let Some(object) = frame.as_object() else {
                continue;
            };
            if object.get("id").and_then(Value::as_u64) != Some(id) {
                continue;
            }
            if object.contains_key("result") || object.contains_key("error") {
                let response =
                    crate::mcp_client::classify_tool_response(&Value::Object(object.clone()))?;
                return Ok((response, server_requests));
            }
        }
        bail!("tools/call stream ended without the tool response")
    }

    pub fn unsubscribe(&mut self, uri: &str) -> anyhow::Result<Value> {
        self.raw_request("resources/unsubscribe", json!({"uri": uri}))
    }

    /// Read a resource by URI.
    pub fn read_resource(&mut self, uri: &str) -> anyhow::Result<Value> {
        self.raw_request("resources/read", json!({"uri": uri}))
    }

    /// Issue one `completion/complete` request for the completion probe.
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

    /// Wait for the server's `notifications/resources/updated` carrying
    /// `uri` on the session's GET stream, up to `wait`. The subscription
    /// is issued by the caller. Servers that answer the GET with 405 (or
    /// any non-200) signal "no standalone stream", which counts as a
    /// missing notification.
    pub fn wait_for_resource_update(&mut self, uri: &str, wait: Duration) -> bool {
        let mut request = self
            .agent
            .get(&self.endpoint)
            .timeout(wait)
            .set("Accept", "text/event-stream");
        if let Some(session_id) = &self.session_id {
            request = request.set("Mcp-Session-Id", session_id);
        }
        if let Ok(authorization) = std::env::var("MCPEVAL_HTTP_AUTHORIZATION") {
            if authorization.is_empty()
                || authorization.len() > 8192
                || authorization.bytes().any(|byte| byte.is_ascii_control())
            {
                // A transport-level failure counts as "no notification
                // observed" for the subscription contract.
                return false;
            }
            request = request.set("Authorization", &authorization);
        }
        let response = match request.call() {
            Ok(response) => response,
            // Read timeout with no matching frame: no notification arrived.
            Err(error) if request_timed_out(&error) => return false,
            Err(_) => return false,
        };
        if response.status() != 200 {
            return false;
        }
        let body = match read_bounded(response.into_reader()) {
            Ok(body) => body,
            Err(_) => return false,
        };
        let Ok(frames) = sse_frames(&body) else {
            return false;
        };
        for frame in frames {
            let method = frame.get("method").and_then(Value::as_str).unwrap_or("");
            let params = frame.get("params").cloned().unwrap_or(Value::Null);
            if method == "notifications/resources/updated"
                && params.get("uri").and_then(Value::as_str) == Some(uri)
            {
                return true;
            }
        }
        false
    }

    /// Issue a `tools/call` on its own connection, send
    /// `notifications/cancelled` for it, then classify how the server
    /// resolved the cancelled request.
    ///
    /// The call POST runs on a worker thread, on its own connection with
    /// `grace` as its timeout, so the cancellation can land while it is in
    /// flight. The classification follows what real production servers do
    /// (the reference gateway answers a cancelled request with the
    /// structured `-32800 Request cancelled` error): a `-32800` error means
    /// the server observed the cancellation, a full result means the server
    /// ignored it, any other error means the server answered the cancelled
    /// id without cancellation awareness, and no response within `grace`
    /// (a read timeout, or an SSE stream that ends without a frame for the
    /// id) counts as silence, which honors the cancellation.
    pub fn cancel_tool_call(
        &mut self,
        tool: &str,
        arguments: &Value,
        reason: &str,
        grace: Duration,
    ) -> anyhow::Result<CancellationOutcome> {
        let id = self.next_id;
        self.next_id += 1;
        let (worker_agent, worker_endpoint, worker_session) = self.clone_for_request();
        let call_message = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "tools/call",
            "params": {"name": tool, "arguments": arguments}
        });
        let worker = std::thread::spawn(move || {
            worker_call(
                &worker_agent,
                &worker_endpoint,
                worker_session,
                call_message,
                id,
                grace,
            )
        });
        // Give the call a moment to reach the server before cancelling.
        std::thread::sleep(Duration::from_millis(50));
        self.notify(
            "notifications/cancelled",
            json!({"requestId": id, "reason": reason}),
        )?;
        match worker.join() {
            Ok(joined) => joined,
            Err(_) => bail!("cancelled call worker terminated unexpectedly"),
        }
    }

    fn clone_for_request(&self) -> (ureq::Agent, String, Option<String>) {
        (
            self.agent.clone(),
            self.endpoint.clone(),
            self.session_id.clone(),
        )
    }

    fn notify(&mut self, method: &str, params: Value) -> anyhow::Result<()> {
        let message = json!({"jsonrpc": "2.0", "method": method, "params": params});
        let response = self.post(&message)?;
        if response.status() != 202 {
            bail!("MCP notification returned an unexpected HTTP status");
        }
        Ok(())
    }

    fn request(&mut self, method: &str, params: Value) -> anyhow::Result<Value> {
        let id = self.next_id;
        self.next_id += 1;
        let message = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        let response = self.post(&message)?;
        if response.status() != 200 {
            bail!("MCP request returned an unexpected HTTP status");
        }
        if let Some(session_id) = response.header("Mcp-Session-Id") {
            if session_id.is_empty()
                || session_id.len() > 1024
                || !session_id.bytes().all(|byte| (0x21..=0x7e).contains(&byte))
            {
                bail!("MCP session header is invalid");
            }
            if self
                .session_id
                .as_deref()
                .is_some_and(|current| current != session_id)
            {
                bail!("MCP server changed the session identifier");
            }
            self.session_id = Some(session_id.to_owned());
        }
        let content_type = response
            .header("Content-Type")
            .and_then(|value| value.split(';').next())
            .unwrap_or("")
            .to_owned();
        let body = read_bounded(response.into_reader())?;
        let value = match content_type.trim() {
            "application/json" => {
                serde_json::from_slice(&body).context("MCP HTTP response is not valid JSON")?
            }
            "text/event-stream" => parse_sse_response(&body, id)?,
            _ => bail!("MCP HTTP response has an unsupported content type"),
        };
        validate_response(&value, id)?;
        Ok(value)
    }

    fn post(&self, message: &Value) -> anyhow::Result<ureq::Response> {
        let mut request = self
            .agent
            .post(&self.endpoint)
            .set("Accept", "application/json, text/event-stream")
            .set("Content-Type", "application/json")
            .set("MCP-Protocol-Version", PROTOCOL_VERSION);
        if let Some(session_id) = &self.session_id {
            request = request.set("Mcp-Session-Id", session_id);
        }
        if let Ok(authorization) = std::env::var("MCPEVAL_HTTP_AUTHORIZATION") {
            if authorization.is_empty()
                || authorization.len() > 8192
                || authorization.bytes().any(|byte| byte.is_ascii_control())
            {
                bail!("HTTP authorization environment value is invalid");
            }
            request = request.set("Authorization", &authorization);
        }
        request.send_json(message).map_err(|error| match error {
            ureq::Error::Status(status, _) => {
                anyhow::anyhow!("MCP HTTP request failed with status {status}")
            }
            other => anyhow::anyhow!("MCP HTTP request failed: {other}"),
        })
    }
}

pub(crate) fn validate_endpoint(endpoint: &str, allow_remote: bool) -> anyhow::Result<String> {
    let parsed = url::Url::parse(endpoint).context("HTTP endpoint is invalid")?;
    if !matches!(parsed.scheme(), "http" | "https")
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.query().is_some()
        || parsed.fragment().is_some()
    {
        bail!("HTTP endpoint must be a credential-free HTTP(S) URL without query or fragment");
    }
    let host = parsed
        .host_str()
        .context("HTTP endpoint is missing a host")?;
    let loopback = host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<IpAddr>()
            .is_ok_and(|address| address.is_loopback());
    if !loopback && !allow_remote {
        bail!("remote HTTP endpoints require --allow-remote-http");
    }
    if !loopback && parsed.scheme() != "https" {
        bail!("remote HTTP endpoints require HTTPS");
    }
    Ok(parsed.into())
}

fn read_bounded(mut reader: impl Read) -> anyhow::Result<Vec<u8>> {
    let mut body = Vec::new();
    reader
        .by_ref()
        .take((MAX_RESPONSE_BYTES + 1) as u64)
        .read_to_end(&mut body)?;
    if body.len() > MAX_RESPONSE_BYTES {
        bail!("MCP HTTP response exceeded the size limit");
    }
    Ok(body)
}

fn parse_sse_response(body: &[u8], expected_id: u64) -> anyhow::Result<Value> {
    find_sse_response(body, expected_id)?
        .context("MCP SSE stream ended without the matching response")
}

/// Scan an SSE body for the frame carrying `expected_id`; `None` means the
/// stream ended without one.
fn find_sse_response(body: &[u8], expected_id: u64) -> anyhow::Result<Option<Value>> {
    let text = std::str::from_utf8(body)
        .context("MCP SSE response is not UTF-8")?
        .replace("\r\n", "\n");
    for event in text.split("\n\n") {
        let data = event
            .lines()
            .filter_map(|line| line.strip_prefix("data:"))
            .map(str::trim_start)
            .collect::<Vec<_>>()
            .join("\n");
        if data.is_empty() {
            continue;
        }
        let value: Value = serde_json::from_str(&data).context("MCP SSE data is not valid JSON")?;
        if value.get("id").and_then(Value::as_u64) == Some(expected_id) {
            return Ok(Some(value));
        }
    }
    Ok(None)
}

/// ureq surfaces its overall request timeout as a transport error whose
/// source is an `io::Error` of kind `TimedOut` (WouldBlock is normalized).
fn request_timed_out(error: &ureq::Error) -> bool {
    match error {
        ureq::Error::Transport(transport) => std::error::Error::source(transport)
            .and_then(|source| source.downcast_ref::<std::io::Error>())
            .is_some_and(|io| io.kind() == std::io::ErrorKind::TimedOut),
        ureq::Error::Status(..) => false,
    }
}

fn read_timed_out(error: &anyhow::Error) -> bool {
    error
        .downcast_ref::<std::io::Error>()
        .is_some_and(|io| io.kind() == std::io::ErrorKind::TimedOut)
}

/// Run the in-flight call on a dedicated connection and classify the
/// server's resolution of a cancelled request. `grace` bounds the whole
/// request: a cancelled call that is never answered within it, or an SSE
/// stream that ends without a frame for the id, is silence and honors the
/// cancellation. A transport failure before any response, or a non-200
/// status, is an error, mirroring the stdio client's closed-stdout case.
fn worker_call(
    agent: &ureq::Agent,
    endpoint: &str,
    session_id: Option<String>,
    message: Value,
    id: u64,
    grace: Duration,
) -> anyhow::Result<CancellationOutcome> {
    let mut request = agent
        .post(endpoint)
        .timeout(grace)
        .set("Accept", "application/json, text/event-stream")
        .set("Content-Type", "application/json")
        .set("MCP-Protocol-Version", PROTOCOL_VERSION);
    if let Some(session_id) = &session_id {
        request = request.set("Mcp-Session-Id", session_id);
    }
    if let Ok(authorization) = std::env::var("MCPEVAL_HTTP_AUTHORIZATION") {
        if authorization.is_empty()
            || authorization.len() > 8192
            || authorization.bytes().any(|byte| byte.is_ascii_control())
        {
            bail!("HTTP authorization environment value is invalid");
        }
        request = request.set("Authorization", &authorization);
    }
    let response = match request.send_json(message) {
        Ok(response) => response,
        Err(error) if request_timed_out(&error) => return Ok(CancellationOutcome::Honored),
        Err(error) => bail!("cancelled call POST failed: {error}"),
    };
    if response.status() != 200 {
        bail!("cancelled call returned an unexpected HTTP status");
    }
    let content_type = response
        .header("Content-Type")
        .and_then(|value| value.split(';').next())
        .unwrap_or("")
        .to_owned();
    let body = match read_bounded(response.into_reader()) {
        Ok(body) => body,
        Err(error) if read_timed_out(&error) => return Ok(CancellationOutcome::Honored),
        Err(error) => return Err(error),
    };
    let value: Value = match content_type.trim() {
        "application/json" => {
            serde_json::from_slice(&body).context("cancelled call response is not valid JSON")?
        }
        "text/event-stream" => match find_sse_response(&body, id)? {
            Some(value) => value,
            None => return Ok(CancellationOutcome::Honored),
        },
        _ => bail!("cancelled call response has an unsupported content type"),
    };
    if let Some(error) = value.get("error") {
        let code = error.get("code").and_then(Value::as_i64);
        // -32800 "Request cancelled" is the structured cancellation
        // acknowledgement used by production servers.
        if code == Some(-32800) {
            return Ok(CancellationOutcome::Honored);
        }
        return Ok(CancellationOutcome::Errored);
    }
    if value.get("result").is_some() {
        return Ok(CancellationOutcome::Ignored);
    }
    bail!("cancelled call response is neither a result nor an error")
}

/// Split an SSE body into its data frames, parsed as JSON. Unparseable
/// and empty frames are skipped.
fn sse_frames(body: &[u8]) -> anyhow::Result<Vec<Value>> {
    let text = std::str::from_utf8(body)
        .context("MCP SSE response is not UTF-8")?
        .replace("\r\n", "\n");
    let mut frames = Vec::new();
    for event in text.split("\n\n") {
        let data = event
            .lines()
            .filter_map(|line| line.strip_prefix("data:"))
            .map(str::trim_start)
            .collect::<Vec<_>>()
            .join("\n");
        if data.is_empty() {
            continue;
        }
        let value: Value = serde_json::from_str(&data).context("MCP SSE data is not valid JSON")?;
        frames.push(value);
    }
    Ok(frames)
}

fn validate_response(value: &Value, id: u64) -> anyhow::Result<()> {
    let object = value.as_object().context("MCP response is not an object")?;
    if object.get("jsonrpc").and_then(Value::as_str) != Some("2.0")
        || object.get("id").and_then(Value::as_u64) != Some(id)
        || object.contains_key("result") == object.contains_key("error")
    {
        bail!("MCP HTTP response is invalid");
    }
    Ok(())
}
