mod report;
mod transport;
pub use report::{
    estimate_tokens, BoundDetail, CaseReport, ProbeReport, TokenUsage, ToolTokenUsage,
    CHARS_PER_TOKEN,
};
pub(crate) use transport::{ClientTarget, ProbeClient};

use std::io::Write;
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use crate::fingerprint::Salt;
use crate::manifest::{
    Access, Expectation, Manifest, OutcomeExpectation, ProbeCase, ProbeKind, WorkflowStep,
};
use crate::mcp_client::{ToolCatalog, ToolDefinition, ToolResponse, TransportFailure};
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
    /// Run the standard battery after the gate (full-battery runs only).
    pub standard: bool,
    /// Attest unannotated tools as read-only for the standard battery.
    pub confirm_read_only: bool,
    /// Tools the standard battery never calls (`--skip-tool`).
    pub skip_tools: Vec<String>,
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
    OutputSchemaInvalidResult,
    CancellationIgnored,
    CancellationErrored,
    NegotiationEchoedUnknown,
    NegotiationInvalidVersion,
    NegotiationInconsistentSupport,
    SamplingInvalidRequest,
    SamplingRequestFlood,
    SamplingStalledCall,
    ElicitationInvalidRequest,
    ElicitationRequestFlood,
    ElicitationStalledCall,
    ResourceUnreadable,
    SubscriptionRejected,
    SubscriptionNotificationMissing,
    CompletionInvalidRequest,
    CompletionValueFlood,
    CompletionArgumentUnknown,
    CompletionStalledRequest,
    /// The case could not be evaluated: no response within the timeout.
    TransportTimeout,
    /// The case could not be evaluated: the server closed the connection.
    TransportClosed,
    /// The case could not be evaluated: any other failure while it ran,
    /// such as a malformed or mismatched response.
    TransportError,
    /// Shared evaluation deadline or request count exhausted; no verdict.
    EvaluationBudgetExceeded,
}

impl ProbeKind {
    pub fn from_report_label(label: &str) -> Option<Self> {
        let candidate = match label {
            "workflow" => Self::Workflow,
            "contention" => Self::Contention,
            "error-honesty" => Self::ErrorHonesty,
            "state-recovery" => Self::StateRecovery,
            "discovery-cost" => Self::DiscoveryCost,
            "token-cost" => Self::TokenCost,
            "schema-guessability" => Self::SchemaGuessability,
            "degradation-over-n" => Self::DegradationOverN,
            "instruction-fidelity" => Self::InstructionFidelity,
            "latency-budget" => Self::LatencyBudget,
            "pagination" => Self::Pagination,
            "payload-bounds" => Self::PayloadBounds,
            "surface-listing" => Self::SurfaceListing,
            "output-schema" => Self::OutputSchema,
            "cancellation" => Self::Cancellation,
            "protocol-negotiation" => Self::ProtocolNegotiation,
            "sampling" => Self::Sampling,
            "elicitation" => Self::Elicitation,
            "resource-subscription" => Self::ResourceSubscription,
            "completion" => Self::Completion,
            _ => return None,
        };
        Some(candidate)
    }
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
        Self::OutputSchemaInvalidResult,
        Self::CancellationIgnored,
        Self::CancellationErrored,
        Self::NegotiationEchoedUnknown,
        Self::NegotiationInvalidVersion,
        Self::NegotiationInconsistentSupport,
        Self::SamplingInvalidRequest,
        Self::SamplingRequestFlood,
        Self::SamplingStalledCall,
        Self::ElicitationInvalidRequest,
        Self::ElicitationRequestFlood,
        Self::ElicitationStalledCall,
        Self::ResourceUnreadable,
        Self::SubscriptionRejected,
        Self::SubscriptionNotificationMissing,
        Self::CompletionInvalidRequest,
        Self::CompletionValueFlood,
        Self::CompletionArgumentUnknown,
        Self::CompletionStalledRequest,
        Self::TransportTimeout,
        Self::TransportClosed,
        Self::TransportError,
        Self::EvaluationBudgetExceeded,
    ];

    pub fn from_report_label(label: &str) -> Option<Self> {
        Some(match label {
            "unexpected-outcome" => Self::UnexpectedOutcome,
            "missing-field" => Self::MissingField,
            "value-mismatch" => Self::ValueMismatch,
            "error-code-mismatch" => Self::ErrorCodeMismatch,
            "discovery-limit-exceeded" => Self::DiscoveryLimitExceeded,
            "token-budget-exceeded" => Self::TokenBudgetExceeded,
            "invalid-schema" => Self::InvalidSchema,
            "missing-required-argument" => Self::MissingRequiredArgument,
            "expected-error" => Self::ExpectedError,
            "unstable-error-code" => Self::UnstableErrorCode,
            "retryability-mismatch" => Self::RetryabilityMismatch,
            "retry-did-not-recover" => Self::RetryDidNotRecover,
            "failure-not-observed" => Self::FailureNotObserved,
            "recovery-failed" => Self::RecoveryFailed,
            "validation-failed" => Self::ValidationFailed,
            "contended-client-failed" => Self::ContendedClientFailed,
            "latency-budget-exceeded" => Self::LatencyBudgetExceeded,
            "pagination-invalid-entry" => Self::PaginationInvalidEntry,
            "pagination-duplicate-tool" => Self::PaginationDuplicateTool,
            "pagination-stalled-cursor" => Self::PaginationStalledCursor,
            "payload-unhandled" => Self::PayloadUnhandled,
            "surface-invalid-envelope" => Self::SurfaceInvalidEnvelope,
            "surface-stalled-cursor" => Self::SurfaceStalledCursor,
            "output-schema-declared-but-missing" => Self::OutputSchemaDeclaredButMissing,
            "output-schema-field-missing" => Self::OutputSchemaFieldMissing,
            "output-schema-invalid-result" => Self::OutputSchemaInvalidResult,
            "cancellation-ignored" => Self::CancellationIgnored,
            "cancellation-errored" => Self::CancellationErrored,
            "negotiation-echoed-unknown" => Self::NegotiationEchoedUnknown,
            "negotiation-invalid-version" => Self::NegotiationInvalidVersion,
            "negotiation-inconsistent-support" => Self::NegotiationInconsistentSupport,
            "sampling-invalid-request" => Self::SamplingInvalidRequest,
            "sampling-request-flood" => Self::SamplingRequestFlood,
            "sampling-stalled-call" => Self::SamplingStalledCall,
            "elicitation-invalid-request" => Self::ElicitationInvalidRequest,
            "elicitation-request-flood" => Self::ElicitationRequestFlood,
            "elicitation-stalled-call" => Self::ElicitationStalledCall,
            "resource-unreadable" => Self::ResourceUnreadable,
            "subscription-rejected" => Self::SubscriptionRejected,
            "subscription-notification-missing" => Self::SubscriptionNotificationMissing,
            "completion-invalid-request" => Self::CompletionInvalidRequest,
            "completion-value-flood" => Self::CompletionValueFlood,
            "completion-argument-unknown" => Self::CompletionArgumentUnknown,
            "completion-stalled-request" => Self::CompletionStalledRequest,
            "transport-timeout" => Self::TransportTimeout,
            "transport-closed" => Self::TransportClosed,
            "transport-error" => Self::TransportError,
            "evaluation-budget-exceeded" => Self::EvaluationBudgetExceeded,
            _ => return None,
        })
    }

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
            Self::OutputSchemaInvalidResult => "output-schema-invalid-result",
            Self::CancellationIgnored => "cancellation-ignored",
            Self::CancellationErrored => "cancellation-errored",
            Self::NegotiationEchoedUnknown => "negotiation-echoed-unknown",
            Self::NegotiationInvalidVersion => "negotiation-invalid-version",
            Self::NegotiationInconsistentSupport => "negotiation-inconsistent-support",
            Self::SamplingInvalidRequest => "sampling-invalid-request",
            Self::SamplingRequestFlood => "sampling-request-flood",
            Self::SamplingStalledCall => "sampling-stalled-call",
            Self::ElicitationInvalidRequest => "elicitation-invalid-request",
            Self::ElicitationRequestFlood => "elicitation-request-flood",
            Self::ElicitationStalledCall => "elicitation-stalled-call",
            Self::ResourceUnreadable => "resource-unreadable",
            Self::SubscriptionRejected => "subscription-rejected",
            Self::SubscriptionNotificationMissing => "subscription-notification-missing",
            Self::CompletionInvalidRequest => "completion-invalid-request",
            Self::CompletionValueFlood => "completion-value-flood",
            Self::CompletionArgumentUnknown => "completion-argument-unknown",
            Self::CompletionStalledRequest => "completion-stalled-request",
            Self::TransportTimeout => "transport-timeout",
            Self::TransportClosed => "transport-closed",
            Self::TransportError => "transport-error",
            Self::EvaluationBudgetExceeded => "evaluation-budget-exceeded",
        }
    }

    /// Transport and evaluation-budget reasons mean the case could not
    /// finish; every other reason is a verdict on the server.
    pub fn is_transport(&self) -> bool {
        matches!(
            self,
            Self::TransportTimeout
                | Self::TransportClosed
                | Self::TransportError
                | Self::EvaluationBudgetExceeded
        )
    }
}

