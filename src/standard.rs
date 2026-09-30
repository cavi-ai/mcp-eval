//! The mcpeval standard battery: read-only observations of a server's
//! whole catalog, folded into readiness by [`crate::score`]. It runs on
//! its own connection, never calls a tool declared a writer, and journals
//! nothing: its deliberate calls must not become promoted findings.

use std::time::{Duration, Instant};

use anyhow::Context;
use serde_json::{json, Value};

use crate::manifest::{Access, ProbeCase};
use crate::mcp_client::{ToolCatalog, ToolDefinition, ToolResponse};
use crate::probe::{estimate_tokens, ClientTarget, ProbeClient};
use crate::score::{CheckReason, Readiness};

/// Per-call response timeout, pinned by the standard: it bounds what a
/// hung tool costs and is part of what a score means.
pub const CALL_TIMEOUT: Duration = Duration::from_secs(10);
/// Calls per exercised tool.
pub const REPEATS: usize = 3;
/// `tools/list` pages followed before the catalog is taken as complete.
pub const MAX_PAGES: usize = 20;
/// Server-to-client requests declined during one tool call.
const MAX_SERVER_REQUESTS: u64 = 8;
const PAYLOAD_BYTES: u64 = 1_000_000;

#[derive(Clone, Debug, Default)]
pub struct StandardOptions {
    /// Attest that unannotated tools are read-only, so they are called.
    pub confirm_read_only: bool,
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

#[derive(Clone, Debug, PartialEq)]
pub struct ToolObservation {
    pub name: String,
    /// `estimate_tokens` of the tool's `tools/list` entry.
    pub tokens: u64,
    pub class: ToolClass,
    /// None for writers, which are never called.
    pub exercise: Option<Exercise>,
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
    let mut tools = Vec::with_capacity(catalog.tools.len());
    // Tools whose three calls all succeeded, with the arguments used: the
    // server-level cases run on the first of them.
    let mut succeeded: Vec<(&ToolDefinition, Value)> = Vec::new();
    for tool in &catalog.tools {
        let class = ToolClass::of(tool);
        let exercise = match class {
            ToolClass::Writer => None,
            ToolClass::Unannotated if !options.confirm_read_only => {
                Some(Exercise::NotExercised(CheckReason::CoverageUnannotated))
            }
            _ if !crate::init::zero_required(&tool.input_schema) => Some(Exercise::NotExercised(
                CheckReason::CoverageRequiredArguments,
            )),
            _ => {
                let arguments = json!({});
                let (exercise, all_succeeded) =
                    exercise_tool(&mut client, target, tool, &arguments);
                if all_succeeded {
                    succeeded.push((tool, arguments));
                }
                Some(exercise)
            }
        };
        tools.push(ToolObservation {
            name: tool.name.clone(),
            tokens: estimate_tokens(tool.entry_bytes),
            class,
            exercise,
        });
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
    Ok(Observations {
        tools,
        attested_read_only: options.confirm_read_only,
        contention,
        payload,
    })
}

/// Every page of `tools/list`, each distinct tool once, at most
/// [`MAX_PAGES`] pages.
fn list_catalog(client: &mut ProbeClient) -> anyhow::Result<ToolCatalog> {
    let mut tools: Vec<ToolDefinition> = Vec::new();
    let mut cursor: Option<String> = None;
    for _ in 0..MAX_PAGES {
        let params = match &cursor {
            Some(cursor) => json!({"cursor": cursor}),
            None => json!({}),
        };
        let response = client.raw_request("tools/list", params)?;
        let result = response
            .get("result")
            .context("tools/list returned an error")?;
        let entries = result
            .get("tools")
            .and_then(Value::as_array)
            .context("tools/list response is missing tools")?;
        for entry in entries {
            let tool = crate::mcp_client::tool_definition(entry)?;
            if !tools.iter().any(|seen| seen.name == tool.name) {
                tools.push(tool);
            }
        }
        cursor = result
            .get("nextCursor")
            .and_then(Value::as_str)
            .filter(|cursor| !cursor.is_empty())
            .map(str::to_owned);
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
