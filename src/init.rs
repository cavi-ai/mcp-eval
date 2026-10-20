//! Scaffold a starter manifest from a live server's `initialize`
//! capabilities and `tools/list` catalog.
//!
//! Everything is measured in memory through the same client boundary the
//! probes use; the generated manifest contains only structural fields
//! (bounds derived from measured sizes and latencies, identifier-shaped tool
//! and property names) and never persists payloads, descriptions, or schemas.

use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{bail, Context};
use serde_json::Value;

use crate::http_client::HttpMcpClient;
use crate::manifest::ProbeKind;
use crate::mcp_client::{McpClient, ToolCatalog, ToolDefinition};
use crate::privacy;
use crate::probe::estimate_tokens;
use crate::tool_arguments::ToolArguments;

const MAX_VERIFIED_TOOLS: usize = 20;
const PAYLOAD_SIZE_BYTES: u64 = 1_000_000;

pub struct InitOptions {
    pub server: String,
    pub output: PathBuf,
    pub force: bool,
    pub confirm_read_only: bool,
    /// Restrict candidates to these tools; empty selects every tool.
    pub tools: Vec<String>,
    /// Arguments to call named tools with (`--arguments`).
    pub arguments: Option<ToolArguments>,
    pub command: Vec<String>,
    pub http_url: Option<String>,
    pub allow_remote_http: bool,
}

pub struct InitSummary {
    pub path: PathBuf,
    pub tool_count: u64,
    pub case_count: usize,
    /// Scaffolded cases per probe kind, in first-appearance order.
    pub kind_counts: Vec<(ProbeKind, usize)>,
    /// Tools never called because the server annotates them as writers.
    pub skipped_by_annotations: usize,
    /// Every catalog tool without cases, and why, in catalog order; tools
    /// left out by `--tool` are not listed.
    pub skipped: Vec<(String, String)>,
}

/// Whether `init` calls and scaffolds a catalog tool, and why.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    ReadOnlyHint,
    Attested,
    NeedsAttestation,
    DestructiveHint,
    NotReadOnly,
    RequiredArguments,
    NotSelected,
}

impl Decision {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ReadOnlyHint => "candidate (readOnlyHint)",
            Self::Attested => "candidate (attested)",
            Self::NeedsAttestation => "needs --confirm-read-only",
            Self::DestructiveHint => "skipped: destructiveHint",
            Self::NotReadOnly => "skipped: readOnlyHint=false",
            Self::RequiredArguments => "skipped: required arguments",
            Self::NotSelected => "skipped: not in --tool",
        }
    }

    /// Why a tool that is not a candidate gets no cases.
    fn reason(self) -> &'static str {
        match self {
            Self::ReadOnlyHint | Self::Attested => "a candidate",
            Self::NeedsAttestation => "no readOnlyHint; pass --confirm-read-only to attest it",
            Self::DestructiveHint => "destructiveHint",
            Self::NotReadOnly => "readOnlyHint=false",
            Self::RequiredArguments => {
                "required arguments its schema cannot build; pass them with --arguments"
            }
            Self::NotSelected => "not in --tool",
        }
    }

    fn is_candidate(self) -> bool {
        matches!(self, Self::ReadOnlyHint | Self::Attested)
    }

    fn by_annotations(self) -> bool {
        matches!(self, Self::DestructiveHint | Self::NotReadOnly)
    }
}

enum InitClient {
    Stdio(McpClient),
    Http(HttpMcpClient),
}

