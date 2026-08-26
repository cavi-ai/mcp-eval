//! Scaffold a starter manifest from a live server's `tools/list` catalog.
//!
//! Everything is measured in memory through the same client boundary the
//! probes use; the generated manifest contains only structural fields
//! (bounds derived from measured sizes) and never persists payloads,
//! descriptions, or schemas.

use std::path::{Path, PathBuf};

use anyhow::{bail, Context};
use serde_json::Value;

use crate::http_client::HttpMcpClient;
use crate::mcp_client::{McpClient, ToolCatalog};
use crate::privacy;
use crate::probe::estimate_tokens;

const MAX_SCHEMA_CASES: usize = 20;

pub struct InitOptions {
    pub server: String,
    pub output: PathBuf,
    pub force: bool,
    pub confirm_read_only: bool,
    pub command: Vec<String>,
    pub http_url: Option<String>,
    pub allow_remote_http: bool,
}

pub struct InitSummary {
    pub path: PathBuf,
    pub tool_count: u64,
    pub schema_cases: usize,
}

enum InitClient {
    Stdio(McpClient),
    Http(HttpMcpClient),
}

impl InitClient {
    fn connect(options: &InitOptions) -> anyhow::Result<Self> {
        match (&options.http_url, options.command.is_empty()) {
            (None, false) => Ok(Self::Stdio(McpClient::spawn(&options.command)?)),
            (Some(endpoint), true) => Ok(Self::Http(HttpMcpClient::connect(
                endpoint,
                options.allow_remote_http,
            )?)),
            (Some(_), false) => bail!("select an HTTP endpoint or a stdio command, not both"),
            (None, true) => bail!("an HTTP endpoint or stdio command is required"),
        }
    }

    fn catalog(&mut self) -> anyhow::Result<ToolCatalog> {
        match self {
            Self::Stdio(client) => client.list_tools_catalog(),
            Self::Http(client) => client.list_tools_catalog(),
        }
    }

    fn initialize(&mut self) -> anyhow::Result<()> {
        match self {
            Self::Stdio(client) => client.initialize(),
            Self::Http(client) => client.initialize(),
        }
    }

    /// Read-only smoke call used to verify a candidate case can actually
    /// succeed before it is written into the manifest.
    fn naive_call(&mut self, tool: &str) -> anyhow::Result<bool> {
        let response = match self {
            Self::Stdio(client) => client.call_tool(tool, &serde_json::json!({})),
            Self::Http(client) => client.call_tool(tool, &serde_json::json!({})),
        }?;
        Ok(matches!(
            response,
            crate::mcp_client::ToolResponse::Success(_)
        ))
    }
}

/// True when a naive `{}` call can satisfy the declared required fields,
/// which is exactly what the `schema-guessability` probe demands.
fn zero_required(schema: &Value) -> bool {
    schema
        .get("required")
        .and_then(Value::as_array)
        .is_none_or(Vec::is_empty)
}

fn round_up_to(value: u64, step: u64) -> u64 {
    value.div_ceil(step) * step
}

fn scaffold(
    client: &mut InitClient,
    catalog: &ToolCatalog,
    confirm_read_only: bool,
) -> anyhow::Result<crate::manifest::Manifest> {
    use crate::manifest::{Access, Manifest, ProbeCase};
    let tool_count = catalog.tools.len() as u64;
    let encoded_bytes = catalog.encoded_bytes as u64;

    let total_budget =
        round_up_to(estimate_tokens(encoded_bytes as usize) * 2, 100).clamp(100, 1_000_000);
    let heaviest = catalog
        .tools
        .iter()
        .map(|tool| estimate_tokens(tool.entry_bytes))
        .max()
        .unwrap_or(1);
    let per_tool_budget = round_up_to(heaviest * 2, 100)
        .clamp(100, 100_000)
        .min(total_budget);

    let mut probes = vec![
        ProbeCase::DiscoveryCost {
            id: "discovery-budget".into(),
            access: Access::ReadOnly,
            max_tools: (tool_count * 2).clamp(10, 10_000),
            max_schema_bytes: (encoded_bytes * 2).clamp(1000, 10_000_000),
        },
        ProbeCase::TokenCost {
            id: "token-budget".into(),
            access: Access::ReadOnly,
            max_total_tokens: total_budget,
            max_tool_tokens: Some(per_tool_budget),
        },
    ];

    let mut schema_cases = 0usize;
    if confirm_read_only {
        for tool in &catalog.tools {
            if schema_cases >= MAX_SCHEMA_CASES {
                break;
            }
            if !zero_required(&tool.input_schema) {
                continue;
            }
            // The operator attested that every candidate is read-only. Only
            // declare calls that were just observed to accept naive `{}`.
            match client.naive_call(&tool.name) {
                Ok(true) => {}
                Ok(false) | Err(_) => continue,
            }
            let base = format!("{}-guessable", tool.name);
            let id = if privacy::valid_identifier(&base) {
                base
            } else {
                format!("case-schema-{}", schema_cases + 1)
            };
            probes.push(ProbeCase::SchemaGuessability {
                id,
                tool: tool.name.clone(),
                access: Access::ReadOnly,
                sandbox: None,
                arguments: Value::Object(serde_json::Map::new()),
            });
            schema_cases += 1;
        }
    }

    let manifest = Manifest {
        version: 1,
        sandboxes: Default::default(),
        probes,
    };
    manifest.validate()?;
    Ok(manifest)
}

pub fn run(options: InitOptions) -> anyhow::Result<InitSummary> {
    if !privacy::valid_server(&options.server) {
        bail!("server label is invalid");
    }
    write_guarded(&options.output, options.force)?;
    let mut client = InitClient::connect(&options)?;
    client.initialize().context("initializing MCP server")?;
    let catalog = client.catalog().context("listing tools")?;
    if catalog.tools.is_empty() {
        bail!("the server declared no tools; there is nothing to scaffold");
    }
    let manifest = scaffold(&mut client, &catalog, options.confirm_read_only)?;
    let body = serde_json::to_string_pretty(&manifest).context("serializing manifest")?;
    std::fs::write(&options.output, body + "\n").context("writing manifest")?;
    Ok(InitSummary {
        path: options.output,
        tool_count: catalog.tools.len() as u64,
        schema_cases: manifest
            .probes
            .iter()
            .filter(|case| matches!(case, crate::manifest::ProbeCase::SchemaGuessability { .. }))
            .count(),
    })
}

fn write_guarded(path: &Path, force: bool) -> anyhow::Result<()> {
    if path.exists() && !force {
        bail!(
            "{} already exists; pass --force to replace it",
            path.display()
        );
    }
    Ok(())
}
