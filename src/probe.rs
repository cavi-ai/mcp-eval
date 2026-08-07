use std::path::PathBuf;
use std::time::Instant;

use crate::fingerprint::Salt;
use crate::manifest::{Access, Expectation, Manifest, OutcomeExpectation, ProbeCase, ProbeKind};
use crate::mcp_client::{McpClient, ToolCatalog, ToolDefinition, ToolResponse};
use crate::record::{error_info, CallRecord};
use crate::store::Store;
use anyhow::{bail, Context};

#[derive(Debug)]
pub struct ProbeOptions {
    pub server: String,
    pub manifest_path: PathBuf,
    pub selected_probe: Option<ProbeKind>,
    pub selected_case: Option<String>,
    pub allow_mutation: bool,
    pub command: Vec<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FailureReason {
    UnexpectedOutcome,
    MissingField,
    ValueMismatch,
    ErrorCodeMismatch,
    DiscoveryLimitExceeded,
    InvalidSchema,
    MissingRequiredArgument,
}

#[derive(Debug)]
pub struct CaseReport {
    pub id: String,
    pub probe: ProbeKind,
    pub attempts: u64,
    pub first_failure: Option<u64>,
    pub reason: Option<FailureReason>,
    pub tool_count: Option<u64>,
    pub schema_bytes: Option<u64>,
}

impl CaseReport {
    pub fn passed(&self) -> bool {
        self.reason.is_none()
    }
}

#[derive(Debug)]
pub struct ProbeReport {
    pub cases: Vec<CaseReport>,
}

impl ProbeReport {
    pub fn passed(&self) -> bool {
        self.cases.iter().all(CaseReport::passed)
    }
}

pub fn run(options: ProbeOptions, store: &mut Store) -> anyhow::Result<ProbeReport> {
    if !crate::privacy::valid_server(&options.server) {
        bail!("server label is invalid");
    }
    let manifest = Manifest::load(&options.manifest_path)?;
    if options.selected_probe.is_some() && options.selected_case.is_some() {
        bail!("select a probe kind or a probe case, not both");
    }
    let cases: Vec<&ProbeCase> = manifest
        .probes
        .iter()
        .filter(|case| {
            options
                .selected_case
                .as_deref()
                .is_none_or(|id| case.id() == id)
                && options
                    .selected_probe
                    .is_none_or(|kind| case.kind() == kind)
        })
        .collect();
    if cases.is_empty() {
        bail!("no probe cases selected");
    }
    if cases.iter().any(|case| case.access() == Access::Mutating) && !options.allow_mutation {
        bail!("mutating probes require --allow-mutation");
    }

    let salt = Salt::load(store.root())?;
    let session = uuid::Uuid::new_v4().to_string();
    let mut seq = 0;
    let mut client = McpClient::spawn(&options.command)?;
    client.initialize()?;
    let catalog = client.list_tools_catalog()?;
    for case in &cases {
        if case
            .tool()
            .is_some_and(|name| !catalog.tools.iter().any(|tool| tool.name == name))
        {
            bail!("probe tool was not declared by the server");
        }
    }
    let mut reports = Vec::with_capacity(cases.len());
    let mut context = RunContext {
        server: &options.server,
        session: &session,
        seq: &mut seq,
        salt: &salt,
        client: &mut client,
        catalog: &catalog,
        store,
    };
    for case in cases {
        reports.push(run_case(case, &mut context)?);
    }
    Ok(ProbeReport { cases: reports })
}

struct RunContext<'a> {
    server: &'a str,
    session: &'a str,
    seq: &'a mut u64,
    salt: &'a Salt,
    client: &'a mut McpClient,
    catalog: &'a ToolCatalog,
    store: &'a mut Store,
}

