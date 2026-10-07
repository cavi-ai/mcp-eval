//! Transport dispatch shared by gate and standard measurements.
use super::ProbeOptions;
use crate::http_client::HttpMcpClient;
use crate::mcp_client::{McpClient, ToolCatalog, ToolResponse};
use anyhow::bail;
use serde_json::Value;
use std::time::Duration;

#[derive(Clone, Debug)]
pub(crate) struct ClientTarget {
    transport: TargetTransport,
    pub(crate) budget: crate::evaluation_budget::Budget,
}

#[derive(Clone, Debug)]
enum TargetTransport {
    Stdio(Vec<String>),
    Http {
        endpoint: String,
        allow_remote: bool,
    },
}

pub(crate) enum ProbeClient {
    Stdio(McpClient),
    Http(HttpMcpClient),
}

impl ClientTarget {
    pub(crate) fn new(
        command: Vec<String>,
        http_url: Option<String>,
        allow_remote_http: bool,
    ) -> anyhow::Result<Self> {
        let transport = match (http_url, command.is_empty()) {
            (None, false) => TargetTransport::Stdio(command),
            (Some(endpoint), true) => TargetTransport::Http {
                endpoint,
                allow_remote: allow_remote_http,
            },
            (Some(_), false) => bail!("select an HTTP endpoint or a stdio command, not both"),
            (None, true) => bail!("an HTTP endpoint or stdio command is required"),
        };
        Ok(Self {
            transport,
            budget: crate::evaluation_budget::Budget::default(),
        })
    }

    pub(super) fn from_options(options: &ProbeOptions) -> anyhow::Result<Self> {
        Self::new(
            options.command.clone(),
            options.http_url.clone(),
            options.allow_remote_http,
        )
    }

    /// Open a client whose requests wait `timeout` (`None`: the
    /// transport's default).
    pub(crate) fn connect(&self, timeout: Option<Duration>) -> anyhow::Result<ProbeClient> {
        self.budget.check()?;
        let mut client = match &self.transport {
            TargetTransport::Stdio(command) => ProbeClient::Stdio(McpClient::spawn(command)?),
            TargetTransport::Http {
                endpoint,
                allow_remote,
            } => ProbeClient::Http(HttpMcpClient::connect(endpoint, *allow_remote)?),
        };
        client.set_response_timeout(timeout);
        match &mut client {
            ProbeClient::Stdio(client) => client.set_evaluation_budget(self.budget.clone()),
            ProbeClient::Http(client) => client.set_evaluation_budget(self.budget.clone()),
        }
        Ok(client)
    }
}

impl ProbeClient {
    pub(crate) fn set_response_timeout(&mut self, timeout: Option<Duration>) {
        match self {
            Self::Stdio(client) => client.set_response_timeout(timeout),
            Self::Http(client) => client.set_response_timeout(timeout),
        }
    }

    /// The response timeout the transport applies when none is set.
    pub(super) fn default_timeout(&self) -> Duration {
        match self {
            Self::Stdio(_) => crate::mcp_client::DEFAULT_RESPONSE_TIMEOUT,
            Self::Http(_) => crate::http_client::DEFAULT_IO_TIMEOUT,
        }
    }

    pub(crate) fn initialize(&mut self) -> anyhow::Result<()> {
        match self {
            Self::Stdio(client) => client.initialize(),
            Self::Http(client) => client.initialize(),
        }
    }

    pub(crate) fn initialize_with_capabilities(
        &mut self,
        capabilities: &Value,
    ) -> anyhow::Result<Value> {
        match self {
            Self::Stdio(client) => client.initialize_with_capabilities(capabilities),
            Self::Http(client) => client.initialize_with_capabilities(capabilities),
        }
    }

    pub(super) fn list_tools_catalog(&mut self) -> anyhow::Result<ToolCatalog> {
        match self {
            Self::Stdio(client) => client.list_tools_catalog(),
            Self::Http(client) => client.list_tools_catalog(),
        }
    }