pub fn run(options: ProbeOptions, store: &mut Store) -> anyhow::Result<ProbeReport> {
    let (manifest, manifest_sha256) = load_manifest(&options).map_err(crate::exit::usage)?;
    let cases = select_cases(&manifest, &options).map_err(crate::exit::usage)?;
    let target = ClientTarget::from_options(&options).map_err(crate::exit::usage)?;

    let salt = Salt::load(store.root())?;
    let session = uuid::Uuid::new_v4().to_string();
    let mut seq = 0;
    let timeout = manifest.timeout_ms.map(Duration::from_millis);
    let mut client = target.connect(timeout)?;
    client.initialize()?;
    let discovery = client.discover_tools_catalog()?;
    let catalog = discovery.catalog;
    let discovery_error = discovery.incomplete.as_ref().map(transport_reason);
    if let Some(error) = discovery.incomplete.as_ref() {
        let _ = writeln!(std::io::stderr(), "listing tools failed: {error:#}");
    }
    for case in &cases {
        if discovery_error.is_some() {
            continue;
        }
        validate_workflow_tools(case, &catalog).map_err(crate::exit::usage)?;
        if case
            .required_tools()
            .iter()
            .any(|name| !catalog.tools.iter().any(|tool| tool.name == **name))
        {
            return Err(crate::exit::usage(anyhow::anyhow!(
                "probe tool was not declared by the server"
            )));
        }
    }
    let mut reports = Vec::with_capacity(cases.len());
    let mut context = RunContext {
        journal: Some(Journal {
            capture_id: uuid::Uuid::new_v4(),
            server: &options.server,
            session: &session,
            seq: &mut seq,
            salt: &salt,
            store,
        }),
        client: &mut client,
        catalog: &catalog,
        target: &target,
        timeout,
    };
    // A failure inside one case costs that case, never the run: it is
    // reported as errored and the next case starts on a fresh connection,
    // because the session state after a lost or broken exchange is unknown.
    // Once the server cannot be reached again, the remaining cases error
    // with the same reason.
    let mut unreachable = None;
    for case in cases {
        if let Some(reason) = discovery_error {
            if case.kind() != ProbeKind::Pagination {
                reports.push(errored_case(case, reason));
                continue;
            }
        }
        if let Some(reason) = unreachable {
            reports.push(errored_case(case, reason));
            continue;
        }
        let case_timeout = case_timeout(case, timeout, context.client.default_timeout());
        context.client.set_response_timeout(case_timeout);
        match run_case(case, &mut context) {
            Ok(mut report) => {
                report.detail = bound_detail(case, &report);
                reports.push(report);
            }
            Err(error) => {
                let reason = transport_reason(&error);
                // Diagnostics only: a closed stderr must not end the run.
                let _ = writeln!(
                    std::io::stderr(),
                    "{} {}: {error:#}",
                    case.id(),
                    reason.as_str()
                );
                reports.push(errored_case(case, reason));
                match reconnect(&target, timeout) {
                    Ok(fresh) => *context.client = fresh,
                    Err(error) => {
                        let reason = transport_reason(&error);
                        let _ = writeln!(std::io::stderr(), "reconnecting failed: {error:#}");
                        unreachable = Some(reason);
                    }
                }
            }
        }
    }
    // The gate's connection closes before the standard opens its own, so
    // the manifest's calls cannot move the score.
    drop(client);
    let mut report = ProbeReport {
        cases: reports,
        manifest_sha256: Some(manifest_sha256),
        ..ProbeReport::default()
    };
    if options.standard && options.selected_probe.is_none() && options.selected_case.is_none() {
        measure_standard(
            &mut report,
            &target,
            &crate::standard::StandardOptions {
                confirm_read_only: options.confirm_read_only,
                skip_tools: options.skip_tools,
            },
        )?;
    }
    Ok(report)
}

pub struct ScoreOptions {
    pub server: String,
    pub command: Vec<String>,
    pub http_url: Option<String>,
    pub allow_remote_http: bool,
    pub confirm_read_only: bool,
    pub skip_tools: Vec<String>,
}

/// The standard battery alone: no manifest, no gate.
pub fn score(options: ScoreOptions) -> anyhow::Result<ProbeReport> {
    if !crate::privacy::valid_server(&options.server) {
        return Err(crate::exit::usage(anyhow::anyhow!(
            "server label is invalid"
        )));
    }
    let target = ClientTarget::new(options.command, options.http_url, options.allow_remote_http)
        .map_err(crate::exit::usage)?;
    let mut report = ProbeReport::default();
    measure_standard(
        &mut report,
        &target,
        &crate::standard::StandardOptions {
            confirm_read_only: options.confirm_read_only,
            skip_tools: options.skip_tools,
        },
    )?;
    Ok(report)
}