fn run_case(case: &ProbeCase, context: &mut RunContext<'_>) -> anyhow::Result<CaseReport> {
    match case {
        ProbeCase::DiscoveryCost {
            max_tools,
            max_schema_bytes,
            ..
        } => {
            let tool_count = context.catalog.tools.len() as u64;
            let schema_bytes = context.catalog.encoded_bytes as u64;
            let reason = (tool_count > *max_tools || schema_bytes > *max_schema_bytes)
                .then_some(FailureReason::DiscoveryLimitExceeded);
            Ok(CaseReport {
                id: case.id().to_owned(),
                probe: case.kind(),
                attempts: 1,
                first_failure: reason.map(|_| 1),
                reason,
                tool_count: Some(tool_count),
                schema_bytes: Some(schema_bytes),
            })
        }
        ProbeCase::SchemaGuessability { .. } => {
            let definition = context
                .catalog
                .tools
                .iter()
                .find(|tool| Some(tool.name.as_str()) == case.tool())
                .expect("selected tool was preflighted");
            let preflight =
                check_schema(definition, case.arguments().expect("schema case has args"));
            let reason = if preflight.is_some() {
                preflight
            } else {
                match call_and_record(case, context)? {
                    ToolResponse::Success(_) => None,
                    ToolResponse::Error { .. } => Some(FailureReason::UnexpectedOutcome),
                }
            };
            Ok(CaseReport {
                id: case.id().to_owned(),
                probe: case.kind(),
                attempts: u64::from(preflight.is_none()),
                first_failure: reason.map(|_| 1),
                reason,
                tool_count: None,
                schema_bytes: None,
            })
        }
        ProbeCase::DegradationOverN { .. } => {
            let limit = case.max_attempts().expect("degradation case has a bound");
            for attempt in 1..=limit {
                let response = call_and_record(case, context)?;
                if matches!(response, ToolResponse::Error { .. }) {
                    return Ok(CaseReport {
                        id: case.id().to_owned(),
                        probe: case.kind(),
                        attempts: attempt,
                        first_failure: Some(attempt),
                        reason: Some(FailureReason::UnexpectedOutcome),
                        tool_count: None,
                        schema_bytes: None,
                    });
                }
            }
            Ok(CaseReport {
                id: case.id().to_owned(),
                probe: case.kind(),
                attempts: limit,
                first_failure: None,
                reason: None,
                tool_count: None,
                schema_bytes: None,
            })
        }
        ProbeCase::InstructionFidelity { .. } => {
            let response = call_and_record(case, context)?;
            let reason = check_expectation(
                case.expectation().expect("fidelity case has expectation"),
                &response,
            );
            Ok(CaseReport {
                id: case.id().to_owned(),
                probe: case.kind(),
                attempts: 1,
                first_failure: reason.map(|_| 1),
                reason,
                tool_count: None,
                schema_bytes: None,
            })
        }
    }
}

fn call_and_record(case: &ProbeCase, context: &mut RunContext<'_>) -> anyhow::Result<ToolResponse> {
    *context.seq += 1;
    let started = Instant::now();
    let tool = case.tool().expect("call probe has a tool");
    let arguments = case.arguments().expect("call probe has arguments");
    let response = context.client.call_tool(tool, arguments)?;
    let latency_ms = started.elapsed().as_millis() as u64;
    let (outcome, error) = match &response {
        ToolResponse::Success(_) => ("ok", None),
        ToolResponse::Error { payload, .. } => ("error", Some(error_info(payload, context.salt))),
    };
    context
        .store
        .append(&CallRecord {
            ts: chrono::Utc::now()
                .format("%Y-%m-%dT%H:%M:%S%.3fZ")
                .to_string(),
            session: context.session.to_owned(),
            seq: *context.seq,
            server: context.server.to_owned(),
            method: "tools/call".into(),
            tool: Some(tool.to_owned()),
            args: Some(arguments.clone()),
            latency_ms: Some(latency_ms),
            outcome: outcome.into(),
            error,
            shim_self_us: 0,
            kind: "synthetic".into(),
        })
        .context("recording probe call")?;
    Ok(response)
}

fn check_schema(
    definition: &ToolDefinition,
    arguments: &serde_json::Value,
) -> Option<FailureReason> {
    let Some(schema) = definition.input_schema.as_object() else {
        return Some(FailureReason::InvalidSchema);
    };
    if schema.get("type").and_then(serde_json::Value::as_str) != Some("object") {
        return Some(FailureReason::InvalidSchema);
    }
    let properties = match schema.get("properties") {
        None => serde_json::Map::new(),
        Some(value) => match value.as_object() {
            Some(properties) => properties.clone(),
            None => return Some(FailureReason::InvalidSchema),
        },
    };
    let required = match schema.get("required") {
        None => &[][..],
        Some(value) => match value.as_array() {
            Some(required) => required.as_slice(),
            None => return Some(FailureReason::InvalidSchema),
        },
    };
    let arguments = arguments.as_object().expect("manifest validates arguments");
    for field in required {
        let Some(field) = field.as_str() else {
            return Some(FailureReason::InvalidSchema);
        };
        if !properties.contains_key(field) {
            return Some(FailureReason::InvalidSchema);
        }
        if !arguments.contains_key(field) {
            return Some(FailureReason::MissingRequiredArgument);
        }
    }
    None
}

fn check_expectation(expect: &Expectation, response: &ToolResponse) -> Option<FailureReason> {
    match (expect.outcome, response) {
        (OutcomeExpectation::Ok, ToolResponse::Error { .. })
        | (OutcomeExpectation::Error, ToolResponse::Success(_)) => {
            Some(FailureReason::UnexpectedOutcome)
        }
        (OutcomeExpectation::Error, ToolResponse::Error { code, .. }) => expect
            .error_code
            .filter(|expected| expected != code)
            .map(|_| FailureReason::ErrorCodeMismatch),
        (OutcomeExpectation::Ok, ToolResponse::Success(result)) => {
            let Some(result) = result.as_object() else {
                return Some(FailureReason::MissingField);
            };
            if expect
                .required_result_fields
                .iter()
                .any(|field| !result.contains_key(field))
            {
                return Some(FailureReason::MissingField);
            }
            if expect
                .equals
                .iter()
                .any(|(field, expected)| result.get(field) != Some(expected))
            {
                return Some(FailureReason::ValueMismatch);
            }
            None
        }
    }
}
