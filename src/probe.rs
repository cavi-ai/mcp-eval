use std::path::PathBuf;
use std::sync::{mpsc, Arc, Barrier};
use std::time::Instant;

use crate::fingerprint::Salt;
use crate::http_client::HttpMcpClient;
use crate::manifest::{Access, Expectation, Manifest, OutcomeExpectation, ProbeCase, ProbeKind};
use crate::mcp_client::{McpClient, ToolCatalog, ToolDefinition, ToolResponse};
use crate::record::{error_info, CallRecord};
use crate::store::Store;
use anyhow::{bail, Context};
use serde_json::{json, Value};

#[derive(Debug)]
pub struct ProbeOptions {
    pub server: String,
    pub manifest_path: PathBuf,
    /// Inline manifest JSON; when set, `manifest_path` is ignored. Used by
    /// surfaces that receive the manifest over the wire (mcpeval serve).
    pub manifest_inline: Option<String>,
    pub selected_probe: Option<ProbeKind>,
    pub selected_case: Option<String>,
    pub allow_mutation: bool,
    pub command: Vec<String>,
    pub http_url: Option<String>,
    pub allow_remote_http: bool,
}

#[derive(Clone, Debug)]
enum ClientTarget {
    Stdio(Vec<String>),
    Http {
        endpoint: String,
        allow_remote: bool,
    },
}

enum ProbeClient {
    Stdio(McpClient),
    Http(HttpMcpClient),
}

impl ClientTarget {
    fn from_options(options: &ProbeOptions) -> anyhow::Result<Self> {
        match (&options.http_url, options.command.is_empty()) {
            (None, false) => Ok(Self::Stdio(options.command.clone())),
            (Some(endpoint), true) => Ok(Self::Http {
                endpoint: endpoint.clone(),
                allow_remote: options.allow_remote_http,
            }),
            (Some(_), false) => bail!("select an HTTP endpoint or a stdio command, not both"),
            (None, true) => bail!("an HTTP endpoint or stdio command is required"),
        }
    }

    fn connect(&self) -> anyhow::Result<ProbeClient> {
        match self {
            Self::Stdio(command) => Ok(ProbeClient::Stdio(McpClient::spawn(command)?)),
            Self::Http {
                endpoint,
                allow_remote,
            } => Ok(ProbeClient::Http(HttpMcpClient::connect(
                endpoint,
                *allow_remote,
            )?)),
        }
    }
}

impl ProbeClient {
    fn initialize(&mut self) -> anyhow::Result<()> {
        match self {
            Self::Stdio(client) => client.initialize(),
            Self::Http(client) => client.initialize(),
        }
    }

    fn list_tools(&mut self) -> anyhow::Result<Vec<String>> {
        match self {
            Self::Stdio(client) => client.list_tools(),
            Self::Http(client) => client.list_tools(),
        }
    }

    fn list_tools_catalog(&mut self) -> anyhow::Result<ToolCatalog> {
        match self {
            Self::Stdio(client) => client.list_tools_catalog(),
            Self::Http(client) => client.list_tools_catalog(),
        }
    }

    fn call_tool(
        &mut self,
        tool: &str,
        arguments: &serde_json::Value,
    ) -> anyhow::Result<ToolResponse> {
        match self {
            Self::Stdio(client) => client.call_tool(tool, arguments),
            Self::Http(client) => client.call_tool(tool, arguments),
        }
    }

    fn raw_request(
        &mut self,
        method: &str,
        params: serde_json::Value,
    ) -> anyhow::Result<serde_json::Value> {
        match self {
            Self::Stdio(client) => client.raw_request(method, params),
            Self::Http(client) => client.raw_request(method, params),
        }
    }