/// Run the standard battery into `report`; a battery that cannot finish
/// leaves readiness unmeasured with its transport reason. A usage error
/// (an unknown `--skip-tool`) ends the run.
fn measure_standard(
    report: &mut ProbeReport,
    target: &ClientTarget,
    options: &crate::standard::StandardOptions,
) -> anyhow::Result<()> {
    match crate::standard::run(target, options) {
        Ok(readiness) => report.readiness = Some(readiness),
        Err(error) if crate::exit::code(&error) == crate::exit::USAGE => return Err(error),
        Err(error) => {
            let reason = transport_reason(&error);
            // Diagnostics only: a closed stderr must not end the run.
            let _ = writeln!(
                std::io::stderr(),
                "standard battery {}: {error:#}",
                reason.as_str()
            );
            report.readiness_error = Some(reason);
        }
    }
    Ok(())
}

/// The validated manifest and the SHA-256 of the exact bytes it was parsed
/// from.
fn load_manifest(options: &ProbeOptions) -> anyhow::Result<(Manifest, String)> {
    if !crate::privacy::valid_server(&options.server) {
        bail!("server label is invalid");
    }
    match &options.manifest_inline {
        Some(body) => {
            let manifest: Manifest =
                serde_json::from_str(body).context("parsing inline manifest structure")?;
            manifest.validate()?;
            Ok((manifest, sha256_hex(body.as_bytes())))
        }
        None => {
            let body = Manifest::read(&options.manifest_path)?;
            Ok((Manifest::parse(&body)?, sha256_hex(&body)))
        }
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn select_cases<'m>(
    manifest: &'m Manifest,
    options: &ProbeOptions,
) -> anyhow::Result<Vec<&'m ProbeCase>> {
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
    Ok(cases)
}

/// The manifest bound behind a bound-based failure: the case's declared
/// limit beside the measurement the report already carries.
fn bound_detail(case: &ProbeCase, report: &CaseReport) -> Option<BoundDetail> {
    let detail = |bound: &'static str, limit: u64, observed: u64| {
        Some(BoundDetail {
            bound,
            limit,
            observed,
        })
    };
    match (case, report.reason?) {
        (
            ProbeCase::DiscoveryCost {
                max_tools,
                max_schema_bytes,
                ..
            },
            FailureReason::DiscoveryLimitExceeded,
        ) => {
            let tools = report.tool_count?;
            if tools > *max_tools {
                detail("max_tools", *max_tools, tools)
            } else {
                detail("max_schema_bytes", *max_schema_bytes, report.schema_bytes?)
            }
        }
        (
            ProbeCase::TokenCost {
                max_total_tokens,
                max_tool_tokens,
                ..
            },
            FailureReason::TokenBudgetExceeded,
        ) => {
            let usage = report.token_usage.as_ref()?;
            if usage.total_tokens > *max_total_tokens {
                detail("max_total_tokens", *max_total_tokens, usage.total_tokens)
            } else {
                detail(
                    "max_tool_tokens",
                    (*max_tool_tokens)?,
                    usage.per_tool.first()?.tokens,
                )
            }
        }
        (ProbeCase::LatencyBudget { max_latency_ms, .. }, FailureReason::LatencyBudgetExceeded) => {
            detail("max_latency_ms", *max_latency_ms, report.latency_ms?)
        }
        (ProbeCase::Pagination { max_pages, .. }, FailureReason::PaginationStalledCursor)
        | (ProbeCase::SurfaceListing { max_pages, .. }, FailureReason::SurfaceStalledCursor) => {
            detail("max_pages", *max_pages, report.pages?)
        }
        _ => None,
    }
}

fn reconnect(target: &ClientTarget, timeout: Option<Duration>) -> anyhow::Result<ProbeClient> {
    let mut client = target.connect(timeout)?;
    client.initialize()?;
    Ok(client)
}

/// A latency budget must never be cut short by the transport timeout: its
/// calls wait the budget plus the timeout, so a slow call is measured and
/// judged against the budget instead of erroring.
fn case_timeout(
    case: &ProbeCase,
    timeout: Option<Duration>,
    transport_default: Duration,
) -> Option<Duration> {
    match case {
        ProbeCase::LatencyBudget { max_latency_ms, .. } => {
            Some(timeout.unwrap_or(transport_default) + Duration::from_millis(*max_latency_ms))
        }
        _ => timeout,
    }
}

pub(crate) fn transport_reason(error: &anyhow::Error) -> FailureReason {
    if error
        .downcast_ref::<crate::evaluation_budget::Exhausted>()
        .is_some()
    {
        return FailureReason::EvaluationBudgetExceeded;
    }
    match TransportFailure::of(error) {
        Some(TransportFailure::Timeout) => FailureReason::TransportTimeout,
        Some(TransportFailure::Closed) => FailureReason::TransportClosed,
        None => FailureReason::TransportError,
    }
}

/// A case that could not be evaluated: no attempt completed.
fn errored_case(case: &ProbeCase, reason: FailureReason) -> CaseReport {
    CaseReport {
        reason: Some(reason),
        ..CaseReport::for_case(case)
    }
}

/// Where a run's calls are journaled. The standard battery runs without
/// one: its deliberate calls must never become promoted findings.
struct Journal<'a> {
    capture_id: uuid::Uuid,
    server: &'a str,
    session: &'a str,
    seq: &'a mut u64,
    salt: &'a Salt,
    store: &'a mut Store,
}

struct RunContext<'a> {
    journal: Option<Journal<'a>>,
    client: &'a mut ProbeClient,
    catalog: &'a ToolCatalog,
    target: &'a ClientTarget,
    /// The manifest's response timeout, for connections a case opens.
    timeout: Option<Duration>,
}

/// Run one synthesized case without journaling its calls: the standard
/// battery reuses the contention, payload, and protocol runners this way.
pub(crate) fn run_unjournaled(
    case: &ProbeCase,
    client: &mut ProbeClient,
    catalog: &ToolCatalog,
    target: &ClientTarget,
    timeout: Option<Duration>,
) -> anyhow::Result<CaseReport> {
    run_case(
        case,
        &mut RunContext {
            journal: None,
            client,
            catalog,
            target,
            timeout,
        },
    )
}

fn run_case(case: &ProbeCase, context: &mut RunContext<'_>) -> anyhow::Result<CaseReport> {
    context.target.budget.check()?;
    let result = run_case_inner(case, context);
    context.target.budget.finish(result)
}