impl InitClient {
    fn connect(
        options: &InitOptions,
        budget: &crate::evaluation_budget::Budget,
    ) -> anyhow::Result<Self> {
        budget.check()?;
        let mut client = match (&options.http_url, options.command.is_empty()) {
            (None, false) => Self::Stdio(McpClient::spawn(&options.command)?),
            (Some(endpoint), true) => {
                Self::Http(HttpMcpClient::connect(endpoint, options.allow_remote_http)?)
            }
            (Some(_), false) => bail!("select an HTTP endpoint or a stdio command, not both"),
            (None, true) => bail!("an HTTP endpoint or stdio command is required"),
        };
        match &mut client {
            Self::Stdio(client) => client.set_evaluation_budget(budget.clone()),
            Self::Http(client) => client.set_evaluation_budget(budget.clone()),
        }
        Ok(client)
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

    fn capabilities(&self) -> Option<Value> {
        match self {
            Self::Stdio(client) => client.capabilities(),
            Self::Http(client) => client.capabilities(),
        }
    }

    /// Read-only smoke call used to verify a candidate case can actually
    /// succeed before it is written into the manifest. Returns the call's
    /// latency in milliseconds when it succeeded.
    fn smoke_call(&mut self, tool: &str, arguments: &Value) -> anyhow::Result<Option<u64>> {
        let started = Instant::now();
        let response = match self {
            Self::Stdio(client) => client.call_tool(tool, arguments),
            Self::Http(client) => client.call_tool(tool, arguments),
        }?;
        let latency_ms = started.elapsed().as_millis() as u64;
        Ok(matches!(response, crate::mcp_client::ToolResponse::Success(_)).then_some(latency_ms))
    }
}

/// `<tool>-<suffix>` when that is a valid case id, else `case-<kind>-<n>`.
fn case_id(tool: &str, suffix: &str, kind: ProbeKind, ordinal: usize) -> String {
    let base = format!("{tool}-{suffix}");
    if privacy::valid_identifier(&base) {
        base
    } else {
        format!("case-{}-{ordinal}", kind.as_str())
    }
}

/// The first declared string property that can name a payload field.
fn string_field(tool: &ToolDefinition) -> Option<&str> {
    tool.input_schema
        .get("properties")
        .and_then(Value::as_object)?
        .iter()
        .find(|(name, property)| {
            property.get("type").and_then(Value::as_str) == Some("string")
                && privacy::valid_identifier(name)
        })
        .map(|(name, _)| name.as_str())
}

/// True when a naive `{}` call can satisfy the declared required fields,
/// which is exactly what the `schema-guessability` probe demands.
pub(crate) fn zero_required(schema: &Value) -> bool {
    schema
        .get("required")
        .and_then(Value::as_array)
        .is_none_or(Vec::is_empty)
}

fn round_up_to(value: u64, step: u64) -> u64 {
    value.div_ceil(step) * step
}

/// Where the arguments a tool is called with came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Source {
    /// `{}` for a tool with no required parameters.
    Empty,
    Supplied,
    Synthesized,
}

/// The arguments `init` calls a tool with: `--arguments` first, `{}` for a
/// tool with no required parameters, else values built from its schema.
fn call_arguments(
    tool: &ToolDefinition,
    supplied: Option<&ToolArguments>,
) -> Option<(Value, Source)> {
    if let Some(arguments) = supplied.and_then(|supplied| supplied.get(&tool.name)) {
        return Some((arguments.clone(), Source::Supplied));
    }
    if zero_required(&tool.input_schema) {
        return Some((Value::Object(serde_json::Map::new()), Source::Empty));
    }
    crate::standard::synthesize(&tool.input_schema)
        .map(|arguments| (arguments, Source::Synthesized))
}

/// Annotations outrank the attestation: a tool the server marks
/// destructive or not read-only is never called.
fn decide(
    tool: &ToolDefinition,
    confirm_read_only: bool,
    selected: &[String],
    supplied: Option<&ToolArguments>,
) -> Decision {
    if !selected.is_empty() && !selected.contains(&tool.name) {
        Decision::NotSelected
    } else if tool.destructive_hint == Some(true) {
        Decision::DestructiveHint
    } else if tool.read_only_hint == Some(false) {
        Decision::NotReadOnly
    } else if call_arguments(tool, supplied).is_none() {
        Decision::RequiredArguments
    } else if tool.read_only_hint == Some(true) {
        Decision::ReadOnlyHint
    } else if confirm_read_only {
        Decision::Attested
    } else {
        Decision::NeedsAttestation
    }
}