    fn capabilities(&self) -> Option<serde_json::Value> {
        match self {
            Self::Stdio(client) => client.capabilities(),
            Self::Http(client) => client.capabilities(),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FailureReason {
    UnexpectedOutcome,
    MissingField,
    ValueMismatch,
    ErrorCodeMismatch,
    DiscoveryLimitExceeded,
    TokenBudgetExceeded,
    InvalidSchema,
    MissingRequiredArgument,
    ExpectedError,
    UnstableErrorCode,
    RetryabilityMismatch,
    RetryDidNotRecover,
    FailureNotObserved,
    RecoveryFailed,
    ValidationFailed,
    ContendedClientFailed,
    LatencyBudgetExceeded,
    PaginationInvalidEntry,
    PaginationDuplicateTool,
    PaginationStalledCursor,
    PayloadUnhandled,
    SurfaceInvalidEnvelope,
    SurfaceStalledCursor,
    OutputSchemaDeclaredButMissing,
    OutputSchemaFieldMissing,
}

impl FailureReason {
    /// Every reason, in stable order, for `mcpeval explain` with no
    /// argument.
    pub const ALL: &[FailureReason] = &[
        Self::UnexpectedOutcome,
        Self::MissingField,
        Self::ValueMismatch,
        Self::ErrorCodeMismatch,
        Self::DiscoveryLimitExceeded,
        Self::TokenBudgetExceeded,
        Self::InvalidSchema,
        Self::MissingRequiredArgument,
        Self::ExpectedError,
        Self::UnstableErrorCode,
        Self::RetryabilityMismatch,
        Self::RetryDidNotRecover,
        Self::FailureNotObserved,
        Self::RecoveryFailed,
        Self::ValidationFailed,
        Self::ContendedClientFailed,
        Self::LatencyBudgetExceeded,
        Self::PaginationInvalidEntry,
        Self::PaginationDuplicateTool,
        Self::PaginationStalledCursor,
        Self::PayloadUnhandled,
        Self::SurfaceInvalidEnvelope,
        Self::SurfaceStalledCursor,
        Self::OutputSchemaDeclaredButMissing,
        Self::OutputSchemaFieldMissing,
    ];

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::UnexpectedOutcome => "unexpected-outcome",
            Self::MissingField => "missing-field",
            Self::ValueMismatch => "value-mismatch",
            Self::ErrorCodeMismatch => "error-code-mismatch",
            Self::DiscoveryLimitExceeded => "discovery-limit-exceeded",
            Self::TokenBudgetExceeded => "token-budget-exceeded",
            Self::InvalidSchema => "invalid-schema",
            Self::MissingRequiredArgument => "missing-required-argument",
            Self::ExpectedError => "expected-error",
            Self::UnstableErrorCode => "unstable-error-code",
            Self::RetryabilityMismatch => "retryability-mismatch",
            Self::RetryDidNotRecover => "retry-did-not-recover",
            Self::FailureNotObserved => "failure-not-observed",
            Self::RecoveryFailed => "recovery-failed",
            Self::ValidationFailed => "validation-failed",
            Self::ContendedClientFailed => "contended-client-failed",
            Self::LatencyBudgetExceeded => "latency-budget-exceeded",
            Self::PaginationInvalidEntry => "pagination-invalid-entry",
            Self::PaginationDuplicateTool => "pagination-duplicate-tool",
            Self::PaginationStalledCursor => "pagination-stalled-cursor",
            Self::PayloadUnhandled => "payload-unhandled",
            Self::SurfaceInvalidEnvelope => "surface-invalid-envelope",
            Self::SurfaceStalledCursor => "surface-stalled-cursor",
            Self::OutputSchemaDeclaredButMissing => "output-schema-declared-but-missing",
            Self::OutputSchemaFieldMissing => "output-schema-field-missing",
        }
    }
}

/// Deterministic, model-independent token estimate: one token per
/// `CHARS_PER_TOKEN` encoded bytes, rounded up. This is a heuristic budget
/// unit, not a specific model's tokenizer; it is stable across runs so that
/// manifests and baselines can compare like with like.
pub const CHARS_PER_TOKEN: usize = 4;

pub fn estimate_tokens(encoded_bytes: usize) -> u64 {
    encoded_bytes.div_ceil(CHARS_PER_TOKEN) as u64
}

#[derive(Debug)]
pub struct ToolTokenUsage {
    pub tool: String,
    pub tokens: u64,
}

#[derive(Debug)]
pub struct TokenUsage {
    pub total_tokens: u64,
    /// Sorted by tokens descending, then tool name, for stable output.
    pub per_tool: Vec<ToolTokenUsage>,
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
    pub token_usage: Option<TokenUsage>,
    /// Slowest observed call for latency-budget cases.
    pub latency_ms: Option<u64>,
    /// Number of `tools/list` pages visited by pagination cases.
    pub pages: Option<u64>,
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