fn run_case_inner(case: &ProbeCase, context: &mut RunContext<'_>) -> anyhow::Result<CaseReport> {
    match case {
        ProbeCase::Workflow {
            repetitions, steps, ..
        } => run_workflow(case, *repetitions, steps, context),
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
                attempts: 1,
                first_failure: reason.map(|_| 1),
                reason,
                tool_count: Some(tool_count),
                schema_bytes: Some(schema_bytes),
                ..CaseReport::for_case(case)
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
                attempts: 1,
                first_failure: reason.map(|_| 1),
                reason,
                tool_count: Some(context.catalog.tools.len() as u64),
                token_usage: Some(TokenUsage {
                    total_tokens,
                    per_tool,
                }),
                ..CaseReport::for_case(case)
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
                attempts: u64::from(preflight.is_none()),
                first_failure: reason.map(|_| 1),
                reason,
                ..CaseReport::for_case(case)
            })
        }
        ProbeCase::DegradationOverN { .. } => {
            let limit = case.max_attempts().expect("degradation case has a bound");
            for attempt in 1..=limit {
                let response = call_and_record(case, context)?;
                if matches!(response, ToolResponse::Error { .. }) {
                    return Ok(CaseReport {
                        attempts: attempt,
                        first_failure: Some(attempt),
                        reason: Some(FailureReason::UnexpectedOutcome),
                        ..CaseReport::for_case(case)
                    });
                }
            }
            Ok(CaseReport {
                attempts: limit,
                first_failure: None,
                reason: None,
                ..CaseReport::for_case(case)
            })
        }
        ProbeCase::InstructionFidelity { .. } => {
            let response = call_and_record(case, context)?;
            let reason = check_expectation(
                case.expectation().expect("fidelity case has expectation"),
                &response,
            );
            Ok(CaseReport {
                attempts: 1,
                first_failure: reason.map(|_| 1),
                reason,
                ..CaseReport::for_case(case)
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
        ProbeCase::Cancellation { .. } => run_cancellation(case, context),
        ProbeCase::ProtocolNegotiation { .. } => run_protocol_negotiation(case, context),
        ProbeCase::Sampling { .. } => run_sampling(case, context),
        ProbeCase::Elicitation { .. } => run_elicitation(case, context),
        ProbeCase::ResourceSubscription { .. } => run_resource_subscription(case, context),
        ProbeCase::Completion { .. } => run_completion(case, context),
    }
}

/// Workflow access is an explicit operator attestation. An annotation that
/// contradicts it always refuses the complete workflow before any step runs.
fn validate_workflow_tools(case: &ProbeCase, catalog: &ToolCatalog) -> anyhow::Result<()> {
    let ProbeCase::Workflow { steps, .. } = case else {
        return Ok(());
    };
    for step in steps {
        let Some(tool) = catalog.tools.iter().find(|tool| tool.name == step.tool) else {
            bail!("workflow tool was not declared by the server");
        };
        if crate::standard::ToolClass::of(tool) == crate::standard::ToolClass::Writer {
            bail!("workflow refuses a tool annotated as a writer");
        }
    }
    Ok(())
}

fn run_workflow(
    case: &ProbeCase,
    repetitions: u64,
    steps: &[WorkflowStep],
    context: &mut RunContext<'_>,
) -> anyhow::Result<CaseReport> {
    // Do not reuse another case's client session. Never
    // reconnect midway: a lost exchange makes the whole workflow incomplete.
    let mut client = context.target.connect(context.timeout)?;
    client.initialize()?;
    let catalog = client.list_tools_catalog()?;
    validate_workflow_tools(case, &catalog)?;
    let previous = std::mem::replace(context.client, client);
    let result = (|| {
        let mut calls = 0;
        for _ in 0..repetitions {
            for step in steps {
                calls += 1;
                let response = call_named_and_record(&step.tool, &step.arguments, context)?;
                if let Some(reason) = check_expectation(&step.expect, &response) {
                    return Ok(failed_case(case, calls, reason));
                }
            }
        }
        Ok(passed_case(case, calls))
    })();
    *context.client = previous;
    result
}

fn run_contention(case: &ProbeCase, context: &mut RunContext<'_>) -> anyhow::Result<CaseReport> {
    let tool = case.tool().expect("contention has a tool").to_owned();
    let arguments = case.arguments().expect("contention has arguments").clone();
    let target = context.target.clone();
    let timeout = context.timeout;
    let (ready_tx, ready_rx) = mpsc::sync_channel(0);
    let (start_tx, start_rx) = mpsc::sync_channel(0);
    let worker_tool = tool.clone();
    let worker_arguments = arguments.clone();
    let worker = std::thread::spawn(move || -> anyhow::Result<(ToolResponse, u64)> {
        let mut client = target.connect(timeout)?;
        client.initialize()?;
        if !client
            .list_tools_catalog()?
            .tools
            .iter()
            .any(|listed| listed.name == worker_tool)
        {
            bail!("contended client is missing the probe tool");
        }
        ready_tx
            .send(())
            .map_err(|_| anyhow::anyhow!("contention coordinator closed"))?;
        let wait = target.budget.timeout(Duration::from_secs(30))?;
        target.budget.finish(
            start_rx
                .recv_timeout(wait)
                .map_err(|_| anyhow::anyhow!("contention coordinator closed")),
        )?;
        let started = Instant::now();
        let response = call_declining(&mut client, &worker_tool, &worker_arguments)?;
        Ok((response, started.elapsed().as_millis() as u64))
    });
    let primary = (|| {
        let wait = context.target.budget.timeout(Duration::from_secs(30))?;
        ready_rx
            .recv_timeout(wait)
            .map_err(|_| anyhow::anyhow!("contended client failed to initialize"))?;
        start_tx
            .send(())
            .map_err(|_| anyhow::anyhow!("contended client closed"))?;
        let started = Instant::now();
        let response = call_declining(context.client, &tool, &arguments)?;
        Ok::<_, anyhow::Error>((response, started.elapsed().as_millis() as u64))
    })();
    // Release coordination waits before joining, including initialization failures.
    drop(start_tx);
    drop(ready_rx);
    let secondary = worker
        .join()
        .map_err(|_| anyhow::anyhow!("contended client terminated unexpectedly"));
    let (primary, primary_latency) = primary?;
    record_response(&tool, &arguments, primary_latency, &primary, context)?;
    let (secondary, latency_ms) = secondary??;
    record_response(&tool, &arguments, latency_ms, &secondary, context)?;
    if matches!(primary, ToolResponse::Success(_)) && matches!(secondary, ToolResponse::Success(_))
    {
        Ok(passed_case(case, 2))
    } else {
        Ok(failed_case(case, 2, FailureReason::ContendedClientFailed))
    }
}

/// A `tools/call` that declines any request the server sends before it
/// answers, as a client without sampling or elicitation does.
fn call_declining(
    client: &mut ProbeClient,
    tool: &str,
    arguments: &Value,
) -> anyhow::Result<ToolResponse> {
    let (response, _) = client.call_tool_observing(
        tool,
        arguments,
        &mut |_, _| None,
        crate::standard::MAX_SERVER_REQUESTS,
    )?;
    Ok(response)
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
                attempts: attempt,
                first_failure: Some(attempt),
                reason: Some(FailureReason::UnexpectedOutcome),
                latency_ms: Some(slowest_ms),
                ..CaseReport::for_case(case)
            });
        }
        if latency_ms > max_latency_ms {
            return Ok(CaseReport {
                attempts: attempt,
                first_failure: Some(attempt),
                reason: Some(FailureReason::LatencyBudgetExceeded),
                latency_ms: Some(slowest_ms),
                ..CaseReport::for_case(case)
            });
        }
    }
    Ok(CaseReport {
        attempts,
        first_failure: None,
        reason: None,
        latency_ms: Some(slowest_ms),
        ..CaseReport::for_case(case)
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
                attempts: pages + 1,
                first_failure: Some(pages + 1),
                reason: Some(FailureReason::PaginationStalledCursor),
                tool_count: Some(seen.len() as u64),
                pages: Some(pages + 1),
                ..CaseReport::for_case(case)
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
                    attempts: pages,
                    first_failure: Some(pages),
                    reason: Some(FailureReason::PaginationDuplicateTool),
                    tool_count: Some(seen.len() as u64),
                    pages: Some(pages),
                    ..CaseReport::for_case(case)
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
        attempts: pages,
        first_failure: None,
        reason: None,
        tool_count: Some(seen.len() as u64),
        pages: Some(pages),
        ..CaseReport::for_case(case)
    })
}

