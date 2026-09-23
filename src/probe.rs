use std::path::PathBuf;
use std::sync::{mpsc, Arc, Barrier};
use std::time::{Duration, Instant};

use crate::fingerprint::Salt;
use crate::http_client::HttpMcpClient;
use crate::manifest::{Access, Expectation, Manifest, OutcomeExpectation, ProbeCase, ProbeKind};
use crate::mcp_client::{McpClient, ToolCatalog, ToolDefinition, ToolResponse, TransportFailure};
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

    /// Open a client whose requests wait `timeout` (`None`: the
    /// transport's default).
    fn connect(&self, timeout: Option<Duration>) -> anyhow::Result<ProbeClient> {
        let mut client = match self {
            Self::Stdio(command) => ProbeClient::Stdio(McpClient::spawn(command)?),
            Self::Http {
                endpoint,
                allow_remote,
            } => ProbeClient::Http(HttpMcpClient::connect(endpoint, *allow_remote)?),
        };
        client.set_response_timeout(timeout);
        Ok(client)
    }
}

impl ProbeClient {
    fn set_response_timeout(&mut self, timeout: Option<Duration>) {
        match self {
            Self::Stdio(client) => client.set_response_timeout(timeout),
            Self::Http(client) => client.set_response_timeout(timeout),
        }
    }

    /// The response timeout the transport applies when none is set.
    fn default_timeout(&self) -> Duration {
        match self {
            Self::Stdio(_) => crate::mcp_client::DEFAULT_RESPONSE_TIMEOUT,
            Self::Http(_) => crate::http_client::DEFAULT_IO_TIMEOUT,
        }
    }

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