    /// Versioned, deterministic JSON document: no timestamps, no session
    /// identifiers, cases in manifest order. Contains only share-safe
    /// fields — server label, case IDs, probe kinds, counts, fixed reason
    /// labels, measurement numbers, and the readiness score. Suitable for
    /// CI artifacts and committed baselines.
    pub fn to_json(&self, server: &str) -> serde_json::Value {
        let cases: Vec<serde_json::Value> = self
            .cases
            .iter()
            .map(|case| {
                let mut measurements = serde_json::Map::new();
                if let Some(tool_count) = case.tool_count {
                    measurements.insert("tool_count".into(), tool_count.into());
                }
                if let Some(schema_bytes) = case.schema_bytes {
                    measurements.insert("schema_bytes".into(), schema_bytes.into());
                }
                if let Some(usage) = &case.token_usage {
                    measurements.insert("total_tokens".into(), usage.total_tokens.into());
                    measurements.insert(
                        "per_tool".into(),
                        serde_json::Value::Array(
                            usage
                                .per_tool
                                .iter()
                                .map(|tool| {
                                    serde_json::json!({"tool": tool.tool, "tokens": tool.tokens})
                                })
                                .collect(),
                        ),
                    );
                }
                if let Some(latency_ms) = case.latency_ms {
                    measurements.insert("latency_ms".into(), latency_ms.into());
                }
                if let Some(pages) = case.pages {
                    measurements.insert("pages".into(), pages.into());
                }
                serde_json::json!({
                    "id": case.id,
                    "probe": case.probe.as_str(),
                    "passed": case.passed(),
                    "attempts": case.attempts,
                    "first_failure": case.first_failure,
                    "reason": case.reason.map(|reason| reason.as_str()),
                    "measurements": serde_json::Value::Object(measurements),
                })
            })
            .collect();
        let readiness = crate::score::readiness(self);
        serde_json::json!({
            "schema": "mcpeval.probe-report/v1",
            "server": server,
            "passed": self.passed(),
            "readiness": readiness.to_json(),
            "cases": cases,
        })
    }
}

pub fn run(options: ProbeOptions, store: &mut Store) -> anyhow::Result<ProbeReport> {
    if !crate::privacy::valid_server(&options.server) {
        bail!("server label is invalid");
    }
    let manifest = match &options.manifest_inline {
        Some(body) => {
            let manifest: Manifest =
                serde_json::from_str(body).context("parsing inline manifest structure")?;
            manifest.validate()?;
            manifest
        }
        None => Manifest::load(&options.manifest_path)?,
    };
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
    let target = ClientTarget::from_options(&options)?;

    let salt = Salt::load(store.root())?;
    let session = uuid::Uuid::new_v4().to_string();
    let mut seq = 0;
    let mut client = target.connect()?;
    client.initialize()?;
    let catalog = client.list_tools_catalog()?;
    for case in &cases {
        if case
            .required_tools()
            .iter()
            .any(|name| !catalog.tools.iter().any(|tool| tool.name == **name))
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
        target: &target,
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
    client: &'a mut ProbeClient,
    catalog: &'a ToolCatalog,
    target: &'a ClientTarget,
    store: &'a mut Store,
}

fn run_case(case: &ProbeCase, context: &mut RunContext<'_>) -> anyhow::Result<CaseReport> {
    match case {
        ProbeCase::Contention { .. } => run_contention(case, context),
        ProbeCase::ErrorHonesty {
            max_attempts,
            expect_retryable,
            ..
        } => run_error_honesty(case, *max_attempts, *expect_retryable, context),
        ProbeCase::StateRecovery {
            failure_tool,
            failure_arguments,
            recovery_tool,
            recovery_arguments,
            validation_tool,
            validation_arguments,
            ..
        } => run_state_recovery(
            case,
            RecoveryPlan {
                failure_tool,
                failure_arguments,
                recovery_tool,
                recovery_arguments,
                validation_tool,
                validation_arguments,
            },
            context,
        ),
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
                token_usage: None,
                latency_ms: None,
                pages: None,
            })
        }
        ProbeCase::TokenCost {
            max_total_tokens,
            max_tool_tokens,
            ..
        } => {
            let mut per_tool: Vec<ToolTokenUsage> = context
                .catalog
                .tools
                .iter()
                .map(|tool| ToolTokenUsage {
                    tool: tool.name.clone(),
                    tokens: estimate_tokens(tool.entry_bytes),
                })
                .collect();
            per_tool.sort_by(|a, b| b.tokens.cmp(&a.tokens).then_with(|| a.tool.cmp(&b.tool)));
            let total_tokens = per_tool.iter().map(|tool| tool.tokens).sum();
            let over_total = total_tokens > *max_total_tokens;
            let over_tool = max_tool_tokens
                .is_some_and(|limit| per_tool.iter().any(|tool| tool.tokens > limit));
            let reason = (over_total || over_tool).then_some(FailureReason::TokenBudgetExceeded);
            Ok(CaseReport {
                id: case.id().to_owned(),
                probe: case.kind(),
                attempts: 1,
                first_failure: reason.map(|_| 1),
                reason,
                tool_count: Some(context.catalog.tools.len() as u64),
                schema_bytes: None,
                token_usage: Some(TokenUsage {
                    total_tokens,
                    per_tool,
                }),
                latency_ms: None,
                pages: None,
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
                token_usage: None,
                latency_ms: None,
                pages: None,
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
                        token_usage: None,
                        latency_ms: None,
                        pages: None,
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
                token_usage: None,
                latency_ms: None,
                pages: None,
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
                token_usage: None,
                latency_ms: None,
                pages: None,
            })
        }
        ProbeCase::LatencyBudget { max_latency_ms, .. } => {
            run_latency_budget(case, *max_latency_ms, context)
        }
        ProbeCase::Pagination { max_pages, .. } => run_pagination(case, *max_pages, context),
        ProbeCase::PayloadBounds {
            field, size_bytes, ..
        } => {
            let expect_handled = match case {
                ProbeCase::PayloadBounds { expect_handled, .. } => *expect_handled,
                _ => unreachable!("payload arm"),
            };
            run_payload_bounds(case, field, *size_bytes, expect_handled, context)
        }
        ProbeCase::SurfaceListing { max_pages, .. } => {
            run_surface_listing(case, *max_pages, context)
        }
        ProbeCase::OutputSchema { .. } => run_output_schema(case, context),
    }
}