fn passed_case(case: &ProbeCase, attempts: u64) -> CaseReport {
    CaseReport {
        attempts,
        ..CaseReport::for_case(case)
    }
}

fn failed_case(case: &ProbeCase, attempt: u64, reason: FailureReason) -> CaseReport {
    CaseReport {
        attempts: attempt,
        first_failure: Some(attempt),
        reason: Some(reason),
        ..CaseReport::for_case(case)
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
    let Some(journal) = context.journal.as_mut() else {
        return Ok(());
    };
    *journal.seq += 1;
    let (outcome, error) = match &response {
        ToolResponse::Success(_) => ("ok", None),
        ToolResponse::Error { payload, .. } => ("error", Some(error_info(payload, journal.salt))),
    };
    journal
        .store
        .append(&CallRecord {
            identity: Some(crate::record::EventIdentity::new(journal.capture_id)),
            ts: chrono::Utc::now()
                .format("%Y-%m-%dT%H:%M:%S%.3fZ")
                .to_string(),
            session: journal.session.to_owned(),
            seq: *journal.seq,
            server: journal.server.to_owned(),
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
            let Some(object) = result.as_object() else {
                return Some(FailureReason::MissingField);
            };
            if expect
                .required_result_fields
                .iter()
                .any(|field| !object.contains_key(field))
                || expect
                    .required_result_paths
                    .iter()
                    .any(|path| result.pointer(path).is_none())
            {
                return Some(FailureReason::MissingField);
            }
            if expect
                .equals
                .iter()
                .any(|(field, expected)| object.get(field) != Some(expected))
                || expect
                    .equals_paths
                    .iter()
                    .any(|(path, expected)| result.pointer(path) != Some(expected))
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
    let outcome = call_declining(
        context.client,
        case.tool().expect("payload case has a tool"),
        &arguments,
    );
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
            attempts,
            first_failure: Some(attempts),
            reason: Some(reason),
            latency_ms: Some(latency_ms),
            ..CaseReport::for_case(case)
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
        attempts,
        first_failure: None,
        reason: None,
        latency_ms: Some(latency_ms),
        ..CaseReport::for_case(case)
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
                    attempts: pages + 1,
                    first_failure: Some(pages + 1),
                    reason: Some(FailureReason::SurfaceStalledCursor),
                    pages: Some(pages + 1),
                    ..CaseReport::for_case(case)
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
        attempts: 1,
        first_failure: None,
        reason: None,
        tool_count: Some(total_items),
        ..CaseReport::for_case(case)
    })
}

/// For a tool that declares `outputSchema`, the response must carry
/// `structuredContent` satisfying the complete declared schema.
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
    let output_schema = output_schema.clone();
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
            attempts: 1,
            first_failure: Some(1),
            reason: Some(FailureReason::OutputSchemaDeclaredButMissing),
            ..CaseReport::for_case(case)
        });
    };
    let missing = required.iter().any(|field| structured.get(field).is_none());
    let reason = if missing {
        Some(FailureReason::OutputSchemaFieldMissing)
    } else if !crate::schema::conforms(&output_schema, &structured) {
        Some(FailureReason::OutputSchemaInvalidResult)
    } else {
        None
    };
    Ok(CaseReport {
        attempts: 1,
        first_failure: reason.map(|_| 1),
        reason,
        ..CaseReport::for_case(case)
    })
}

/// Cancellation probe: issue a read-only call, cancel it immediately, and
/// require the server to honor the cancellation: the cancelled request id
/// must never be resolved, or only with the structured "Request
/// cancelled" acknowledgement used by production servers.
fn run_cancellation(case: &ProbeCase, context: &mut RunContext<'_>) -> anyhow::Result<CaseReport> {
    let (tool, arguments, grace_seconds, reason) = match case {
        ProbeCase::Cancellation {
            tool,
            arguments,
            grace_seconds,
            reason,
            ..
        } => (
            tool.to_owned(),
            arguments.to_owned(),
            *grace_seconds,
            reason.to_owned(),
        ),
        _ => unreachable!("cancellation arm"),
    };
    // Preflight: the tool must succeed uncancelled, otherwise a pass here
    // would only mean the server was broken in a different way.
    let preflight = call_named_and_record(&tool, &arguments, context)?;
    if matches!(preflight, ToolResponse::Error { .. }) {
        return Ok(failed_case(case, 1, FailureReason::UnexpectedOutcome));
    }
    let outcome = context
        .client
        .cancel_tool_call(
            &tool,
            &arguments,
            &reason,
            std::time::Duration::from_secs(grace_seconds),
        )
        .context("cancellation probe failed")?;
    let attempts = 2;
    let failure = outcome_had_failure(outcome);
    Ok(CaseReport {
        attempts,
        first_failure: failure.map(|_| attempts),
        reason: failure,
        ..CaseReport::for_case(case)
    })
}

fn outcome_had_failure(outcome: crate::mcp_client::CancellationOutcome) -> Option<FailureReason> {
    match outcome {
        crate::mcp_client::CancellationOutcome::Honored => None,
        crate::mcp_client::CancellationOutcome::Ignored => Some(FailureReason::CancellationIgnored),
        crate::mcp_client::CancellationOutcome::Errored => Some(FailureReason::CancellationErrored),
    }
}

/// Protocol-negotiation probe: three handshakes assert version selection.
/// (1) A fresh handshake with the supported version must echo it, or answer
/// with another date-shaped version the server supports (the MCP lifecycle
/// lets a server that does not speak the requested version answer with its
/// own), which must itself be echoed on a further handshake. (2) A
/// fresh handshake with an unknown date-shaped version must answer with a
/// date-shaped, non-echoed version (the spec: respond with the server's
/// latest supported version, never the requested one). (3) The version
/// the server claims on the unknown handshake must itself be echoable on
/// a third handshake — a server that lies about its own support fails.
fn run_protocol_negotiation(
    case: &ProbeCase,
    context: &mut RunContext<'_>,
) -> anyhow::Result<CaseReport> {
    let bogus_version = match case {
        ProbeCase::ProtocolNegotiation { bogus_version, .. } => bogus_version.as_str(),
        _ => unreachable!("negotiation arm"),
    };
    let supported = crate::http_client::PROTOCOL_VERSION;
    let client = &mut context.client;
    let attempts = 3;
    let version_of = |reply: &Value| {
        reply
            .get("result")
            .and_then(|result| result.get("protocolVersion"))
            .and_then(Value::as_str)
            .map(str::to_owned)
    };
    // (1) The supported version is echoed, or answered with another
    //     supported version that is itself echoed.
    let answered = version_of(&client.initialize_raw(supported)?);
    let Some(answered) = answered.filter(|version| is_date_shaped(version)) else {
        return Ok(failed_case(
            case,
            1,
            FailureReason::NegotiationInvalidVersion,
        ));
    };
    if answered != supported
        && version_of(&client.initialize_raw(&answered)?).as_deref() != Some(answered.as_str())
    {
        return Ok(failed_case(
            case,
            1,
            FailureReason::NegotiationInconsistentSupport,
        ));
    }
    // (2) The unknown version must not be echoed back.
    let negotiated = version_of(&client.initialize_raw(bogus_version)?);
    let Some(negotiated) = negotiated else {
        return Ok(failed_case(
            case,
            2,
            FailureReason::NegotiationInvalidVersion,
        ));
    };
    if negotiated == bogus_version {
        return Ok(failed_case(
            case,
            2,
            FailureReason::NegotiationEchoedUnknown,
        ));
    }
    if !is_date_shaped(&negotiated) {
        return Ok(failed_case(
            case,
            2,
            FailureReason::NegotiationInvalidVersion,
        ));
    }
    // (3) The claimed version must actually be supported.
    if version_of(&client.initialize_raw(&negotiated)?).as_deref() != Some(negotiated.as_str()) {
        return Ok(failed_case(
            case,
            3,
            FailureReason::NegotiationInconsistentSupport,
        ));
    }
    Ok(passed_case(case, attempts))
}

