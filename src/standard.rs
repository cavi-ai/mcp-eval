//! The mcpeval standard battery: read-only observations of a server's
//! whole catalog, folded into readiness by [`crate::score`]. It runs on
//! its own connection, never calls a tool declared a writer, and journals
//! nothing: its deliberate calls must not become promoted findings.

use std::time::{Duration, Instant};

use anyhow::Context;
use serde_json::{json, Value};

use crate::manifest::{Access, ProbeCase};
use crate::mcp_client::{ToolCatalog, ToolDefinition, ToolResponse};
use crate::probe::{estimate_tokens, ClientTarget, FailureReason, ProbeClient};
use crate::score::{CheckId, CheckReason, Readiness};

/// Per-call response timeout, pinned by the standard: it bounds what a
/// hung tool costs and is part of what a score means.
pub const CALL_TIMEOUT: Duration = Duration::from_secs(10);
/// Calls per exercised tool.
pub const REPEATS: usize = 3;
/// `tools/list` pages followed before the catalog is taken as complete.
pub const MAX_PAGES: usize = 20;
/// Server-to-client requests declined during one tool call.
pub(crate) const MAX_SERVER_REQUESTS: u64 = 8;
const PAYLOAD_BYTES: u64 = 1_000_000;

#[derive(Clone, Debug, Default)]
pub struct StandardOptions {
    /// Attest that unannotated tools are read-only, so they are called.
    pub confirm_read_only: bool,
    /// Tools never to call; each must be one the server lists.
    pub skip_tools: Vec<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ToolClass {
    /// `readOnlyHint: false` or `destructiveHint: true`: never called.
    Writer,
    /// `readOnlyHint: true`.
    ReadOnly,
    /// No deciding annotation: called only under `--confirm-read-only`.
    Unannotated,
}

impl ToolClass {
    pub fn of(tool: &ToolDefinition) -> Self {
        if tool.read_only_hint == Some(false) || tool.destructive_hint == Some(true) {
            Self::Writer
        } else if tool.read_only_hint == Some(true) {
            Self::ReadOnly
        } else {
            Self::Unannotated
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Exercise {
    /// The tool was not called, or its calls did not count: why.
    NotExercised(CheckReason),
    /// Three calls completed and none was rejected.
    Exercised {
        consistent: bool,
        median_latency_ms: u64,
        /// Whether the first successful result honored the declared
        /// output schema; None without a declaration or a success.
        output_schema: Option<bool>,
    },
}

/// Presence facts about one catalog entry; no description or schema text.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct CatalogFacts {
    pub description_chars: usize,
    pub properties: usize,
    pub described_properties: usize,
    pub typed_properties: usize,
    pub read_only_declared: bool,
    pub destructive_declared: bool,
    pub output_schema: bool,
}

impl CatalogFacts {
    pub fn of(tool: &ToolDefinition) -> Self {
        const TYPED: [&str; 6] = ["type", "enum", "const", "$ref", "anyOf", "oneOf"];
        let properties = tool
            .input_schema
            .get("properties")
            .and_then(Value::as_object);
        let count = |keep: &dyn Fn(&Value) -> bool| {
            properties.map_or(0, |properties| {
                properties.values().filter(|value| keep(value)).count()
            })
        };
        Self {
            description_chars: tool.description_chars,
            properties: properties.map_or(0, serde_json::Map::len),
            described_properties: count(&|property| {
                property
                    .get("description")
                    .and_then(Value::as_str)
                    .is_some_and(|text| !text.is_empty())
            }),
            typed_properties: count(&|property| {
                TYPED.iter().any(|key| property.get(*key).is_some())
            }),
            read_only_declared: tool.read_only_hint.is_some(),
            destructive_declared: tool.destructive_hint.is_some(),
            output_schema: tool.output_schema.is_some(),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ToolObservation {
    pub name: String,
    /// `estimate_tokens` of the tool's `tools/list` entry.
    pub tokens: u64,
    pub class: ToolClass,
    /// None for writers, which are never called.
    pub exercise: Option<Exercise>,
    pub catalog: CatalogFacts,
    /// How the tool refused schema-violating arguments; None for tools
    /// that are not called or whose schema admits every input.
    pub honesty: Option<Honesty>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Honesty {
    Honest,
    Dishonest(CheckReason),
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Observations {
    /// Every distinct tool, in catalog order.
    pub tools: Vec<ToolObservation>,
    pub attested_read_only: bool,
    /// Contention on the first fully successful tool; None when none.
    pub contention: Option<bool>,
    /// Payload bounds on the first fully successful tool with a string
    /// property; None when none.
    pub payload: Option<bool>,
    /// Each applicable protocol check and, when it failed, why.
    pub protocol: Vec<(CheckId, Option<CheckReason>)>,
}

/// How one call ended.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Outcome {
    Success,
    /// A result with `isError: true`.
    ToolError,
    /// A JSON-RPC error other than invalid params.
    RpcError,
    /// JSON-RPC -32602: the arguments were refused.
    Rejected,
}

impl Outcome {
    pub fn of(response: &ToolResponse) -> Self {
        match response {
            ToolResponse::Success(_) => Self::Success,
            ToolResponse::Error { code: -32602, .. } => Self::Rejected,
            ToolResponse::Error { payload, .. }
                if payload.get("code").and_then(Value::as_str) == Some("tool-error") =>
            {
                Self::ToolError
            }
            ToolResponse::Error { .. } => Self::RpcError,
        }
    }
}

/// Run the standard battery against `target` and fold it into readiness.
pub(crate) fn run(target: &ClientTarget, options: &StandardOptions) -> anyhow::Result<Readiness> {
    Ok(crate::score::fold(&observe(target, options)?))
}

/// A fresh, initialized connection. The handshake keeps the transport's
/// default timeout (cold starts); the standard's per-call timeout applies
/// from the first request after it.
fn connect(target: &ClientTarget) -> anyhow::Result<ProbeClient> {
    let mut client = target.connect(None)?;
    client.initialize()?;
    client.set_response_timeout(Some(CALL_TIMEOUT));
    Ok(client)
}

fn observe(target: &ClientTarget, options: &StandardOptions) -> anyhow::Result<Observations> {
    let mut client = Some(connect(target).context("connecting for the standard battery")?);
    let catalog = list_catalog(client.as_mut().expect("just connected"))
        .context("listing tools for the standard battery")?;
    if let Some(name) = options
        .skip_tools
        .iter()
        .find(|name| !catalog.tools.iter().any(|tool| &tool.name == *name))
    {
        return Err(crate::exit::usage(anyhow::anyhow!(
            "--skip-tool {name} is not a tool the server lists"
        )));
    }
    let mut tools = Vec::with_capacity(catalog.tools.len());
    // Tools whose three calls all succeeded, with the arguments used: the
    // server-level cases run on the first of them.
    let mut succeeded: Vec<(&ToolDefinition, Value)> = Vec::new();
    for tool in &catalog.tools {
        let class = ToolClass::of(tool);
        let callable = match class {
            ToolClass::Writer => false,
            ToolClass::Unannotated => options.confirm_read_only,
            ToolClass::ReadOnly => true,
        };
        // A skipped tool is never called; it scores as the worst call it
        // could have made (see score.rs), so skipping never pays.
        let skipped = callable && options.skip_tools.contains(&tool.name);
        let honesty = if skipped {
            invalid_arguments(&tool.input_schema)
                .map(|_| Honesty::Dishonest(CheckReason::CoverageSkipped))
        } else if callable {
            probe_honesty(&mut client, target, tool)
        } else {
            None
        };
        let exercise = match class {
            ToolClass::Writer => None,
            ToolClass::Unannotated if !options.confirm_read_only => {
                Some(Exercise::NotExercised(CheckReason::CoverageUnannotated))
            }
            _ => match synthesize(&tool.input_schema) {
                Some(_) if skipped => Some(Exercise::NotExercised(CheckReason::CoverageSkipped)),
                Some(arguments) => {
                    let (exercise, all_succeeded) =
                        exercise_tool(&mut client, target, tool, &arguments);
                    if all_succeeded {
                        succeeded.push((tool, arguments));
                    }
                    Some(exercise)
                }
                None => Some(Exercise::NotExercised(CheckReason::CoverageUnsynthesizable)),
            },
        };
        tools.push(ToolObservation {
            name: tool.name.clone(),
            tokens: estimate_tokens(tool.entry_bytes),
            class,
            exercise,
            catalog: CatalogFacts::of(tool),
            honesty,
        });
    }
    // With a tool skipped, the server-level cases score 0 (score.rs) and
    // are not run.
    let any_skipped = tools
        .iter()
        .any(|tool| tool.exercise == Some(Exercise::NotExercised(CheckReason::CoverageSkipped)));
    if any_skipped {
        succeeded.clear();
    }
    let contention = succeeded.first().map(|(tool, arguments)| {
        server_case(
            &mut client,
            target,
            &catalog,
            &ProbeCase::Contention {
                id: "standard-contention".into(),
                tool: tool.name.clone(),
                access: Access::ReadOnly,
                sandbox: None,
                arguments: arguments.clone(),
            },
        )
    });
    let payload = succeeded
        .iter()
        .find_map(|(tool, arguments)| string_property(tool).map(|field| (tool, arguments, field)))
        .map(|(tool, arguments, field)| {
            server_case(
                &mut client,
                target,
                &catalog,
                &ProbeCase::PayloadBounds {
                    id: "standard-payload".into(),
                    tool: tool.name.clone(),
                    access: Access::ReadOnly,
                    sandbox: None,
                    arguments: arguments.clone(),
                    field,
                    size_bytes: PAYLOAD_BYTES,
                    expect_handled: false,
                },
            )
        });
    let protocol = protocol_checks(&mut client, target, &catalog);
    Ok(Observations {
        tools,
        attested_read_only: options.confirm_read_only,
        contention,
        payload,
        protocol,
    })
}

/// Every distinct tool over at most [`MAX_PAGES`] pages. A first page that
/// fails stops the battery; a later page that fails (an error, a malformed
/// envelope, or an invalid entry) ends the listing with what was listed.
fn list_catalog(client: &mut ProbeClient) -> anyhow::Result<ToolCatalog> {
    let mut tools: Vec<ToolDefinition> = Vec::new();
    let mut cursor: Option<String> = None;
    for page in 0..MAX_PAGES {
        let params = match &cursor {
            Some(cursor) => json!({"cursor": cursor}),
            None => json!({}),
        };
        let (entries, next) = match list_page(client, params) {
            Ok(listed) => listed,
            Err(error) if page == 0 => return Err(error),
            Err(_) => break,
        };
        for tool in entries {
            if !tools.iter().any(|seen| seen.name == tool.name) {
                tools.push(tool);
            }
        }
        cursor = next;
        if cursor.is_none() {
            break;
        }
    }
    let encoded_bytes = tools.iter().map(|tool| tool.entry_bytes).sum();
    Ok(ToolCatalog {
        tools,
        encoded_bytes,
    })
}

/// Nesting followed while synthesizing, `$ref` hops included.
const MAX_SCHEMA_DEPTH: usize = 8;

/// Valid arguments built from `schema` alone: every required property,
/// deterministically. None when a required property has no rule.
pub fn synthesize(schema: &Value) -> Option<Value> {
    object_value(schema, schema, 0)
}

fn object_value(schema: &Value, root: &Value, depth: usize) -> Option<Value> {
    let properties = schema.get("properties").and_then(Value::as_object);
    let mut arguments = serde_json::Map::new();
    for name in schema
        .get("required")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let name = name.as_str()?;
        let property = properties?.get(name)?;
        arguments.insert(
            name.to_owned(),
            synthesize_value(property, root, depth + 1)?,
        );
    }
    Some(Value::Object(arguments))
}

/// One value, first rule that applies: `$ref`; `const`, the first `enum`
/// member, `default`, or the first of `examples`; the first `anyOf` or
/// `oneOf` branch; then by type.
fn synthesize_value(schema: &Value, root: &Value, depth: usize) -> Option<Value> {
    if depth > MAX_SCHEMA_DEPTH {
        return None;
    }
    if let Some(reference) = schema.get("$ref").and_then(Value::as_str) {
        let target = root.pointer(reference.strip_prefix('#')?)?;
        return synthesize_value(target, root, depth + 1);
    }
    for key in ["const", "enum", "default", "examples"] {
        let Some(value) = schema.get(key) else {
            continue;
        };
        return match key {
            "enum" | "examples" => value.as_array()?.first().cloned(),
            _ => Some(value.clone()),
        };
    }
    if let Some(branch) = schema
        .get("anyOf")
        .or_else(|| schema.get("oneOf"))
        .and_then(Value::as_array)
        .and_then(|branches| branches.first())
    {
        return synthesize_value(branch, root, depth + 1);
    }
    let kind = match schema.get("type") {
        Some(Value::String(kind)) => kind.as_str(),
        Some(Value::Array(kinds)) => kinds
            .iter()
            .filter_map(Value::as_str)
            .find(|kind| *kind != "null")?,
        _ => return None,
    };
    match kind {
        "string" => string_value(schema),
        "integer" | "number" => Some(number_value(schema)),
        "boolean" => Some(json!(false)),
        "array" => {
            let count = schema.get("minItems").and_then(Value::as_u64).unwrap_or(0) as usize;
            if count == 0 {
                return Some(json!([]));
            }
            let item = synthesize_value(schema.get("items")?, root, depth + 1)?;
            Some(Value::Array(vec![item; count]))
        }
        "object" => object_value(schema, root, depth),
        "null" => Some(Value::Null),
        _ => None,
    }
}

fn string_value(schema: &Value) -> Option<Value> {
    let formatted = match schema.get("format").and_then(Value::as_str) {
        Some("date-time") => Some("2026-01-01T00:00:00Z"),
        Some("date") => Some("2026-01-01"),
        Some("uri") => Some("https://example.com"),
        Some("email") => Some("user@example.com"),
        Some("uuid") => Some("00000000-0000-4000-8000-000000000000"),
        _ => None,
    };
    if let Some(value) = formatted {
        return Some(json!(value));
    }
    if schema.get("pattern").is_some() {
        return None;
    }
    let min = schema.get("minLength").and_then(Value::as_u64).unwrap_or(0) as usize;
    let max = schema
        .get("maxLength")
        .and_then(Value::as_u64)
        .map(|max| max as usize);
    let mut value = String::from("mcpeval");
    while value.len() < min {
        value.push('x');
    }
    if let Some(max) = max {
        value.truncate(max);
    }
    Some(json!(value))
}

fn number_value(schema: &Value) -> Value {
    let minimum = schema.get("minimum").and_then(Value::as_f64);
    let exclusive = schema
        .get("exclusiveMinimum")
        .and_then(Value::as_f64)
        .map(|bound| bound + 1.0);
    let mut value = minimum.or(exclusive).unwrap_or(1.0);
    if let Some(maximum) = schema.get("maximum").and_then(Value::as_f64) {
        value = value.min(maximum);
    }
    if value.fract() == 0.0 {
        json!(value as i64)
    } else {
        json!(value)
    }
}

/// Arguments that violate `schema`, first rule that applies: omit the
/// required properties; else wrong-type the first typed property by name;
/// else an unknown property when the schema forbids extras. None when the
/// schema admits every input.
pub fn invalid_arguments(schema: &Value) -> Option<Value> {
    if !crate::init::zero_required(schema) {
        return Some(json!({}));
    }
    let typed = schema
        .get("properties")
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
        .find_map(|(name, property)| {
            property
                .get("type")
                .and_then(Value::as_str)
                .map(|kind| (name, kind))
        });
    if let Some((name, kind)) = typed {
        let wrong = if kind == "string" {
            json!(12345)
        } else {
            json!("mcpeval")
        };
        let mut arguments = serde_json::Map::new();
        arguments.insert(name.clone(), wrong);
        return Some(Value::Object(arguments));
    }
    (schema.get("additionalProperties") == Some(&Value::Bool(false)))
        .then(|| json!({"mcpeval_unknown": 1}))
}

/// Invalid arguments must be refused with -32602, or with an `isError`
/// result that says why.
fn honesty_of(response: &ToolResponse) -> Honesty {
    match response {
        ToolResponse::Error { code: -32602, .. } => Honesty::Honest,
        ToolResponse::Error { payload, .. }
            if payload.get("code").and_then(Value::as_str) == Some("tool-error") =>
        {
            let message = payload
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or_default();
            if message.is_empty() || message == crate::mcp_client::UNTEXTED_TOOL_ERROR {
                Honesty::Dishonest(CheckReason::HonestyEmptyError)
            } else {
                Honesty::Honest
            }
        }
        ToolResponse::Error { .. } => Honesty::Dishonest(CheckReason::HonestyWrongCode),
        ToolResponse::Success(_) => Honesty::Dishonest(CheckReason::HonestyAcceptedInvalid),
    }
}

/// One call with schema-violating arguments, then `ping`: the session must
/// answer after the refusal. Any JSON-RPC reply to `ping` counts; whether
/// `ping` itself is implemented is the protocol area's check.
fn probe_honesty(
    client: &mut Option<ProbeClient>,
    target: &ClientTarget,
    tool: &ToolDefinition,
) -> Option<Honesty> {
    let arguments = invalid_arguments(&tool.input_schema)?;
    if client.is_none() {
        *client = connect(target).ok();
    }
    let failed = Some(Honesty::Dishonest(CheckReason::HonestyCallFailed));
    let Some(active) = client.as_mut() else {
        return failed;
    };
    let Ok((response, _)) = active.call_tool_observing(
        &tool.name,
        &arguments,
        &mut |_, _| None,
        MAX_SERVER_REQUESTS,
    ) else {
        *client = connect(target).ok();
        return failed;
    };
    if active.raw_request("ping", json!({})).is_err() {
        *client = connect(target).ok();
        return failed;
    }
    Some(honesty_of(&response))
}

/// One `tools/list` page: its parsed entries and the next cursor.
fn list_page(
    client: &mut ProbeClient,
    params: Value,
) -> anyhow::Result<(Vec<ToolDefinition>, Option<String>)> {
    let response = client.raw_request("tools/list", params)?;
    let result = response
        .get("result")
        .context("tools/list returned an error")?;
    let entries = result
        .get("tools")
        .and_then(Value::as_array)
        .context("tools/list response is missing tools")?
        .iter()
        .map(crate::mcp_client::tool_definition)
        .collect::<anyhow::Result<Vec<_>>>()?;
    let next = result
        .get("nextCursor")
        .and_then(Value::as_str)
        .filter(|cursor| !cursor.is_empty())
        .map(str::to_owned);
    Ok((entries, next))
}

/// Three calls with `arguments`. A call that does not complete costs this
/// tool and the next tool starts on a fresh connection. Returns the
/// observation and whether all three calls succeeded.
fn exercise_tool(
    client: &mut Option<ProbeClient>,
    target: &ClientTarget,
    tool: &ToolDefinition,
    arguments: &Value,
) -> (Exercise, bool) {
    let mut outcomes = Vec::with_capacity(REPEATS);
    let mut latencies = Vec::with_capacity(REPEATS);
    let mut first_success: Option<Value> = None;
    if client.is_none() {
        *client = connect(target).ok();
    }
    for _ in 0..REPEATS {
        let Some(active) = client.as_mut() else {
            return (
                Exercise::NotExercised(CheckReason::CoverageCallFailed),
                false,
            );
        };
        let started = Instant::now();
        let response = active.call_tool_observing(
            &tool.name,
            arguments,
            &mut |_, _| None,
            MAX_SERVER_REQUESTS,
        );
        let latency_ms = started.elapsed().as_millis() as u64;
        let Ok((response, _)) = response else {
            *client = connect(target).ok();
            return (
                Exercise::NotExercised(CheckReason::CoverageCallFailed),
                false,
            );
        };
        let outcome = Outcome::of(&response);
        if outcome == Outcome::Rejected {
            return (
                Exercise::NotExercised(CheckReason::CoverageRejectedArguments),
                false,
            );
        }
        if let (ToolResponse::Success(result), None) = (&response, &first_success) {
            first_success = Some(result.clone());
        }
        outcomes.push(outcome);
        latencies.push(latency_ms);
    }
    latencies.sort_unstable();
    let consistent = outcomes.windows(2).all(|pair| pair[0] == pair[1]);
    let output_schema = tool.declared_output_schema().and_then(|schema| {
        first_success
            .as_ref()
            .map(|result| conforms(schema, result))
    });
    let all_succeeded = outcomes.iter().all(|outcome| *outcome == Outcome::Success);
    (
        Exercise::Exercised {
            consistent,
            median_latency_ms: latencies[REPEATS / 2],
            output_schema,
        },
        all_succeeded,
    )
}

/// The output-schema probe's rule: `structuredContent` present with every
/// top-level field the declared schema requires.
fn conforms(schema: &Value, result: &Value) -> bool {
    let Some(structured) = result.get("structuredContent") else {
        return false;
    };
    schema
        .get("required")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .all(|field| structured.get(field).is_some())
}

/// The first declared string property, by name.
fn string_property(tool: &ToolDefinition) -> Option<String> {
    tool.input_schema
        .get("properties")?
        .as_object()?
        .iter()
        .find(|(_, schema)| schema.get("type").and_then(Value::as_str) == Some("string"))
        .map(|(name, _)| name.clone())
}

/// A synthesized server-level case; a case that cannot complete fails.
fn server_case(
    client: &mut Option<ProbeClient>,
    target: &ClientTarget,
    catalog: &ToolCatalog,
    case: &ProbeCase,
) -> bool {
    if client.is_none() {
        *client = connect(target).ok();
    }
    let Some(active) = client.as_mut() else {
        return false;
    };
    match crate::probe::run_unjournaled(case, active, catalog, target, Some(CALL_TIMEOUT)) {
        Ok(report) => report.passed(),
        Err(_) => {
            *client = connect(target).ok();
            false
        }
    }
}

/// Protocol conformance, last so its re-handshakes cannot disturb the tool
/// calls. A check that cannot complete fails with `protocol-call-failed`
/// and the next check starts on a fresh connection.
fn protocol_checks(
    client: &mut Option<ProbeClient>,
    target: &ClientTarget,
    catalog: &ToolCatalog,
) -> Vec<(CheckId, Option<CheckReason>)> {
    let failed = Some(CheckReason::ProtocolCallFailed);
    let mut results = Vec::new();
    let unknown_method = raw(client, target, "mcpeval/unknown", json!({})).map(|response| {
        let code = response
            .get("error")
            .and_then(|error| error.get("code"))
            .and_then(Value::as_i64);
        (code != Some(-32601)).then_some(CheckReason::ProtocolUnknownMethodAnswered)
    });
    results.push((
        CheckId::ProtocolUnknownMethod,
        unknown_method.unwrap_or(failed),
    ));
    let ping = raw(client, target, "ping", json!({})).map(|response| {
        (!response.get("result").is_some_and(Value::is_object))
            .then_some(CheckReason::ProtocolPingFailed)
    });
    results.push((CheckId::ProtocolPing, ping.unwrap_or(failed)));
    let unknown_tool = raw(
        client,
        target,
        "tools/call",
        json!({"name": "mcpeval-unknown-tool", "arguments": {}}),
    )
    .map(|response| {
        let refused = response.get("error").is_some()
            || response
                .get("result")
                .and_then(|result| result.get("isError"))
                .and_then(Value::as_bool)
                == Some(true);
        (!refused).then_some(CheckReason::ProtocolUnknownToolAccepted)
    });
    results.push((CheckId::ProtocolUnknownTool, unknown_tool.unwrap_or(failed)));
    let pagination = ProbeCase::Pagination {
        id: "standard-pagination".into(),
        access: Access::ReadOnly,
        max_pages: MAX_PAGES as u64,
    };
    results.push((
        CheckId::ProtocolPagination,
        reason_of(client, target, catalog, &pagination, |_| {
            CheckReason::ProtocolPaginationInvalid
        }),
    ));
    let declares_surfaces = client
        .as_ref()
        .and_then(ProbeClient::capabilities)
        .is_some_and(|capabilities| {
            capabilities.get("resources").is_some() || capabilities.get("prompts").is_some()
        });
    if declares_surfaces {
        let surfaces = ProbeCase::SurfaceListing {
            id: "standard-surfaces".into(),
            access: Access::ReadOnly,
            max_pages: MAX_PAGES as u64,
        };
        results.push((
            CheckId::ProtocolSurfaces,
            reason_of(client, target, catalog, &surfaces, |_| {
                CheckReason::ProtocolSurfaceInvalid
            }),
        ));
    }
    let negotiation = ProbeCase::ProtocolNegotiation {
        id: "standard-negotiation".into(),
        access: Access::ReadOnly,
        bogus_version: "2000-01-01".into(),
    };
    results.push((
        CheckId::ProtocolNegotiation,
        reason_of(
            client,
            target,
            catalog,
            &negotiation,
            |reason| match reason {
                FailureReason::NegotiationEchoedUnknown => {
                    CheckReason::ProtocolNegotiationEchoedUnknown
                }
                FailureReason::NegotiationInconsistentSupport => {
                    CheckReason::ProtocolNegotiationInconsistentSupport
                }
                _ => CheckReason::ProtocolNegotiationInvalidVersion,
            },
        ),
    ));
    results
}

/// One raw request; None when it did not complete, after which the next
/// request starts on a fresh connection.
fn raw(
    client: &mut Option<ProbeClient>,
    target: &ClientTarget,
    method: &str,
    params: Value,
) -> Option<Value> {
    if client.is_none() {
        *client = connect(target).ok();
    }
    let response = client.as_mut()?.raw_request(method, params);
    if response.is_err() {
        *client = connect(target).ok();
    }
    response.ok()
}

/// Run a reused probe case; its failure maps to a check reason.
fn reason_of(
    client: &mut Option<ProbeClient>,
    target: &ClientTarget,
    catalog: &ToolCatalog,
    case: &ProbeCase,
    map: impl Fn(FailureReason) -> CheckReason,
) -> Option<CheckReason> {
    if client.is_none() {
        *client = connect(target).ok();
    }
    let Some(active) = client.as_mut() else {
        return Some(CheckReason::ProtocolCallFailed);
    };
    match crate::probe::run_unjournaled(case, active, catalog, target, Some(CALL_TIMEOUT)) {
        Ok(report) => match report.reason {
            None => None,
            Some(reason) if reason.is_transport() => Some(CheckReason::ProtocolCallFailed),
            Some(reason) => Some(map(reason)),
        },
        Err(_) => {
            *client = connect(target).ok();
            Some(CheckReason::ProtocolCallFailed)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn definition(entry: Value) -> ToolDefinition {
        crate::mcp_client::tool_definition(&entry).unwrap()
    }

    fn annotated(annotations: Value) -> ToolDefinition {
        definition(
            json!({"name": "t", "inputSchema": {"type": "object"}, "annotations": annotations}),
        )
    }

    #[test]
    fn writers_are_declared_by_either_annotation() {
        assert_eq!(
            ToolClass::of(&annotated(json!({"readOnlyHint": false}))),
            ToolClass::Writer
        );
        assert_eq!(
            ToolClass::of(&annotated(json!({"destructiveHint": true}))),
            ToolClass::Writer
        );
        assert_eq!(
            ToolClass::of(&annotated(
                json!({"readOnlyHint": true, "destructiveHint": true})
            )),
            ToolClass::Writer
        );
        assert_eq!(
            ToolClass::of(&annotated(json!({"readOnlyHint": true}))),
            ToolClass::ReadOnly
        );
        assert_eq!(ToolClass::of(&annotated(json!({}))), ToolClass::Unannotated);
    }

    #[test]
    fn outcomes_separate_rejections_tool_errors_and_protocol_errors() {
        let classify = |envelope: Value| {
            Outcome::of(&crate::mcp_client::classify_tool_response(&envelope).unwrap())
        };
        assert_eq!(
            classify(json!({"result": {"content": []}})),
            Outcome::Success
        );
        assert_eq!(
            classify(
                json!({"result": {"isError": true, "content": [{"type": "text", "text": "no"}]}})
            ),
            Outcome::ToolError
        );
        assert_eq!(
            classify(json!({"error": {"code": -32602, "message": "bad"}})),
            Outcome::Rejected
        );
        assert_eq!(
            classify(json!({"error": {"code": -32001, "message": "busy"}})),
            Outcome::RpcError
        );
    }

    #[test]
    fn output_conformance_needs_structured_content_with_required_fields() {
        let schema = json!({"type": "object", "required": ["a", "b"]});
        assert!(conforms(
            &schema,
            &json!({"structuredContent": {"a": 1, "b": 2}})
        ));
        assert!(!conforms(&schema, &json!({"structuredContent": {"a": 1}})));
        assert!(!conforms(&schema, &json!({"content": []})));
        assert!(conforms(
            &json!({"type": "object"}),
            &json!({"structuredContent": {}})
        ));
    }

    #[test]
    fn catalog_facts_count_presence_only() {
        let tool = definition(json!({
            "name": "t",
            "description": "Forty-one characters of useful description",
            "inputSchema": {"type": "object", "properties": {
                "a": {"type": "string", "description": "A"},
                "b": {"enum": [1, 2]},
                "c": {}
            }},
            "annotations": {"readOnlyHint": true}
        }));
        assert_eq!(
            CatalogFacts::of(&tool),
            CatalogFacts {
                description_chars: 42,
                properties: 3,
                described_properties: 1,
                typed_properties: 2,
                read_only_declared: true,
                destructive_declared: false,
                output_schema: false,
            }
        );
    }

    #[test]
    fn synthesis_follows_the_standard_rules_in_order() {
        let schema = json!({
            "type": "object",
            "required": ["kind", "fixed", "day", "at", "site", "mail", "key", "limit", "ratio",
                         "flag", "tags", "empty", "name", "filter", "pick", "code_or"],
            "properties": {
                "kind": {"type": "string", "enum": ["alpha", "beta"]},
                "fixed": {"const": 7},
                "day": {"type": "string", "format": "date"},
                "at": {"type": "string", "format": "date-time"},
                "site": {"type": "string", "format": "uri"},
                "mail": {"type": "string", "format": "email"},
                "key": {"type": "string", "format": "uuid"},
                "limit": {"type": "integer", "minimum": 5},
                "ratio": {"type": "number", "exclusiveMinimum": 0},
                "flag": {"type": "boolean"},
                "tags": {"type": "array", "minItems": 2, "items": {"type": "string", "default": "t"}},
                "empty": {"type": "array", "items": {"type": "string"}},
                "name": {"type": "string", "minLength": 10},
                "filter": {"$ref": "#/$defs/filter"},
                "pick": {"anyOf": [{"type": "integer"}, {"type": "string"}]},
                "code_or": {"type": ["null", "string"], "examples": ["x"]}
            },
            "$defs": {"filter": {"type": "object", "required": ["field"],
                                 "properties": {"field": {"type": "string"}}}}
        });
        assert_eq!(
            synthesize(&schema),
            Some(json!({
                "kind": "alpha", "fixed": 7, "day": "2026-01-01", "at": "2026-01-01T00:00:00Z",
                "site": "https://example.com", "mail": "user@example.com",
                "key": "00000000-0000-4000-8000-000000000000", "limit": 5, "ratio": 1,
                "flag": false, "tags": ["t", "t"], "empty": [], "name": "mcpevalxxx",
                "filter": {"field": "mcpeval"}, "pick": 1, "code_or": "x"
            }))
        );
        let patterned = json!({"type": "object", "required": ["code"],
                               "properties": {"code": {"type": "string", "pattern": "^[A-Z]{3}$"}}});
        assert_eq!(synthesize(&patterned), None);
        let undeclared = json!({"type": "object", "required": ["ghost"], "properties": {}});
        assert_eq!(synthesize(&undeclared), None);
        let short = json!({"type": "object", "required": ["s"],
                           "properties": {"s": {"type": "string", "maxLength": 3}}});
        assert_eq!(synthesize(&short), Some(json!({"s": "mcp"})));
        // A self-referencing schema stops at the depth bound.
        let endless = json!({"type": "object", "required": ["next"],
                             "properties": {"next": {"$ref": "#"}}});
        assert_eq!(synthesize(&endless), None);
        assert_eq!(synthesize(&json!({"type": "object"})), Some(json!({})));
    }

    #[test]
    fn invalid_arguments_follow_the_standard_order() {
        // Omit the required properties first.
        assert_eq!(
            invalid_arguments(&json!({
                "type": "object", "required": ["id"], "properties": {"id": {"type": "string"}}
            })),
            Some(json!({}))
        );
        // Else wrong-type the first typed property by name.
        assert_eq!(
            invalid_arguments(
                &json!({"type": "object", "properties": {"city": {"type": "string"}}})
            ),
            Some(json!({"city": 12345}))
        );
        assert_eq!(
            invalid_arguments(&json!({"type": "object", "properties": {
                "port": {"type": "integer"}, "host": {"description": "untyped"}
            }})),
            Some(json!({"port": "mcpeval"}))
        );
        // Else an unknown property the schema forbids.
        assert_eq!(
            invalid_arguments(&json!({"type": "object", "additionalProperties": false})),
            Some(json!({"mcpeval_unknown": 1}))
        );
        // A schema that admits every input cannot be violated.
        assert_eq!(
            invalid_arguments(&json!({"type": "object", "properties": {"x": {}}})),
            None
        );
        assert_eq!(invalid_arguments(&json!({"type": "object"})), None);
    }

    #[test]
    fn honesty_needs_a_refusal_with_words() {
        let judge = |envelope: Value| {
            honesty_of(&crate::mcp_client::classify_tool_response(&envelope).unwrap())
        };
        assert_eq!(
            judge(json!({"error": {"code": -32602, "message": "bad"}})),
            Honesty::Honest
        );
        assert_eq!(
            judge(json!({"result": {"isError": true, "content": [
                {"type": "text", "text": "city must be a string"}
            ]}})),
            Honesty::Honest
        );
        assert_eq!(
            judge(json!({"result": {"isError": true, "content": []}})),
            Honesty::Dishonest(CheckReason::HonestyEmptyError)
        );
        assert_eq!(
            judge(json!({"result": {"isError": true, "content": [{"type": "text", "text": ""}]}})),
            Honesty::Dishonest(CheckReason::HonestyEmptyError)
        );
        assert_eq!(
            judge(json!({"result": {"content": []}})),
            Honesty::Dishonest(CheckReason::HonestyAcceptedInvalid)
        );
        assert_eq!(
            judge(json!({"error": {"code": -32603, "message": "boom"}})),
            Honesty::Dishonest(CheckReason::HonestyWrongCode)
        );
    }

    #[test]
    fn the_payload_field_is_the_first_string_property_by_name() {
        let tool = definition(
            json!({"name": "t", "inputSchema": {"type": "object", "properties": {
                "c": {"type": "string"}, "a": {"type": "integer"}, "b": {"type": "string"}
            }}}),
        );
        assert_eq!(string_property(&tool).as_deref(), Some("b"));
        assert_eq!(string_property(&annotated(json!({}))), None);
    }
}
