use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::Duration;

use anyhow::{bail, Context};
use serde_json::{json, Value};

use crate::privacy;

const RESPONSE_TIMEOUT: Duration = Duration::from_secs(5);

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
}

#[derive(Debug)]
pub struct ToolCatalog {
    pub tools: Vec<ToolDefinition>,
    pub encoded_bytes: usize,
}

pub struct McpClient {
    child: Child,
    stdin: Option<ChildStdin>,
    lines: Receiver<std::io::Result<Vec<u8>>>,
    next_id: u64,
}

impl McpClient {
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
        })
    }

    pub fn initialize(&mut self) -> anyhow::Result<()> {
        self.request(
            "initialize",
            json!({
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": {"name": "mcpeval", "version": env!("CARGO_PKG_VERSION")}
            }),
        )?;
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

    fn notify(&mut self, method: &str, params: Value) -> anyhow::Result<()> {
        self.write(&json!({"jsonrpc": "2.0", "method": method, "params": params}))
    }

    fn request(&mut self, method: &str, params: Value) -> anyhow::Result<Value> {
        let id = self.next_id;
        self.next_id += 1;
        self.write(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}))?;
        let raw = self
            .lines
            .recv_timeout(RESPONSE_TIMEOUT)
            .map_err(|error| match error {
                mpsc::RecvTimeoutError::Timeout => anyhow::anyhow!("MCP response timed out"),
                mpsc::RecvTimeoutError::Disconnected => anyhow::anyhow!("MCP server closed stdout"),
            })??;
        let response: Value =
            serde_json::from_slice(&raw).context("MCP response is not valid JSON")?;
        let object = response
            .as_object()
            .context("MCP response is not an object")?;
        if object.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
            bail!("MCP response has an invalid protocol version");
        }
        if object.get("id").and_then(Value::as_u64) != Some(id) {
            bail!("MCP response id does not match request");
        }
        if object.contains_key("result") == object.contains_key("error") {
            bail!("MCP response must contain exactly one result or error");
        }
        Ok(response)
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