/// A protocol version is date-shaped: YYYY-MM-DD.
fn is_date_shaped(version: &str) -> bool {
    let bytes = version.as_bytes();
    if bytes.len() != 10 {
        return false;
    }
    bytes.iter().enumerate().all(|(index, byte)| match index {
        4 | 7 => *byte == b'-',
        _ => byte.is_ascii_digit(),
    })
}

/// Sampling probe: declare the client `sampling` capability, call the
/// tool, and answer `sampling/createMessage` requests with a minimal
/// well-formed sample. The server may ask at most `max_requests` times;
/// more is a flood. The call must complete with a structured response.
fn run_sampling(case: &ProbeCase, context: &mut RunContext<'_>) -> anyhow::Result<CaseReport> {
    let (tool, arguments, max_requests) = match case {
        ProbeCase::Sampling {
            tool,
            arguments,
            max_requests,
            ..
        } => (tool.to_owned(), arguments.to_owned(), *max_requests),
        _ => unreachable!("sampling arm"),
    };
    let mut respond = |method: &str, params: &Value| -> Option<Value> {
        if method != "sampling/createMessage" {
            return None;
        }
        // The sample must echo the request's structure minimally: the
        // role from the request, one text message, and the declared model
        // preferences when present.
        let role = params
            .get("messages")
            .and_then(Value::as_array)
            .and_then(|messages| messages.first())
            .and_then(|message| message.get("role"))
            .and_then(Value::as_str)
            .unwrap_or("user")
            .to_owned();
        let mut sample = json!({
            "role": "assistant",
            "content": {"type": "text", "text": "mcpeval sample"},
            "model": "mcpeval-stub",
        });
        if role == "assistant" {
            sample["role"] = json!("user");
        }
        Some(sample)
    };
    let outcome = context
        .client
        .call_tool_observing(&tool, &arguments, &mut respond, max_requests);
    let (response, server_requests) = match outcome {
        Ok(outcome) => outcome,
        // Flood is a distinct defect; other transport failures (including
        // the flood bound) are declared reasons.
        Err(error) => {
            if format!("{error:#}").contains("more sub-requests") {
                return Ok(failed_case(case, 1, FailureReason::SamplingRequestFlood));
            }
            if TransportFailure::of(&error) == Some(TransportFailure::Timeout) {
                return Ok(failed_case(case, 1, FailureReason::SamplingStalledCall));
            }
            return Err(error.context("sampling probe failed"));
        }
    };
    let _ = server_requests;
    let failure = match &response {
        ToolResponse::Success(_) => None,
        ToolResponse::Error { payload, .. } => {
            // The call completing with a structured error is only a
            // failure when the server's error shows sampling broke the
            // call: an unhandled `sampling/createMessage` shape or a
            // malformed request the server itself produced.
            payload
                .get("code")
                .and_then(Value::as_i64)
                .map(|_| FailureReason::SamplingInvalidRequest)
        }
    };
    record_response(&tool, &arguments, 0, &response, context)?;
    Ok(CaseReport {
        attempts: 1,
        first_failure: failure.map(|_| 1),
        reason: failure,
        ..CaseReport::for_case(case)
    })
}

/// Elicitation probe: declare the client `elicitation` capability, call
/// the tool, and answer `elicitation/create` requests with the manifest's
/// declared response action. The server may ask at most `max_requests`
/// times; more is a flood.
fn run_elicitation(case: &ProbeCase, context: &mut RunContext<'_>) -> anyhow::Result<CaseReport> {
    let (tool, arguments, max_requests, respond_with) = match case {
        ProbeCase::Elicitation {
            tool,
            arguments,
            max_requests,
            respond,
            ..
        } => (
            tool.to_owned(),
            arguments.to_owned(),
            *max_requests,
            *respond,
        ),
        _ => unreachable!("elicitation arm"),
    };
    let mut respond = move |method: &str, params: &Value| -> Option<Value> {
        if method != "elicitation/create" {
            return None;
        }
        // The request must carry a message and a schema object; a
        // malformed request is answered with method-unavailable so the
        // server's handling surfaces in the tool outcome.
        let well_formed = params.get("message").and_then(Value::as_str).is_some()
            && params.get("requestedSchema").is_some_and(Value::is_object);
        if !well_formed {
            return None;
        }
        let action = match respond_with {
            crate::manifest::ElicitationResponse::Accept => "accept",
            crate::manifest::ElicitationResponse::Decline => "decline",
            crate::manifest::ElicitationResponse::Cancel => "cancel",
        };
        Some(json!({"action": action}))
    };
    let outcome = context
        .client
        .call_tool_observing(&tool, &arguments, &mut respond, max_requests);
    let (response, _server_requests) = match outcome {
        Ok(outcome) => outcome,
        Err(error) => {
            if format!("{error:#}").contains("more sub-requests") {
                return Ok(failed_case(case, 1, FailureReason::ElicitationRequestFlood));
            }
            if TransportFailure::of(&error) == Some(TransportFailure::Timeout) {
                return Ok(failed_case(case, 1, FailureReason::ElicitationStalledCall));
            }
            return Err(error.context("elicitation probe failed"));
        }
    };
    let failure = match &response {
        ToolResponse::Success(_) => None,
        ToolResponse::Error { payload, .. } => payload
            .get("code")
            .and_then(Value::as_i64)
            .map(|_| FailureReason::ElicitationInvalidRequest),
    };
    record_response(&tool, &arguments, 0, &response, context)?;
    Ok(CaseReport {
        attempts: 1,
        first_failure: failure.map(|_| 1),
        reason: failure,
        ..CaseReport::for_case(case)
    })
}