/// Every catalog tool's decision, in catalog order. A selected name the
/// catalog lacks, or one init cannot call (annotated as a writer, required
/// arguments, or unattested), is a usage error.
fn decisions(
    catalog: &ToolCatalog,
    confirm_read_only: bool,
    selected: &[String],
    supplied: Option<&ToolArguments>,
) -> anyhow::Result<Vec<Decision>> {
    if let Some(supplied) = supplied {
        supplied
            .validate(catalog, confirm_read_only)
            .map_err(crate::exit::usage)?;
    }
    for name in selected {
        let reason = match catalog.tools.iter().find(|tool| &tool.name == name) {
            None => "not in the server's tool catalog",
            Some(tool) => match decide(tool, confirm_read_only, selected, supplied) {
                Decision::DestructiveHint => {
                    "the server annotates it destructiveHint, so init never calls it"
                }
                Decision::NotReadOnly => {
                    "the server annotates it readOnlyHint=false, so init never calls it"
                }
                Decision::RequiredArguments => {
                    "it declares required arguments its schema cannot build; pass them with \
                     --arguments"
                }
                Decision::NeedsAttestation => {
                    "it carries no readOnlyHint annotation; pass --confirm-read-only to attest \
                     it is read-only"
                }
                _ => continue,
            },
        };
        return Err(crate::exit::usage(anyhow::anyhow!(
            "--tool {name}: {reason}"
        )));
    }
    Ok(catalog
        .tools
        .iter()
        .map(|tool| decide(tool, confirm_read_only, selected, supplied))
        .collect())
}

/// The manifest, and each candidate left without cases with the reason.
fn scaffold(
    client: &mut InitClient,
    catalog: &ToolCatalog,
    decisions: &[Decision],
    supplied: Option<&ToolArguments>,
    budget: &crate::evaluation_budget::Budget,
) -> anyhow::Result<(crate::manifest::Manifest, Vec<(String, String)>)> {
    budget.check()?;
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
        ProbeCase::Pagination {
            id: "catalog-pagination".into(),
            access: Access::ReadOnly,
            max_pages: 5,
        },
        ProbeCase::ProtocolNegotiation {
            id: "protocol-negotiation".into(),
            access: Access::ReadOnly,
            bogus_version: "2000-01-01".into(),
        },
    ];
    let declares_surfaces = client.capabilities().is_some_and(|capabilities| {
        capabilities.get("resources").is_some() || capabilities.get("prompts").is_some()
    });
    if declares_surfaces {
        probes.push(ProbeCase::SurfaceListing {
            id: "declared-surfaces".into(),
            access: Access::ReadOnly,
            max_pages: 5,
        });
    }

    let mut verified: Vec<(&ToolDefinition, Value)> = Vec::new();
    let mut skipped = Vec::new();
    for (tool, decision) in catalog.tools.iter().zip(decisions) {
        budget.check()?;
        if !decision.is_candidate() {
            continue;
        }
        if verified.len() >= MAX_VERIFIED_TOOLS {
            skipped.push((
                tool.name.clone(),
                format!(
                    "init scaffolds at most {MAX_VERIFIED_TOOLS} tools; select others with --tool"
                ),
            ));
            continue;
        }
        let (arguments, source) =
            call_arguments(tool, supplied).expect("a candidate has call arguments");
        // The server annotated the candidate read-only or the operator
        // attested it. Only declare calls that were just observed to
        // succeed with these arguments.
        let Ok(Some(latency_ms)) = client.smoke_call(&tool.name, &arguments) else {
            let reason = match source {
                Source::Empty => "the call with {} failed",
                Source::Supplied => "the call with its --arguments failed",
                Source::Synthesized => {
                    "the call with arguments built from its schema failed; pass working ones with \
                     --arguments"
                }
            };
            skipped.push((tool.name.clone(), reason.to_owned()));
            continue;
        };
        verified.push((tool, arguments.clone()));
        let ordinal = verified.len();
        probes.push(ProbeCase::SchemaGuessability {
            id: case_id(
                &tool.name,
                "guessable",
                ProbeKind::SchemaGuessability,
                ordinal,
            ),
            tool: tool.name.clone(),
            access: Access::ReadOnly,
            sandbox: None,
            arguments: arguments.clone(),
        });
        probes.push(ProbeCase::DegradationOverN {
            id: case_id(&tool.name, "repeat", ProbeKind::DegradationOverN, ordinal),
            tool: tool.name.clone(),
            access: Access::ReadOnly,
            sandbox: None,
            arguments: arguments.clone(),
            max_attempts: 5,
        });
        probes.push(ProbeCase::LatencyBudget {
            id: case_id(&tool.name, "latency", ProbeKind::LatencyBudget, ordinal),
            tool: tool.name.clone(),
            access: Access::ReadOnly,
            sandbox: None,
            arguments: arguments.clone(),
            attempts: 3,
            max_latency_ms: round_up_to(latency_ms * 4, 100).clamp(1000, 60_000),
        });
        if tool.declared_output_schema().is_some() {
            probes.push(ProbeCase::OutputSchema {
                id: case_id(&tool.name, "output", ProbeKind::OutputSchema, ordinal),
                tool: tool.name.clone(),
                access: Access::ReadOnly,
                sandbox: None,
                arguments,
            });
        }
    }
    if let Some((representative, representative_arguments)) = verified.first() {
        probes.push(ProbeCase::Contention {
            id: case_id(&representative.name, "contention", ProbeKind::Contention, 1),
            tool: representative.name.clone(),
            access: Access::ReadOnly,
            sandbox: None,
            arguments: representative_arguments.clone(),
        });
        let (payload_tool, payload_arguments, field) = verified
            .iter()
            .find_map(|(tool, arguments)| string_field(tool).map(|field| (*tool, arguments, field)))
            .unwrap_or((representative, representative_arguments, "payload"));
        probes.push(ProbeCase::PayloadBounds {
            id: case_id(&payload_tool.name, "payload", ProbeKind::PayloadBounds, 1),
            tool: payload_tool.name.clone(),
            access: Access::ReadOnly,
            sandbox: None,
            arguments: payload_arguments.clone(),
            field: field.to_owned(),
            size_bytes: PAYLOAD_SIZE_BYTES,
            expect_handled: false,
        });
    }

    let manifest = Manifest {
        version: 1,
        timeout_ms: None,
        sandboxes: Default::default(),
        probes,
    };
    manifest.validate()?;
    budget.check()?;
    Ok((manifest, skipped))
}