fn run_contention(case: &ProbeCase, context: &mut RunContext<'_>) -> anyhow::Result<CaseReport> {
    let tool = case.tool().expect("contention has a tool").to_owned();
    let arguments = case.arguments().expect("contention has arguments").clone();
    let target = context.target.clone();
    let barrier = Arc::new(Barrier::new(2));
    let worker_barrier = Arc::clone(&barrier);
    let (ready_tx, ready_rx) = mpsc::sync_channel(0);
    let worker_tool = tool.clone();
    let worker_arguments = arguments.clone();
    let worker = std::thread::spawn(move || -> anyhow::Result<(ToolResponse, u64)> {
        let mut client = target.connect()?;
        client.initialize()?;
        let tools = client.list_tools()?;
        if !tools.iter().any(|name| name == &worker_tool) {
            bail!("contended client is missing the probe tool");
        }
        ready_tx
            .send(())
            .map_err(|_| anyhow::anyhow!("contention coordinator closed"))?;
        worker_barrier.wait();
        let started = Instant::now();
        let response = client.call_tool(&worker_tool, &worker_arguments)?;
        Ok((response, started.elapsed().as_millis() as u64))
    });
    ready_rx
        .recv_timeout(std::time::Duration::from_secs(30))
        .map_err(|_| anyhow::anyhow!("contended client failed to initialize"))?;
    barrier.wait();
    let primary = call_named_and_record(&tool, &arguments, context)?;
    let (secondary, latency_ms) = worker
        .join()
        .map_err(|_| anyhow::anyhow!("contended client terminated unexpectedly"))??;
    record_response(&tool, &arguments, latency_ms, &secondary, context)?;
    if matches!(primary, ToolResponse::Success(_)) && matches!(secondary, ToolResponse::Success(_))
    {
        Ok(passed_case(case, 2))
    } else {
        Ok(failed_case(case, 2, FailureReason::ContendedClientFailed))
    }
}