/// Resource-subscription probe: for a server that declares
/// `resources.subscribe`, read the declared URI, subscribe, fire the
/// trigger tool, and require `notifications/resources/updated` for the
/// URI within the wait bound, then unsubscribe cleanly. Servers that do
/// not declare `subscribe` pass trivially.
fn run_resource_subscription(
    case: &ProbeCase,
    context: &mut RunContext<'_>,
) -> anyhow::Result<CaseReport> {
    let (uri, trigger_tool, trigger_arguments, max_wait_seconds) = match case {
        ProbeCase::ResourceSubscription {
            uri,
            trigger_tool,
            trigger_arguments,
            max_wait_seconds,
            ..
        } => (
            uri.to_owned(),
            trigger_tool.clone(),
            trigger_arguments.clone(),
            *max_wait_seconds,
        ),
        _ => unreachable!("subscription arm"),
    };
    let subscribes = context
        .client
        .capabilities()
        .and_then(|value| value.get("resources").cloned())
        .and_then(|resources| resources.get("subscribe").and_then(Value::as_bool))
        .unwrap_or(false);
    if !subscribes {
        // Undeclared subscription support passes trivially, mirroring
        // surface-listing's treatment of undeclared surfaces.
        return Ok(passed_case(case, 0));
    }
    // A transport failure or a structured JSON-RPC error both mean the
    // server cannot serve the URI it claims to expose.
    let unreadable = match context.client.read_resource(&uri) {
        Err(_) => true,
        Ok(response) => response.get("error").is_some(),
    };
    if unreadable {
        return Ok(failed_case(case, 1, FailureReason::ResourceUnreadable));
    }
    // Subscribe before firing the trigger so the server's update
    // notification cannot race ahead of the subscription.
    let subscribed = context
        .client
        .raw_request("resources/subscribe", json!({"uri": uri}))
        .map(|response| response.get("error").is_none())
        .unwrap_or(false);
    if !subscribed {
        return Ok(failed_case(case, 2, FailureReason::SubscriptionRejected));
    }
    let mut trigger_ok = true;
    if let Some(tool) = &trigger_tool {
        let arguments = trigger_arguments.clone().unwrap_or_else(|| json!({}));
        trigger_ok = matches!(
            call_named_and_record(tool, &arguments, context)?,
            ToolResponse::Success(_)
        );
    }
    let attempts = 3;
    if !trigger_ok {
        return Ok(failed_case(
            case,
            attempts,
            FailureReason::UnexpectedOutcome,
        ));
    }
    let notified = context
        .client
        .wait_for_resource_update(&uri, std::time::Duration::from_secs(max_wait_seconds));
    let unsubscribed = context
        .client
        .unsubscribe(&uri)
        .map(|response| response.get("error").is_none())
        .unwrap_or(false);
    if !notified {
        return Ok(CaseReport {
            attempts,
            first_failure: Some(attempts),
            reason: Some(FailureReason::SubscriptionNotificationMissing),
            ..CaseReport::for_case(case)
        });
    }
    if !unsubscribed {
        return Ok(failed_case(
            case,
            attempts,
            FailureReason::SubscriptionRejected,
        ));
    }
    Ok(passed_case(case, attempts))
}