    fn cancel_tool_call(
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

    fn initialize_raw(&mut self, protocol_version: &str) -> anyhow::Result<serde_json::Value> {
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

    fn call_tool_observing(
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

    fn read_resource(&mut self, uri: &str) -> anyhow::Result<serde_json::Value> {
        match self {
            Self::Stdio(client) => client.read_resource(uri),
            Self::Http(client) => client.read_resource(uri),
        }
    }

    fn complete(
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

    fn wait_for_resource_update(&mut self, uri: &str, wait: std::time::Duration) -> bool {
        match self {
            Self::Stdio(client) => client.wait_for_resource_update(uri, wait),
            Self::Http(client) => client.wait_for_resource_update(uri, wait),
        }
    }

    fn unsubscribe(&mut self, uri: &str) -> anyhow::Result<serde_json::Value> {
        match self {
            Self::Stdio(client) => client.unsubscribe(uri),
            Self::Http(client) => client.unsubscribe(uri),
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
}

impl ProbeKind {
    pub fn from_report_label(label: &str) -> Option<Self> {
        let candidate = match label {
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
        }
    }

    /// Transport reasons mean the case was not evaluated at all; every
    /// other reason is a verdict on the server.
    pub fn is_transport(&self) -> bool {
        matches!(
            self,
            Self::TransportTimeout | Self::TransportClosed | Self::TransportError
        )
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

/// The manifest bound a failing case exceeded, beside the observed value:
/// share-safe numbers only.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BoundDetail {
    /// The manifest field that set the limit, such as `max_tools`.
    pub bound: &'static str,
    pub limit: u64,
    pub observed: u64,
}

#[derive(Debug)]
pub struct CaseReport {
    pub id: String,
    pub probe: ProbeKind,
    /// The tool the case calls, when it calls one.
    pub tool: Option<String>,
    pub attempts: u64,
    pub first_failure: Option<u64>,
    pub reason: Option<FailureReason>,
    /// The bound behind a bound-based failure.
    pub detail: Option<BoundDetail>,
    pub tool_count: Option<u64>,
    pub schema_bytes: Option<u64>,
    pub token_usage: Option<TokenUsage>,
    /// Slowest observed call for latency-budget cases.
    pub latency_ms: Option<u64>,
    /// Number of `tools/list` pages visited by pagination cases.
    pub pages: Option<u64>,
}

impl CaseReport {
    /// A report for `case` with no attempts, verdict, or measurements yet.
    fn for_case(case: &ProbeCase) -> Self {
        Self {
            id: case.id().to_owned(),
            probe: case.kind(),
            tool: case.tool().map(str::to_owned),
            attempts: 0,
            first_failure: None,
            reason: None,
            detail: None,
            tool_count: None,
            schema_bytes: None,
            token_usage: None,
            latency_ms: None,
            pages: None,
        }
    }

    pub fn passed(&self) -> bool {
        self.reason.is_none()
    }

    /// The case could not be evaluated: its reason is a transport reason.
    pub fn errored(&self) -> bool {
        self.reason.is_some_and(|reason| reason.is_transport())
    }
}

#[derive(Debug)]
pub struct ProbeReport {
    pub cases: Vec<CaseReport>,
    /// Lowercase hex SHA-256 of the manifest bytes the run parsed.
    pub manifest_sha256: Option<String>,
}

impl ProbeReport {
    /// Reconstruct a report from its `mcpeval.probe-report/v1` document —
    /// the committed-baseline format. Measurements are restored where the
    /// document carries them; the reconstructed report renders text,
    /// markdown, and SARIF identically to the run that produced it.
    pub fn from_json_document(document: &serde_json::Value) -> anyhow::Result<Self> {
        if document.get("schema").and_then(serde_json::Value::as_str)
            != Some("mcpeval.probe-report/v1")
        {
            bail!("document is not an mcpeval.probe-report/v1 report");
        }
        let cases = document
            .get("cases")
            .and_then(serde_json::Value::as_array)
            .context("report document has no cases array")?;
        let mut parsed = Vec::with_capacity(cases.len());
        for case in cases {
            let probe_label = case
                .get("probe")
                .and_then(serde_json::Value::as_str)
                .context("case is missing a probe label")?;
            let probe = ProbeKind::from_report_label(probe_label)
                .with_context(|| format!("unknown probe label {probe_label}"))?;
            let reason = match case.get("reason") {
                None | Some(serde_json::Value::Null) => None,
                Some(label) => Some(
                    FailureReason::from_report_label(
                        label.as_str().context("reason must be a string")?,
                    )
                    .with_context(|| format!("unknown reason label {}", label))?,
                ),
            };
            let measurements = case
                .get("measurements")
                .cloned()
                .unwrap_or(serde_json::json!({}));
            let token_usage = measurements.get("total_tokens").and_then(|total| {
                let total_tokens = total.as_u64()?;
                let per_tool = measurements
                    .get("per_tool")
                    .and_then(serde_json::Value::as_array)
                    .map(|tools| {
                        tools
                            .iter()
                            .filter_map(|tool| {
                                Some(ToolTokenUsage {
                                    tool: tool.get("tool")?.as_str()?.to_owned(),
                                    tokens: tool.get("tokens")?.as_u64()?,
                                })
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                Some(TokenUsage {
                    total_tokens,
                    per_tool,
                })
            });
            parsed.push(CaseReport {
                id: case
                    .get("id")
                    .and_then(serde_json::Value::as_str)
                    .context("case is missing an id")?
                    .to_owned(),
                probe,
                tool: case
                    .get("tool")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_owned),
                attempts: case
                    .get("attempts")
                    .and_then(serde_json::Value::as_u64)
                    .unwrap_or(0),
                first_failure: case
                    .get("first_failure")
                    .and_then(serde_json::Value::as_u64),
                reason,
                detail: None,
                tool_count: measurements
                    .get("tool_count")
                    .and_then(serde_json::Value::as_u64),
                schema_bytes: measurements
                    .get("schema_bytes")
                    .and_then(serde_json::Value::as_u64),
                token_usage,
                latency_ms: measurements
                    .get("latency_ms")
                    .and_then(serde_json::Value::as_u64),
                pages: measurements
                    .get("pages")
                    .and_then(serde_json::Value::as_u64),
            });
        }
        Ok(Self {
            cases: parsed,
            manifest_sha256: document
                .get("manifest_sha256")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned),
        })
    }

    pub fn passed(&self) -> bool {
        self.cases.iter().all(CaseReport::passed)
    }

    /// Some case could not be evaluated, so the run is incomplete.
    pub fn errored(&self) -> bool {
        self.cases.iter().any(CaseReport::errored)
    }

    /// Versioned, deterministic JSON document: no timestamps, no session
    /// identifiers, cases in manifest order. Contains only share-safe
    /// fields — the generator, server label, manifest hash, case IDs, probe
    /// kinds, tool names, counts, fixed reason labels with their static
    /// remediation hints, declared bounds, measurement numbers, and the
    /// readiness score. Suitable for CI artifacts and committed baselines.
    /// `docs/mcp-eval.probe-report.schema.json` describes it.
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
                    "tool": case.tool,
                    "passed": case.passed(),
                    "attempts": case.attempts,
                    "first_failure": case.first_failure,
                    "reason": case.reason.map(|reason| reason.as_str()),
                    "hint": case.reason.map(crate::remediation::hint),
                    "detail": case.detail.map(|detail| serde_json::json!({
                        "bound": detail.bound,
                        "limit": detail.limit,
                        "observed": detail.observed,
                    })),
                    "measurements": serde_json::Value::Object(measurements),
                })
            })
            .collect();
        let readiness = crate::score::readiness(self);
        serde_json::json!({
            "schema": "mcpeval.probe-report/v1",
            "generator": {"name": "mcpeval", "version": env!("CARGO_PKG_VERSION")},
            "server": server,
            "manifest_sha256": self.manifest_sha256,
            "passed": self.passed(),
            "readiness": readiness.to_json(),
            "cases": cases,
        })
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
    let catalog = client.list_tools_catalog()?;
    for case in &cases {
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
        server: &options.server,
        session: &session,
        seq: &mut seq,
        salt: &salt,
        client: &mut client,
        catalog: &catalog,
        target: &target,
        timeout,
        store,
    };
    // A failure inside one case costs that case, never the run: it is
    // reported as errored and the next case starts on a fresh connection,
    // because the session state after a lost or broken exchange is unknown.
    // Once the server cannot be reached again, the remaining cases error
    // with the same reason.
    let mut unreachable = None;
    for case in cases {
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
                eprintln!("{} {}: {error:#}", case.id(), reason.as_str());
                reports.push(errored_case(case, reason));
                match reconnect(&target, timeout) {
                    Ok(fresh) => *context.client = fresh,
                    Err(error) => {
                        let reason = transport_reason(&error);
                        eprintln!("reconnecting failed: {error:#}");
                        unreachable = Some(reason);
                    }
                }
            }
        }
    }
    Ok(ProbeReport {
        cases: reports,
        manifest_sha256: Some(manifest_sha256),
    })
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
            let body = std::fs::read(&options.manifest_path).context("reading manifest")?;
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

fn transport_reason(error: &anyhow::Error) -> FailureReason {
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

struct RunContext<'a> {
    server: &'a str,
    session: &'a str,
    seq: &'a mut u64,
    salt: &'a Salt,
    client: &'a mut ProbeClient,
    catalog: &'a ToolCatalog,
    target: &'a ClientTarget,
    /// The manifest's response timeout, for connections a case opens.
    timeout: Option<Duration>,
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

fn run_contention(case: &ProbeCase, context: &mut RunContext<'_>) -> anyhow::Result<CaseReport> {
    let tool = case.tool().expect("contention has a tool").to_owned();
    let arguments = case.arguments().expect("contention has arguments").clone();
    let target = context.target.clone();
    let timeout = context.timeout;
    let barrier = Arc::new(Barrier::new(2));
    let worker_barrier = Arc::clone(&barrier);
    let (ready_tx, ready_rx) = mpsc::sync_channel(0);
    let worker_tool = tool.clone();
    let worker_arguments = arguments.clone();
    let worker = std::thread::spawn(move || -> anyhow::Result<(ToolResponse, u64)> {
        let mut client = target.connect(timeout)?;
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
            attempts: 1,
            first_failure: Some(1),
            reason: Some(FailureReason::OutputSchemaDeclaredButMissing),
            ..CaseReport::for_case(case)
        });
    };
    let missing = required.iter().any(|field| structured.get(field).is_none());
    Ok(CaseReport {
        attempts: 1,
        first_failure: missing.then_some(1),
        reason: missing.then_some(FailureReason::OutputSchemaFieldMissing),
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
/// (1) A fresh handshake with the supported version must echo it. (2) A
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
    // (1) The supported version must be echoed verbatim.
    let supported_reply = client.initialize_raw(supported)?;
    let echoed = supported_reply
        .get("result")
        .and_then(|result| result.get("protocolVersion"))
        .and_then(Value::as_str);
    if echoed != Some(supported) {
        return Ok(failed_case(
            case,
            1,
            FailureReason::NegotiationInvalidVersion,
        ));
    }
    // (2) The unknown version must not be echoed back.
    let unknown_reply = client.initialize_raw(bogus_version)?;
    let negotiated = unknown_reply
        .get("result")
        .and_then(|result| result.get("protocolVersion"))
        .and_then(Value::as_str);
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
    if !is_date_shaped(negotiated) {
        return Ok(failed_case(
            case,
            2,
            FailureReason::NegotiationInvalidVersion,
        ));
    }
    // (3) The claimed version must actually be supported.
    let claimed_reply = client.initialize_raw(negotiated)?;
    let claimed_echo = claimed_reply
        .get("result")
        .and_then(|result| result.get("protocolVersion"))
        .and_then(Value::as_str);
    if claimed_echo != Some(negotiated) {
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
    }

    #[test]
    fn probe_kind_labels_round_trip() {
        use clap::ValueEnum;
        for kind in ProbeKind::value_variants() {
            assert_eq!(ProbeKind::from_report_label(kind.as_str()), Some(*kind));
        }
    }
}
