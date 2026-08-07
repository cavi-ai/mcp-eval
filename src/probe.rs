use std::path::PathBuf;
use std::time::Instant;

use crate::fingerprint::Salt;
use crate::manifest::{Access, Expectation, Manifest, OutcomeExpectation, ProbeCase, ProbeKind};
use crate::mcp_client::{McpClient, ToolResponse};
use crate::record::{error_info, CallRecord};
use crate::store::Store;
use anyhow::{bail, Context};

#[derive(Debug)]
pub struct ProbeOptions {
    pub server: String,
    pub manifest_path: PathBuf,
    pub selected_probe: Option<ProbeKind>,
    pub allow_mutation: bool,
    pub command: Vec<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FailureReason {
    UnexpectedOutcome,
    MissingField,
    ValueMismatch,
    ErrorCodeMismatch,
}

#[derive(Debug)]
pub struct CaseReport {
    pub id: String,
    pub probe: ProbeKind,
    pub attempts: u64,
    pub first_failure: Option<u64>,
    pub reason: Option<FailureReason>,
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
    let cases: Vec<&ProbeCase> = manifest
        .probes
        .iter()
        .filter(|case| {
            options
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
    let tools = client.list_tools()?;
    for case in &cases {
        if !tools.iter().any(|tool| tool == case.tool()) {
            bail!("probe tool was not declared by the server");
        }
    }
    let mut reports = Vec::with_capacity(cases.len());
    for case in cases {
        reports.push(run_case(
            case,
            &options.server,
            &session,
            &mut seq,
            &salt,
            &mut client,
            store,
        )?);
    }
    Ok(ProbeReport { cases: reports })
}

fn run_case(
    case: &ProbeCase,
    server: &str,
    session: &str,
    seq: &mut u64,
    salt: &Salt,
    client: &mut McpClient,
    store: &mut Store,
) -> anyhow::Result<CaseReport> {
    match case {
        ProbeCase::DegradationOverN { .. } => {
            let limit = case.max_attempts().expect("degradation case has a bound");
            for attempt in 1..=limit {
                let response = call_and_record(case, server, session, seq, salt, client, store)?;
                if matches!(response, ToolResponse::Error { .. }) {
                    return Ok(CaseReport {
                        id: case.id().to_owned(),
                        probe: case.kind(),
                        attempts: attempt,
                        first_failure: Some(attempt),
                        reason: Some(FailureReason::UnexpectedOutcome),
                    });
                }
            }
            Ok(CaseReport {
                id: case.id().to_owned(),
                probe: case.kind(),
                attempts: limit,
                first_failure: None,
                reason: None,
            })
        }
        ProbeCase::InstructionFidelity { .. } => {
            let response = call_and_record(case, server, session, seq, salt, client, store)?;
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
            })
        }
    }
}

fn call_and_record(
    case: &ProbeCase,
    server: &str,
    session: &str,
    seq: &mut u64,
    salt: &Salt,
    client: &mut McpClient,
    store: &mut Store,
) -> anyhow::Result<ToolResponse> {
    *seq += 1;
    let started = Instant::now();
    let response = client.call_tool(case.tool(), case.arguments())?;
    let latency_ms = started.elapsed().as_millis() as u64;
    let (outcome, error) = match &response {
        ToolResponse::Success(_) => ("ok", None),
        ToolResponse::Error { payload, .. } => ("error", Some(error_info(payload, salt))),
    };
    store
        .append(&CallRecord {
            ts: chrono::Utc::now()
                .format("%Y-%m-%dT%H:%M:%S%.3fZ")
                .to_string(),
            session: session.to_owned(),
            seq: *seq,
            server: server.to_owned(),
            method: "tools/call".into(),
            tool: Some(case.tool().to_owned()),
            args: Some(case.arguments().clone()),
            latency_ms: Some(latency_ms),
            outcome: outcome.into(),
            error,
            shim_self_us: 0,
            kind: "synthetic".into(),
        })
        .context("recording probe call")?;
    Ok(response)
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