/// Completion probe: for a server that declares the `completions`
/// capability, issue one `completion/complete` request for the declared
/// reference and argument. The server must answer with a well-formed
/// completion — `completion.values` (an array of strings) with optional
/// `hasMore`/`total` — and stay within `max_values`. A structured error
/// naming an unknown argument is the argument-unknown defect; a transport
/// failure or malformed envelope is the invalid-request defect. Servers
/// that do not declare `completions` pass trivially.
fn run_completion(case: &ProbeCase, context: &mut RunContext<'_>) -> anyhow::Result<CaseReport> {
    let (ref_uri, ref_type, argument_name, argument_value, max_values) = match case {
        ProbeCase::Completion {
            ref_uri,
            ref_type,
            argument_name,
            argument_value,
            max_values,
            ..
        } => (
            ref_uri.to_owned(),
            ref_type.to_owned(),
            argument_name.to_owned(),
            argument_value.to_owned(),
            *max_values,
        ),
        _ => unreachable!("completion arm"),
    };
    let declares_completions = context
        .client
        .capabilities()
        .is_some_and(|value| value.get("completions").is_some());
    if !declares_completions {
        // Undeclared completion support passes trivially, mirroring
        // surface-listing's treatment of undeclared surfaces.
        return Ok(passed_case(case, 0));
    }
    let response =
        match context
            .client
            .complete(&ref_type, &ref_uri, &argument_name, &argument_value)
        {
            Ok(response) => response,
            // The transport-level request itself failed: the server cannot
            // answer the capability it declared.
            Err(_) => {
                return Ok(failed_case(
                    case,
                    1,
                    FailureReason::CompletionStalledRequest,
                ))
            }
        };
    let completion = response
        .get("result")
        .and_then(|result| result.get("completion"))
        .cloned();
    let Some(completion) = completion else {
        // A structured JSON-RPC error is either the argument-unknown
        // defect (the server names the argument anywhere in its error
        // message or data) or a plain capability-vs-implementation break.
        let unknown_argument = response
            .get("error")
            .map(|error| error.to_string().contains(&argument_name))
            .unwrap_or(false);
        return Ok(failed_case(
            case,
            1,
            if unknown_argument {
                FailureReason::CompletionArgumentUnknown
            } else {
                FailureReason::CompletionInvalidRequest
            },
        ));
    };
    let values = completion
        .get("values")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let well_formed = values.iter().all(Value::is_string);
    if !well_formed {
        return Ok(failed_case(
            case,
            1,
            FailureReason::CompletionInvalidRequest,
        ));
    }
    if values.len() > max_values as usize {
        return Ok(failed_case(case, 1, FailureReason::CompletionValueFlood));
    }
    Ok(passed_case(case, 1))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn budget_target(duration: Duration, requests: u64) -> ClientTarget {
        let mut target = ClientTarget::new(
            vec![
                "python3".into(),
                concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/tests/fixtures/transport_fault_server.py"
                )
                .into(),
            ],
            None,
            false,
        )
        .unwrap();
        target.budget = crate::evaluation_budget::Budget::new(duration, requests);
        target
    }

    #[test]
    fn evaluation_budget_is_shared_across_reconnections_and_catalog_requests() {
        // Keep the normal wall-clock fuse so cold interpreter startup cannot
        // replace the request-limit path this test must exercise.
        let target = budget_target(crate::evaluation_budget::MAX_DURATION, 3);
        let mut first = target.connect(None).unwrap();
        first.initialize().unwrap();
        first.list_tools_catalog().unwrap();
        drop(first);
        let mut second = target.clone().connect(None).unwrap();
        second.initialize().unwrap();
        let error = second.list_tools_catalog().unwrap_err();
        assert!(error
            .downcast_ref::<crate::evaluation_budget::Exhausted>()
            .is_some());
        assert!(
            target.connect(None).is_err(),
            "exhaustion must prevent another process launch"
        );
    }

    #[test]
    fn evaluation_budget_cuts_short_a_slow_stdio_call_and_reaps_its_child() {
        let target = budget_target(crate::mcp_client::DEFAULT_RESPONSE_TIMEOUT, 100);
        let mut client = target.connect(None).unwrap();
        client.initialize().unwrap();
        // Startup keeps its normal allowance; only the measured call gets
        // the short deadline, so cold Python startup cannot fail this test.
        let ProbeClient::Stdio(ref mut stdio) = client else {
            unreachable!()
        };
        stdio.set_evaluation_budget(crate::evaluation_budget::Budget::new(
            Duration::from_secs(2),
            100,
        ));
        let started = Instant::now();
        let result = client.call_tool("slow", &json!({"ms": 5000}));
        drop(client);
        let error = result.unwrap_err();
        assert!(error
            .downcast_ref::<crate::evaluation_budget::Exhausted>()
            .is_some());
        assert!(
            started.elapsed() < Duration::from_secs(4),
            "call and cleanup exceeded the shared deadline allowance"
        );
    }

    #[test]
    fn evaluation_budget_does_not_publish_partial_readiness() {
        let target = budget_target(crate::evaluation_budget::MAX_DURATION, 3);
        let mut report = ProbeReport::default();
        measure_standard(
            &mut report,
            &target,
            &crate::standard::StandardOptions {
                confirm_read_only: true,
                skip_tools: vec![],
            },
        )
        .unwrap();
        assert!(
            report.readiness.is_none(),
            "exhaustion must not fold a partial battery into a score"
        );
        assert_eq!(
            report.readiness_error.map(|r| r.as_str()),
            Some("evaluation-budget-exceeded")
        );
    }

    #[test]
    fn evaluation_budget_contention_exhaustion_joins_workers_and_is_not_a_verdict() {
        let target = budget_target(crate::evaluation_budget::MAX_DURATION, 5);
        let mut client = target.connect(None).unwrap();
        client.initialize().unwrap();
        let catalog = client.list_tools_catalog().unwrap();
        let case = ProbeCase::Contention {
            id: "concurrent".into(),
            tool: "ok".into(),
            access: Access::ReadOnly,
            sandbox: None,
            arguments: json!({}),
        };
        let started = Instant::now();
        let error = run_unjournaled(&case, &mut client, &catalog, &target, None).unwrap_err();
        drop(client);
        let reason = transport_reason(&error);
        assert_eq!(reason, FailureReason::EvaluationBudgetExceeded);
        assert!(
            reason.is_transport(),
            "unfinished verification must not receive credit"
        );
        // Includes a second interpreter's startup. The shared request cap is
        // the exhaustion oracle; elapsed time is only the normal safety fuse.
        assert!(started.elapsed() < crate::evaluation_budget::MAX_DURATION);
    }

    #[test]
    fn failure_reason_all_lists_every_variant_and_labels_round_trip() {
        let source = include_str!("probe.rs");
        let body = source
            .split_once("pub enum FailureReason {")
            .and_then(|(_, rest)| rest.split_once("\n}"))
            .map(|(body, _)| body)
            .unwrap();
        let declared: Vec<&str> = body
            .lines()
            .map(str::trim)
            .filter(|line| line.ends_with(',') && !line.starts_with("//"))
            .map(|line| line.trim_end_matches(','))
            .collect();
        let listed: Vec<String> = FailureReason::ALL
            .iter()
            .map(|reason| format!("{reason:?}"))
            .collect();
        assert_eq!(
            listed, declared,
            "FailureReason::ALL must list every variant in order"
        );
        for reason in FailureReason::ALL {
            assert_eq!(
                FailureReason::from_report_label(reason.as_str()),
                Some(*reason)
            );
        }
    }

    #[test]
    fn the_published_schemas_list_every_reason_and_probe_kind() {
        use clap::ValueEnum;
        let labels = |values: &Value| -> Vec<String> {
            values
                .as_array()
                .unwrap()
                .iter()
                .map(|value| value.as_str().unwrap().to_owned())
                .collect()
        };
        let reasons: Vec<String> = FailureReason::ALL
            .iter()
            .map(|reason| reason.as_str().to_owned())
            .collect();
        let kinds: Vec<String> = ProbeKind::value_variants()
            .iter()
            .map(|kind| kind.as_str().to_owned())
            .collect();
        let report: Value =
            serde_json::from_str(include_str!("../docs/mcp-eval.probe-report.schema.json"))
                .unwrap();
        assert_eq!(labels(&report["$defs"]["reason"]["enum"]), reasons);
        assert_eq!(
            labels(&report["$defs"]["case"]["properties"]["probe"]["enum"]),
            kinds
        );
        let diff: Value =
            serde_json::from_str(include_str!("../docs/mcp-eval.probe-diff.schema.json")).unwrap();
        assert_eq!(
            labels(&diff["$defs"]["reason"]["oneOf"][0]["enum"]),
            reasons
        );
        assert_eq!(
            labels(&diff["$defs"]["case"]["properties"]["probe"]["enum"]),
            kinds
        );
        let areas: Vec<String> = crate::score::Area::ALL
            .iter()
            .map(|area| area.as_str().to_owned())
            .collect();
        let checks: Vec<String> = crate::score::CheckId::ALL
            .iter()
            .map(|id| id.as_str().to_owned())
            .collect();
        let check_reasons: Vec<String> = crate::score::CheckReason::ALL
            .iter()
            .map(|reason| reason.as_str().to_owned())
            .collect();
        assert_eq!(
            report["properties"]["schema"]["const"],
            "mcpeval.probe-report/v2"
        );
        assert_eq!(
            labels(&report["$defs"]["area"]["properties"]["name"]["enum"]),
            areas
        );
        assert_eq!(
            labels(&report["$defs"]["check"]["properties"]["id"]["enum"]),
            checks
        );
        assert_eq!(
            labels(&report["$defs"]["check"]["properties"]["reason"]["enum"]),
            check_reasons
        );
        assert_eq!(
            diff["properties"]["schema"]["const"],
            "mcpeval.probe-diff/v2"
        );
    }

    #[test]
    fn v1_documents_keep_their_score_as_legacy_and_v2_documents_round_trip() {
        let v1 = json!({
            "schema": "mcpeval.probe-report/v1", "server": "demo", "passed": true,
            "readiness": {"score": 100, "categories": [], "badge": "https://img.shields.io/badge/x"},
            "cases": []
        });
        let report = ProbeReport::from_json_document(&v1).unwrap();
        assert_eq!(report.legacy_score, Some(100));
        assert!(report.readiness.is_none());

        let readiness = crate::score::fold(&crate::standard::Observations::default());
        let report = ProbeReport {
            readiness: Some(readiness.clone()),
            ..ProbeReport::default()
        };
        let document = report.to_json("demo");
        assert_eq!(document["schema"], "mcpeval.probe-report/v2");
        assert_eq!(document["gate"], Value::Null);
        let parsed = ProbeReport::from_json_document(&document).unwrap();
        assert_eq!(parsed.readiness, Some(readiness));
        assert_eq!(parsed.legacy_score, None);
        assert!(
            ProbeReport::from_json_document(&json!({"schema": "mcpeval.probe-report/v3"})).is_err()
        );
    }

    #[test]
    fn probe_kind_labels_round_trip() {
        use clap::ValueEnum;
        for kind in ProbeKind::value_variants() {
            assert_eq!(ProbeKind::from_report_label(kind.as_str()), Some(*kind));
        }
    }
}
