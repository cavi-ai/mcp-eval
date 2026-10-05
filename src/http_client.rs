use std::io::{BufRead, BufReader, Read};
use std::time::Duration;

use anyhow::{bail, Context};
use serde_json::{json, Value};

use crate::mcp_client::{CancellationOutcome, ToolCatalog, ToolResponse, TransportFailure};

pub(crate) const PROTOCOL_VERSION: &str = "2025-06-18";
const MAX_RESPONSE_BYTES: usize = 8 * 1024 * 1024;

/// Connect, read, and write timeout when the manifest sets no `timeout_ms`.
pub const DEFAULT_IO_TIMEOUT: Duration = Duration::from_secs(5);

pub struct HttpMcpClient {
    agent: ureq::Agent,
    endpoint: String,
    session_id: Option<String>,
    next_id: u64,
    /// Whole-request deadline for each POST; `None` keeps the agent's
    /// five-second connect, read, and write timeouts.
    response_timeout: Option<Duration>,
    budget: Option<crate::evaluation_budget::Budget>,
    /// The server's advertised capabilities from `initialize`.
    capabilities: Option<Value>,
    /// The protocol version the server answered the handshake with.
    protocol_version: Option<String>,
    /// Discovery profiles expose an empty roots list, never host paths.
    empty_roots: bool,
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
                .timeout_connect(DEFAULT_IO_TIMEOUT)
                .timeout_read(DEFAULT_IO_TIMEOUT)
                .timeout_write(DEFAULT_IO_TIMEOUT)
                .build(),
            endpoint,
            session_id: None,
            next_id: 1,
            response_timeout: None,
            budget: None,
            capabilities: None,
            protocol_version: None,
            empty_roots: false,
        })
    }

    /// The protocol version the server answered `initialize` with.
    pub fn protocol_version(&self) -> Option<String> {
        self.protocol_version.clone()
    }

    /// Bound each request, response body included, by `timeout`; `None`
    /// restores the agent's per-operation timeouts
    /// ([`DEFAULT_IO_TIMEOUT`]).
    pub fn set_response_timeout(&mut self, timeout: Option<Duration>) {
        self.response_timeout = timeout;
    }

    pub(crate) fn set_evaluation_budget(&mut self, budget: crate::evaluation_budget::Budget) {
        self.budget = Some(budget);
    }

    fn finish<T>(&self, result: anyhow::Result<T>) -> anyhow::Result<T> {
        match &self.budget {
            Some(budget) => budget.finish(result),
            None => result,
        }
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
        self.initialize_with_capabilities(&json!({})).map(|_| ())
    }

    /// Initialize a discovery session with explicit client capabilities.
    /// The reply is returned in memory; instruction prose is never journaled.
    pub fn initialize_with_capabilities(&mut self, capabilities: &Value) -> anyhow::Result<Value> {
        self.empty_roots = capabilities.get("roots").is_some_and(Value::is_object);
        let response = self.request(
            "initialize",
            json!({
                "protocolVersion": PROTOCOL_VERSION,
                "capabilities": capabilities,
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
        self.notify("notifications/initialized", json!({}))?;
        Ok(response)
    }

    pub fn list_tools(&mut self) -> anyhow::Result<Vec<String>> {
        Ok(self
            .list_tools_catalog()?
            .tools
            .into_iter()
            .map(|tool| tool.name)
            .collect())
    }

    /// The whole catalog, every page; see `mcp_client::page_catalog`.
    pub fn list_tools_catalog(&mut self) -> anyhow::Result<ToolCatalog> {
        crate::mcp_client::page_catalog(|params| self.request("tools/list", params))
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
        let result = self.call_tool_observing_inner(tool, arguments, max_server_requests);
        self.finish(result)
    }

    fn call_tool_observing_inner(
        &mut self,
        tool: &str,
        arguments: &Value,
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
        let wait = match &self.budget {
            Some(budget) => match budget.timeout(wait) {
                Ok(wait) => wait,
                Err(_) => return false,
            },
            None => wait,
        };
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
        let grace = match &self.budget {
            Some(budget) => {
                budget.acquire()?;
                budget.timeout(grace)?
            }
            None => grace,
        };
        let id = self.next_id;
        self.next_id += 1;
        let (worker_agent, worker_endpoint, worker_session) = self.clone_for_request();
        let worker_budget = self.budget.clone();
        let call_message = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "tools/call",
            "params": {"name": tool, "arguments": arguments}
        });
        let worker = std::thread::spawn(move || {
            let grace = match &worker_budget {
                Some(budget) => budget.timeout(grace)?,
                None => grace,
            };
            let result = worker_call(
                &worker_agent,
                &worker_endpoint,
                worker_session,
                call_message,
                id,
                grace,
            );
            match worker_budget {
                Some(budget) => budget.finish(result),
                None => result,
            }
        });
        // Give the call a moment to reach the server before cancelling.
        std::thread::sleep(Duration::from_millis(50).min(grace));
        let notified = self.notify(
            "notifications/cancelled",
            json!({"requestId": id, "reason": reason}),
        );
        // Always join the owned request, even if notification delivery fails.
        let joined = match worker.join() {
            Ok(joined) => joined,
            Err(_) => Err(anyhow::anyhow!(
                "cancelled call worker terminated unexpectedly"
            )),
        };
        self.finish(notified.and(joined))
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
        let result = self.request_inner(method, params);
        self.finish(result)
    }

    fn request_inner(&mut self, method: &str, params: Value) -> anyhow::Result<Value> {
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
        let value = if content_type.trim() == "text/event-stream" && self.empty_roots {
            self.discovery_sse(response.into_reader(), id)?
        } else {
            let body = read_bounded(response.into_reader())?;
            match content_type.trim() {
                "application/json" => {
                    serde_json::from_slice(&body).context("MCP HTTP response is not valid JSON")?
                }
                "text/event-stream" => parse_sse_response(&body, id)?,
                _ => bail!("MCP HTTP response has an unsupported content type"),
            }
        };
        validate_response(&value, id)?;
        Ok(value)
    }

    /// Answer roots requests as events arrive: waiting for EOF can deadlock
    /// a server that waits for our separate reply POST before sending its result.
    fn discovery_sse(&self, reader: impl Read, id: u64) -> anyhow::Result<Value> {
        let mut reader = BufReader::new(reader.take((MAX_RESPONSE_BYTES + 1) as u64));
        let mut bytes = 0;
        let mut roots_requests = 0;
        let mut event = String::new();
        loop {
            if let Some(budget) = &self.budget {
                budget.check()?;
            }
            let mut line = String::new();
            let read = reader.read_line(&mut line)?;
            if read == 0 {
                bail!("discovery stream ended without a response");
            }
            bytes += read;
            if bytes > MAX_RESPONSE_BYTES {
                bail!("discovery stream exceeded the size limit");
            }
            event.push_str(&line);
            if line != "\n" && line != "\r\n" {
                continue;
            }
            for frame in sse_frames(event.as_bytes())? {
                if frame.get("method").and_then(Value::as_str) == Some("roots/list")
                    && frame.get("id").is_some()
                {
                    if frame.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
                        bail!("invalid roots request");
                    }
                    roots_requests += 1;
                    if roots_requests > crate::standard::MAX_SERVER_REQUESTS {
                        bail!("roots discovery request limit exceeded");
                    }
                    let reply = self.post(
                        &json!({"jsonrpc": "2.0", "id": frame["id"], "result": {"roots": []}}),
                    )?;
                    if reply.status() != 202 {
                        bail!("roots reply was not accepted");
                    }
                } else if frame.get("method").is_none()
                    && frame.get("id").and_then(Value::as_u64) == Some(id)
                {
                    return Ok(frame);
                }
            }
            event.clear();
        }
    }

    fn post(&self, message: &Value) -> anyhow::Result<ureq::Response> {
        let timeout = match &self.budget {
            Some(budget) => {
                if message.get("id").is_some() && message.get("method").is_some() {
                    budget.acquire()?;
                }
                // Keep the agent's existing per-operation timeouts when none
                // was requested; the whole request still ends at the run deadline.
                Some(
                    budget.timeout(
                        self.response_timeout
                            .unwrap_or(crate::evaluation_budget::MAX_DURATION),
                    )?,
                )
            }
            None => self.response_timeout,
        };
        let mut request = self
            .agent
            .post(&self.endpoint)
            .set("Accept", "application/json, text/event-stream")
            .set("Content-Type", "application/json")
            .set("MCP-Protocol-Version", PROTOCOL_VERSION);
        if let Some(session_id) = &self.session_id {
            request = request.set("Mcp-Session-Id", session_id);
        }
        if let Some(timeout) = timeout {
            request = request.timeout(timeout);
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
            other => {
                let failure = if request_timed_out(&other) {
                    TransportFailure::Timeout
                } else {
                    TransportFailure::Closed
                };
                anyhow::Error::new(failure).context(format!("MCP HTTP request failed: {other}"))
            }
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
    let host = parsed.host().context("HTTP endpoint is missing a host")?;
    let loopback = crate::loopback::is_loopback_host(&host);
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
        .read_to_end(&mut body)
        .map_err(|error| {
            let failure = if error.kind() == std::io::ErrorKind::TimedOut {
                TransportFailure::Timeout
            } else {
                TransportFailure::Closed
            };
            anyhow::Error::new(error)
                .context(failure)
                .context("reading the MCP HTTP response")
        })?;
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

#[cfg(test)]
mod budget_tests {
    use super::*;
    use std::io::{BufRead, BufReader, Write};
    use std::net::{TcpListener, TcpStream};
    use std::time::Instant;

    fn request(stream: &TcpStream) -> Value {
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let mut reader = BufReader::new(stream);
        let mut length = 0;
        loop {
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            if line == "\r\n" {
                break;
            }
            if let Some(value) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                length = value.trim().parse().unwrap();
            }
        }
        let mut body = vec![0; length];
        reader.read_exact(&mut body).unwrap();
        serde_json::from_slice(&body).unwrap()
    }

    #[test]
    fn cancellation_notification_failure_still_joins_the_call_worker() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!("http://{}/mcp", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            let (mut call, _) = listener.accept().unwrap();
            let message = request(&call);
            let (mut notification, _) = listener.accept().unwrap();
            assert_eq!(request(&notification)["method"], "notifications/cancelled");
            notification
                .write_all(b"HTTP/1.1 500 Error\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                .unwrap();
            drop(notification);
            std::thread::sleep(Duration::from_millis(250));
            let body = json!({"jsonrpc":"2.0", "id":message["id"], "result":{}}).to_string();
            write!(call, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
        });
        let mut client = HttpMcpClient::connect(&endpoint, false).unwrap();
        let started = Instant::now();
        let result = client.cancel_tool_call("ok", &json!({}), "test", Duration::from_secs(2));
        let elapsed = started.elapsed();
        server.join().unwrap();
        assert!(result.is_err());
        assert!(
            elapsed >= Duration::from_millis(250),
            "the call worker was detached on notification failure"
        );
    }

    #[test]
    fn evaluation_budget_bounds_slow_http_response_bodies() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!("http://{}/mcp", listener.local_addr().unwrap());
        let worker = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let message = request(&stream);
            let body = json!({"jsonrpc":"2.0", "id":message["id"], "result":{}}).to_string();
            write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()).unwrap();
            for byte in body.bytes() {
                if stream.write_all(&[byte]).is_err() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(30));
            }
        });
        let mut client = HttpMcpClient::connect(&endpoint, false).unwrap();
        client.set_evaluation_budget(crate::evaluation_budget::Budget::new(
            Duration::from_millis(200),
            10,
        ));
        let started = Instant::now();
        let result = client.raw_request("ping", json!({}));
        let elapsed = started.elapsed();
        worker.join().unwrap();
        assert!(result
            .unwrap_err()
            .downcast_ref::<crate::evaluation_budget::Exhausted>()
            .is_some());
        assert!(
            elapsed < Duration::from_millis(800),
            "a trickling response extended the evaluation deadline: {elapsed:?}"
        );
    }

    #[test]
    fn evaluation_budget_exhaustion_on_later_http_catalog_page_is_not_partial_success() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!("http://{}/mcp", listener.local_addr().unwrap());
        let worker = std::thread::spawn(move || {
            for page in 0..2 {
                let (mut stream, _) = listener.accept().unwrap();
                let message = request(&stream);
                let body = json!({"jsonrpc":"2.0", "id":message["id"], "result":{
                    "tools":[{"name":format!("tool{page}"),"inputSchema":{"type":"object"}}],
                    "nextCursor":format!("cursor{page}")
                }})
                .to_string();
                write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
            }
        });
        let mut client = HttpMcpClient::connect(&endpoint, false).unwrap();
        client.set_evaluation_budget(crate::evaluation_budget::Budget::new(
            Duration::from_secs(10),
            2,
        ));
        let result = client.list_tools_catalog();
        worker.join().unwrap();
        assert!(result
            .unwrap_err()
            .downcast_ref::<crate::evaluation_budget::Exhausted>()
            .is_some());
    }
}