/// Connect, handshake, and list the catalog every scaffolding path starts
/// from.
fn open(
    options: &InitOptions,
) -> anyhow::Result<(InitClient, ToolCatalog, crate::evaluation_budget::Budget)> {
    if !privacy::valid_server(&options.server) {
        bail!("server label is invalid");
    }
    let budget = crate::evaluation_budget::Budget::default();
    let mut client = InitClient::connect(options, &budget)?;
    client.initialize().context("initializing MCP server")?;
    let catalog = client.catalog().context("listing tools")?;
    if catalog.tools.is_empty() {
        bail!("the server declared no tools; there is nothing to scaffold");
    }
    budget.check()?;
    Ok((client, catalog, budget))
}

/// Each catalog tool with the decision `run` would apply to it. Calls no
/// tool and touches no file.
pub fn dry_run(options: &InitOptions) -> anyhow::Result<Vec<(String, Decision)>> {
    let (_, catalog, budget) = open(options)?;
    let decisions = decisions(
        &catalog,
        options.confirm_read_only,
        &options.tools,
        options.arguments.as_ref(),
    )?;
    budget.check()?;
    Ok(catalog
        .tools
        .into_iter()
        .map(|tool| tool.name)
        .zip(decisions)
        .collect())
}

pub fn run(options: InitOptions) -> anyhow::Result<InitSummary> {
    write_guarded(&options.output, options.force)?;
    let (mut client, catalog, budget) = open(&options)?;
    let supplied = options.arguments.as_ref();
    let decisions = decisions(
        &catalog,
        options.confirm_read_only,
        &options.tools,
        supplied,
    )?;
    let tool_count = catalog.tools.len() as u64;
    let (manifest, failed) = scaffold(&mut client, &catalog, &decisions, supplied, &budget)?;
    let body = serde_json::to_string_pretty(&manifest).context("serializing manifest")?;
    std::fs::write(&options.output, body + "\n").context("writing manifest")?;
    let mut kind_counts: Vec<(ProbeKind, usize)> = Vec::new();
    for case in &manifest.probes {
        match kind_counts
            .iter_mut()
            .find(|(kind, _)| *kind == case.kind())
        {
            Some((_, count)) => *count += 1,
            None => kind_counts.push((case.kind(), 1)),
        }
    }
    Ok(InitSummary {
        path: options.output,
        tool_count,
        case_count: manifest.probes.len(),
        kind_counts,
        skipped_by_annotations: decisions
            .iter()
            .filter(|decision| decision.by_annotations())
            .count(),
        skipped: catalog
            .tools
            .iter()
            .zip(&decisions)
            .filter_map(|(tool, decision)| {
                if *decision == Decision::NotSelected {
                    None
                } else if decision.is_candidate() {
                    failed.iter().find(|(name, _)| *name == tool.name).cloned()
                } else {
                    Some((tool.name.clone(), decision.reason().to_owned()))
                }
            })
            .collect(),
    })
}