fn run_error_honesty(
    case: &ProbeCase,
    max_attempts: u64,
    expect_retryable: bool,
    context: &mut RunContext<'_>,
) -> anyhow::Result<CaseReport> {
    let mut first_code = None;
    for attempt in 1..=max_attempts {
        match call_and_record(case, context)? {
            ToolResponse::Success(_) if attempt == 1 => {
                return Ok(failed_case(case, attempt, FailureReason::ExpectedError));
            }
            ToolResponse::Success(_) if expect_retryable => return Ok(passed_case(case, attempt)),
            ToolResponse::Success(_) => {
                return Ok(failed_case(case, attempt, FailureReason::UnexpectedOutcome));
            }
            ToolResponse::Error { code, payload } => {
                if first_code.is_some_and(|first| first != code) {
                    return Ok(failed_case(case, attempt, FailureReason::UnstableErrorCode));
                }
                first_code.get_or_insert(code);
                let retryable = payload
                    .get("retryable")
                    .and_then(serde_json::Value::as_bool);
                if retryable != Some(expect_retryable) {
                    return Ok(failed_case(
                        case,
                        attempt,
                        FailureReason::RetryabilityMismatch,
                    ));
                }
                if !expect_retryable && attempt == 2 {
                    return Ok(passed_case(case, attempt));
                }
            }
        }
    }
    Ok(failed_case(
        case,
        max_attempts,
        FailureReason::RetryDidNotRecover,
    ))
}

struct RecoveryPlan<'a> {
    failure_tool: &'a str,
    failure_arguments: &'a serde_json::Value,
    recovery_tool: &'a str,
    recovery_arguments: &'a serde_json::Value,
    validation_tool: &'a str,
    validation_arguments: &'a serde_json::Value,
}

fn run_state_recovery(
    case: &ProbeCase,
    plan: RecoveryPlan<'_>,
    context: &mut RunContext<'_>,
) -> anyhow::Result<CaseReport> {
    if matches!(
        call_named_and_record(plan.failure_tool, plan.failure_arguments, context)?,
        ToolResponse::Success(_)
    ) {
        return Ok(failed_case(case, 1, FailureReason::FailureNotObserved));
    }
    if matches!(
        call_named_and_record(plan.recovery_tool, plan.recovery_arguments, context)?,
        ToolResponse::Error { .. }
    ) {
        return Ok(failed_case(case, 2, FailureReason::RecoveryFailed));
    }
    if matches!(
        call_named_and_record(plan.validation_tool, plan.validation_arguments, context)?,
        ToolResponse::Error { .. }
    ) {
        return Ok(failed_case(case, 3, FailureReason::ValidationFailed));
    }
    Ok(passed_case(case, 3))
}

fn run_latency_budget(
    case: &ProbeCase,
    max_latency_ms: u64,
    context: &mut RunContext<'_>,
) -> anyhow::Result<CaseReport> {
    let attempts = case
        .max_attempts()
        .expect("latency case has an attempt bound");
    let mut slowest_ms = 0;
    for attempt in 1..=attempts {
        let (response, latency_ms) = call_timed_and_record(case, context)?;
        slowest_ms = slowest_ms.max(latency_ms);
        if matches!(response, ToolResponse::Error { .. }) {
            return Ok(CaseReport {
                id: case.id().to_owned(),
                probe: case.kind(),
                attempts: attempt,
                first_failure: Some(attempt),
                reason: Some(FailureReason::UnexpectedOutcome),
                tool_count: None,
                schema_bytes: None,
                token_usage: None,
                latency_ms: Some(slowest_ms),
                pages: None,
            });
        }
        if latency_ms > max_latency_ms {
            return Ok(CaseReport {
                id: case.id().to_owned(),
                probe: case.kind(),
                attempts: attempt,
                first_failure: Some(attempt),
                reason: Some(FailureReason::LatencyBudgetExceeded),
                tool_count: None,
                schema_bytes: None,
                token_usage: None,
                latency_ms: Some(slowest_ms),
                pages: None,
            });
        }
    }
    Ok(CaseReport {
        id: case.id().to_owned(),
        probe: case.kind(),
        attempts,
        first_failure: None,
        reason: None,
        tool_count: None,
        schema_bytes: None,
        token_usage: None,
        latency_ms: Some(slowest_ms),
        pages: None,
    })
}