    pub(super) fn discover_tools_catalog(
        &mut self,
    ) -> anyhow::Result<crate::mcp_client::CatalogDiscovery> {
        match self {
            Self::Stdio(client) => client.discover_tools_catalog(),
            Self::Http(client) => client.discover_tools_catalog(),
        }
    }

    pub(super) fn call_tool(
        &mut self,
        tool: &str,
        arguments: &serde_json::Value,
    ) -> anyhow::Result<ToolResponse> {
        match self {
            Self::Stdio(client) => client.call_tool(tool, arguments),
            Self::Http(client) => client.call_tool(tool, arguments),
        }
    }

    pub(crate) fn raw_request(
        &mut self,
        method: &str,
        params: serde_json::Value,
    ) -> anyhow::Result<serde_json::Value> {
        match self {
            Self::Stdio(client) => client.raw_request(method, params),
            Self::Http(client) => client.raw_request(method, params),
        }
    }

    pub(crate) fn capabilities(&self) -> Option<serde_json::Value> {
        match self {
            Self::Stdio(client) => client.capabilities(),
            Self::Http(client) => client.capabilities(),
        }
    }

    pub(super) fn cancel_tool_call(
        &mut self,
        tool: &str,
        arguments: &serde_json::Value,
        reason: &str,
        grace: std::time::Duration,
    ) -> anyhow::Result<crate::mcp_client::CancellationOutcome> {
        match self {
            Self::Stdio(client) => client.cancel_tool_call(tool, arguments, reason, grace),
            Self::Http(client) => client.cancel_tool_call(tool, arguments, reason, grace),
        }
    }

    pub(crate) fn initialize_raw(
        &mut self,
        protocol_version: &str,
    ) -> anyhow::Result<serde_json::Value> {
        match self {
            Self::Stdio(client) => client.initialize_raw(protocol_version),
            Self::Http(client) => {
                // The HTTP transport routes each handshake through a
                // fresh POST; the server's session state is untouched
                // because the negotiated reply is returned verbatim.
                client.initialize_raw(protocol_version)
            }
        }
    }

    pub(crate) fn call_tool_observing(
        &mut self,
        tool: &str,
        arguments: &serde_json::Value,
        respond: &mut dyn FnMut(&str, &serde_json::Value) -> Option<serde_json::Value>,
        max_server_requests: u64,
    ) -> anyhow::Result<(ToolResponse, u64)> {
        match self {
            Self::Stdio(client) => {
                client.call_tool_observing(tool, arguments, respond, max_server_requests)
            }
            Self::Http(client) => {
                client.call_tool_observing(tool, arguments, respond, max_server_requests)
            }
        }
    }

    pub(super) fn read_resource(&mut self, uri: &str) -> anyhow::Result<serde_json::Value> {
        match self {
            Self::Stdio(client) => client.read_resource(uri),
            Self::Http(client) => client.read_resource(uri),
        }
    }

    pub(super) fn complete(
        &mut self,
        ref_type: &str,
        ref_uri: &str,
        argument_name: &str,
        argument_value: &str,
    ) -> anyhow::Result<serde_json::Value> {
        match self {
            Self::Stdio(client) => {
                client.complete(ref_type, ref_uri, argument_name, argument_value)
            }
            Self::Http(client) => client.complete(ref_type, ref_uri, argument_name, argument_value),
        }
    }

    pub(super) fn wait_for_resource_update(
        &mut self,
        uri: &str,
        wait: std::time::Duration,
    ) -> bool {
        match self {
            Self::Stdio(client) => client.wait_for_resource_update(uri, wait),
            Self::Http(client) => client.wait_for_resource_update(uri, wait),
        }
    }

    pub(super) fn unsubscribe(&mut self, uri: &str) -> anyhow::Result<serde_json::Value> {
        match self {
            Self::Stdio(client) => client.unsubscribe(uri),
            Self::Http(client) => client.unsubscribe(uri),
        }
    }
}