/// In-memory scaffolding input shared by the CLI (`init`) and the agent
/// surface (`serve`'s scaffold tool).
pub struct ScaffoldRequest {
    pub server: String,
    pub confirm_read_only: bool,
    pub command: Vec<String>,
    pub http_url: Option<String>,
    pub allow_remote_http: bool,
}

/// Probe a live server and derive its starter manifest without touching
/// the filesystem.
pub fn probe_scaffold(request: ScaffoldRequest) -> anyhow::Result<crate::manifest::Manifest> {
    let (mut client, catalog, budget) = open(&InitOptions {
        server: request.server,
        output: std::path::PathBuf::new(),
        force: false,
        confirm_read_only: request.confirm_read_only,
        tools: Vec::new(),
        arguments: None,
        command: request.command,
        http_url: request.http_url,
        allow_remote_http: request.allow_remote_http,
    })?;
    let decisions = decisions(&catalog, request.confirm_read_only, &[], None)?;
    Ok(scaffold(&mut client, &catalog, &decisions, None, &budget)?.0)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn evaluation_budget_exhaustion_does_not_return_a_partial_scaffold() {
        let options = InitOptions {
            server: "fixture".into(),
            output: PathBuf::new(),
            force: false,
            confirm_read_only: true,
            tools: vec![],
            arguments: None,
            command: vec![
                "python3".into(),
                concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/tests/fixtures/transport_fault_server.py"
                )
                .into(),
            ],
            http_url: None,
            allow_remote_http: false,
        };
        // This tests request exhaustion after discovery, not interpreter
        // startup speed. Deadline behavior has separate short-call coverage.
        let budget =
            crate::evaluation_budget::Budget::new(crate::evaluation_budget::MAX_DURATION, 3);
        let mut client = InitClient::connect(&options, &budget).unwrap();
        client.initialize().unwrap();
        let catalog = client.catalog().unwrap();
        let decisions = decisions(&catalog, true, &[], None).unwrap();
        let error = scaffold(&mut client, &catalog, &decisions, None, &budget).unwrap_err();
        assert!(error
            .downcast_ref::<crate::evaluation_budget::Exhausted>()
            .is_some());
    }

    fn tool(input_schema: Value) -> ToolDefinition {
        ToolDefinition {
            name: "lookup".into(),
            input_schema,
            entry_bytes: 0,
            description_chars: 0,
            output_schema: None,
            read_only_hint: None,
            destructive_hint: None,
        }
    }

    #[test]
    fn case_ids_fall_back_to_kind_and_ordinal() {
        assert_eq!(
            case_id("read", "repeat", ProbeKind::DegradationOverN, 2),
            "read-repeat"
        );
        assert_eq!(
            case_id("9lives", "repeat", ProbeKind::DegradationOverN, 2),
            "case-degradation-over-n-2"
        );
    }

    #[test]
    fn payload_field_is_the_first_identifier_shaped_string_property() {
        let schema = serde_json::json!({"properties": {
            "1bad": {"type": "string"},
            "count": {"type": "integer"},
            "query": {"type": "string"},
            "zone": {"type": "string"}
        }});
        assert_eq!(string_field(&tool(schema)), Some("query"));
        assert_eq!(
            string_field(&tool(
                serde_json::json!({"properties": {"n": {"type": "integer"}}})
            )),
            None
        );
        assert_eq!(string_field(&tool(serde_json::json!({}))), None);
    }
}