fn run_pagination(
    case: &ProbeCase,
    max_pages: u64,
    context: &mut RunContext<'_>,
) -> anyhow::Result<CaseReport> {
    let mut seen: Vec<String> = Vec::new();
    let mut cursor: Option<String> = None;
    let mut pages = 0;
    loop {
        if pages >= max_pages {
            return Ok(CaseReport {
                id: case.id().to_owned(),
                probe: case.kind(),
                attempts: pages + 1,
                first_failure: Some(pages + 1),
                reason: Some(FailureReason::PaginationStalledCursor),
                tool_count: Some(seen.len() as u64),
                schema_bytes: None,
                token_usage: None,
                latency_ms: None,
                pages: Some(pages + 1),
            });
        }
        pages += 1;
        let mut params = serde_json::Map::new();
        if let Some(value) = &cursor {
            params.insert("cursor".into(), Value::String(value.clone()));
        }
        let response = context
            .client
            .raw_request("tools/list", Value::Object(params))?;
        let Some(entries) = response
            .get("result")
            .and_then(|result| result.get("tools"))
            .and_then(Value::as_array)
        else {
            return Ok(failed_case(
                case,
                pages,
                FailureReason::PaginationInvalidEntry,
            ));
        };
        for entry in entries {
            let valid = entry
                .get("name")
                .and_then(Value::as_str)
                .is_some_and(|name| {
                    crate::privacy::valid_tool(name)
                        && entry
                            .get("inputSchema")
                            .is_some_and(serde_json::Value::is_object)
                });
            if !valid {
                return Ok(failed_case(
                    case,
                    pages,
                    FailureReason::PaginationInvalidEntry,
                ));
            }
            let name = entry["name"].as_str().expect("validated above").to_owned();
            if seen.contains(&name) {
                return Ok(CaseReport {
                    id: case.id().to_owned(),
                    probe: case.kind(),
                    attempts: pages,
                    first_failure: Some(pages),
                    reason: Some(FailureReason::PaginationDuplicateTool),
                    tool_count: Some(seen.len() as u64),
                    schema_bytes: None,
                    token_usage: None,
                    latency_ms: None,
                    pages: Some(pages),
                });
            }
            seen.push(name);
        }
        cursor = response
            .get("result")
            .and_then(|result| result.get("nextCursor"))
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .map(str::to_owned);
        if cursor.is_none() {
            break;
        }
    }
    Ok(CaseReport {
        id: case.id().to_owned(),
        probe: case.kind(),
        attempts: pages,
        first_failure: None,
        reason: None,
        tool_count: Some(seen.len() as u64),
        schema_bytes: None,
        token_usage: None,
        latency_ms: None,
        pages: Some(pages),
    })
}

fn passed_case(case: &ProbeCase, attempts: u64) -> CaseReport {
    CaseReport {
        id: case.id().to_owned(),
        probe: case.kind(),
        attempts,
        first_failure: None,
        reason: None,
        tool_count: None,
        schema_bytes: None,
        token_usage: None,
        latency_ms: None,
        pages: None,
    }
}

fn failed_case(case: &ProbeCase, attempt: u64, reason: FailureReason) -> CaseReport {
    CaseReport {
        id: case.id().to_owned(),
        probe: case.kind(),
        attempts: attempt,
        first_failure: Some(attempt),
        reason: Some(reason),
        tool_count: None,
        schema_bytes: None,
        token_usage: None,
        latency_ms: None,
        pages: None,
    }
}

fn call_and_record(case: &ProbeCase, context: &mut RunContext<'_>) -> anyhow::Result<ToolResponse> {
    Ok(call_timed_and_record(case, context)?.0)
}

fn call_timed_and_record(
    case: &ProbeCase,
    context: &mut RunContext<'_>,
) -> anyhow::Result<(ToolResponse, u64)> {
    let tool = case.tool().expect("call probe has a tool");
    let arguments = case.arguments().expect("call probe has arguments");
    let started = Instant::now();
    let response = context.client.call_tool(tool, arguments)?;
    let latency_ms = started.elapsed().as_millis() as u64;
    record_response(tool, arguments, latency_ms, &response, context)?;
    Ok((response, latency_ms))
}

fn call_named_and_record(
    tool: &str,
    arguments: &serde_json::Value,
    context: &mut RunContext<'_>,
) -> anyhow::Result<ToolResponse> {
    let started = Instant::now();
    let response = context.client.call_tool(tool, arguments)?;
    let latency_ms = started.elapsed().as_millis() as u64;
    record_response(tool, arguments, latency_ms, &response, context)?;
    Ok(response)
}

fn record_response(
    tool: &str,
    arguments: &serde_json::Value,
    latency_ms: u64,
    response: &ToolResponse,
    context: &mut RunContext<'_>,
) -> anyhow::Result<()> {
    *context.seq += 1;
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
    Ok(())
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

fn run_payload_bounds(
    case: &ProbeCase,
    field: &str,
    size_bytes: u64,
    expect_handled: bool,
    context: &mut RunContext<'_>,
) -> anyhow::Result<CaseReport> {
    // Inject one exact-size ASCII string into a deep copy of the declared
    // arguments. ASCII 'a' keeps the encoded size equal to the character
    // count, so the measurement is exact and deterministic.
    let mut arguments = case
        .arguments()
        .expect("payload case has arguments")
        .clone();
    let object = arguments
        .as_object_mut()
        .expect("manifest validates arguments as an object");
    object.insert(
        field.to_owned(),
        Value::String("a".repeat(size_bytes as usize)),
    );
    let started = Instant::now();
    let outcome = context
        .client
        .call_tool(case.tool().expect("payload case has a tool"), &arguments);
    let latency_ms = started.elapsed().as_millis() as u64;
    let (response, reason) = match outcome {
        // The transport died: crash, hang, or non-JSON output under load.
        // That is a robustness failure regardless of expect_handled.
        Err(_) => (
            ToolResponse::Error {
                code: -32603,
                payload: json!({}),
            },
            Some(FailureReason::PayloadUnhandled),
        ),
        Ok(response) => {
            let handled_cleanly = match &response {
                // Accepted and answered: the strongest pass.
                ToolResponse::Success(_) => Some(None),
                // A structured JSON-RPC error is honest bounded behavior;
                // it only fails the case when the operator asserted the
                // tool must actually handle this size.
                ToolResponse::Error { .. } if !expect_handled => Some(None),
                ToolResponse::Error { .. } => Some(Some(FailureReason::UnexpectedOutcome)),
            };
            match handled_cleanly {
                Some(reason) => (response, reason),
                None => unreachable!("both ToolResponse arms covered"),
            }
        }
    };
    let attempts = 1;
    if let Some(reason) = reason {
        return Ok(CaseReport {
            id: case.id().to_owned(),
            probe: case.kind(),
            attempts,
            first_failure: Some(attempts),
            reason: Some(reason),
            tool_count: None,
            schema_bytes: None,
            token_usage: None,
            latency_ms: Some(latency_ms),
            pages: None,
        });
    }
    record_response(
        case.tool().expect("payload case has a tool"),
        &arguments,
        latency_ms,
        &response,
        context,
    )?;
    Ok(CaseReport {
        id: case.id().to_owned(),
        probe: case.kind(),
        attempts,
        first_failure: None,
        reason: None,
        tool_count: None,
        schema_bytes: None,
        token_usage: None,
        latency_ms: Some(latency_ms),
        pages: None,
    })
}

/// Cursor-driven traversal of one optional MCP surface (`resources/list`
/// or `prompts/list`). Surfaces the server did not declare pass trivially:
/// the probe only validates what the server claims to offer.
fn run_surface_listing(
    case: &ProbeCase,
    max_pages: u64,
    context: &mut RunContext<'_>,
) -> anyhow::Result<CaseReport> {
    let surfaces: [(&str, &str); 2] =
        [("resources", "resources/list"), ("prompts", "prompts/list")];
    let mut total_items = 0u64;
    for (capability, method) in surfaces {
        let declared = context
            .client
            .capabilities()
            .is_some_and(|value| value.get(capability).is_some());
        if !declared {
            continue;
        }
        let mut cursor: Option<String> = None;
        let mut pages = 0u64;
        loop {
            if pages >= max_pages {
                return Ok(CaseReport {
                    id: case.id().to_owned(),
                    probe: case.kind(),
                    attempts: pages + 1,
                    first_failure: Some(pages + 1),
                    reason: Some(FailureReason::SurfaceStalledCursor),
                    tool_count: None,
                    schema_bytes: None,
                    token_usage: None,
                    latency_ms: None,
                    pages: Some(pages + 1),
                });
            }
            pages += 1;
            let mut params = serde_json::Map::new();
            if let Some(value) = &cursor {
                params.insert("cursor".into(), Value::String(value.clone()));
            }
            let response = match context
                .client
                .raw_request(method, Value::Object(params.clone()))
            {
                Ok(response) => response,
                // A declared surface that errors on listing is a defect.
                Err(_) => {
                    return Ok(failed_case(
                        case,
                        pages,
                        FailureReason::SurfaceInvalidEnvelope,
                    ))
                }
            };
            let Some(result) = response.get("result") else {
                return Ok(failed_case(
                    case,
                    pages,
                    FailureReason::SurfaceInvalidEnvelope,
                ));
            };
            let items_key = if method == "resources/list" {
                "resources"
            } else {
                "prompts"
            };
            match result.get(items_key).and_then(Value::as_array) {
                Some(items) => total_items += items.len() as u64,
                None => {
                    return Ok(failed_case(
                        case,
                        pages,
                        FailureReason::SurfaceInvalidEnvelope,
                    ))
                }
            }
            cursor = result
                .get("nextCursor")
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
                .map(str::to_owned);
            if cursor.is_none() {
                break;
            }
        }
    }
    Ok(CaseReport {
        id: case.id().to_owned(),
        probe: case.kind(),
        attempts: 1,
        first_failure: None,
        reason: None,
        tool_count: Some(total_items),
        schema_bytes: None,
        token_usage: None,
        latency_ms: None,
        pages: None,
    })
}

/// For a tool that declares `outputSchema`, the response must carry
/// `structuredContent` whose required fields (per that schema) are present.
fn run_output_schema(case: &ProbeCase, context: &mut RunContext<'_>) -> anyhow::Result<CaseReport> {
    let tool_name = case.tool().expect("output case has a tool").to_owned();
    let definition = context
        .catalog
        .tools
        .iter()
        .find(|tool| tool.name == tool_name)
        .expect("selected tool was preflighted");
    let Some(output_schema) = definition.declared_output_schema() else {
        // The tool does not declare an output schema: nothing to verify.
        return Ok(passed_case(case, 0));
    };
    let required: Vec<String> = output_schema
        .get("required")
        .and_then(Value::as_array)
        .map(|fields| {
            fields
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default();
    let response = call_and_record(case, context)?;
    let structured = match &response {
        ToolResponse::Success(result) => result.get("structuredContent").cloned(),
        ToolResponse::Error { .. } => None,
    };
    let Some(structured) = structured else {
        return Ok(CaseReport {
            id: case.id().to_owned(),
            probe: case.kind(),
            attempts: 1,
            first_failure: Some(1),
            reason: Some(FailureReason::OutputSchemaDeclaredButMissing),
            tool_count: None,
            schema_bytes: None,
            token_usage: None,
            latency_ms: None,
            pages: None,
        });
    };
    let missing = required.iter().any(|field| structured.get(field).is_none());
    Ok(CaseReport {
        id: case.id().to_owned(),
        probe: case.kind(),
        attempts: 1,
        first_failure: missing.then_some(1),
        reason: missing.then_some(FailureReason::OutputSchemaFieldMissing),
        tool_count: None,
        schema_bytes: None,
        token_usage: None,
        latency_ms: None,
        pages: None,
    })
}
