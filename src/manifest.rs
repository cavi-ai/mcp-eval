use std::collections::{BTreeMap, HashSet};
use std::path::Path;

use anyhow::{bail, Context};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::privacy;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub version: u64,
    /// Per-request response timeout in milliseconds, 100..=600000. Unset,
    /// each transport keeps its own default (30 s stdio, 5 s HTTP).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
    #[serde(default)]
    pub sandboxes: BTreeMap<String, Sandbox>,
    pub probes: Vec<ProbeCase>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Sandbox {
    pub description: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Access {
    ReadOnly,
    Mutating,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum OutcomeExpectation {
    #[serde(rename = "ok")]
    Ok,
    #[serde(rename = "error")]
    Error,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Expectation {
    pub outcome: OutcomeExpectation,
    #[serde(default)]
    pub required_result_fields: Vec<String>,
    #[serde(default)]
    pub equals: BTreeMap<String, Value>,
    pub error_code: Option<i64>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "probe", deny_unknown_fields)]
pub enum ProbeCase {
    #[serde(rename = "contention")]
    Contention {
        id: String,
        tool: String,
        access: Access,
        sandbox: Option<String>,
        arguments: Value,
    },
    #[serde(rename = "error-honesty")]
    ErrorHonesty {
        id: String,
        tool: String,
        access: Access,
        sandbox: Option<String>,
        arguments: Value,
        max_attempts: u64,
        expect_retryable: bool,
    },
    #[serde(rename = "state-recovery")]
    StateRecovery {
        id: String,
        failure_tool: String,
        failure_arguments: Value,
        recovery_tool: String,
        recovery_arguments: Value,
        validation_tool: String,
        validation_arguments: Value,
        access: Access,
        sandbox: Option<String>,
    },
    #[serde(rename = "discovery-cost")]
    DiscoveryCost {
        id: String,
        access: Access,
        max_tools: u64,
        max_schema_bytes: u64,
    },
    #[serde(rename = "token-cost")]
    TokenCost {
        id: String,
        access: Access,
        max_total_tokens: u64,
        max_tool_tokens: Option<u64>,
    },
    #[serde(rename = "schema-guessability")]
    SchemaGuessability {
        id: String,
        tool: String,
        access: Access,
        sandbox: Option<String>,
        arguments: Value,
    },
    #[serde(rename = "degradation-over-n")]
    DegradationOverN {
        id: String,
        tool: String,
        access: Access,
        sandbox: Option<String>,
        arguments: Value,
        max_attempts: u64,
    },
    #[serde(rename = "instruction-fidelity")]
    InstructionFidelity {
        id: String,
        tool: String,
        access: Access,
        sandbox: Option<String>,
        arguments: Value,
        expect: Expectation,
    },
    #[serde(rename = "latency-budget")]
    LatencyBudget {
        id: String,
        tool: String,
        access: Access,
        sandbox: Option<String>,
        arguments: Value,
        attempts: u64,
        max_latency_ms: u64,
    },
    #[serde(rename = "pagination")]
    Pagination {
        id: String,
        access: Access,
        max_pages: u64,
    },
    #[serde(rename = "payload-bounds")]
    PayloadBounds {
        id: String,
        tool: String,
        access: Access,
        sandbox: Option<String>,
        /// Base argument object; the probe injects one oversized string
        /// field into a deep copy of it.
        arguments: Value,
        /// Name of the field receiving the oversized string.
        field: String,
        /// Exact encoded size of the injected string in bytes.
        size_bytes: u64,
        /// When true, a clean JSON-RPC error is a failure: the operator
        /// asserts the tool must handle this payload size, not merely
        /// reject it politely.
        expect_handled: bool,
    },
    #[serde(rename = "surface-listing")]
    SurfaceListing {
        id: String,
        access: Access,
        max_pages: u64,
    },
    #[serde(rename = "output-schema")]
    OutputSchema {
        id: String,
        tool: String,
        access: Access,
        sandbox: Option<String>,
        arguments: Value,
    },
    #[serde(rename = "cancellation")]
    Cancellation {
        id: String,
        tool: String,
        access: Access,
        sandbox: Option<String>,
        arguments: Value,
        /// Bound on how long the probe waits post-cancel for a response
        /// the server must never send. 1..=60 seconds.
        grace_seconds: u64,
        /// Free-form reason recorded in the notification; identifier-shaped.
        reason: String,
    },
    #[serde(rename = "protocol-negotiation")]
    ProtocolNegotiation {
        id: String,
        access: Access,
        /// The version the probe offers when checking echo behavior; the
        /// server must reject an unknown version instead of parroting it.
        bogus_version: String,
    },
    #[serde(rename = "sampling")]
    Sampling {
        id: String,
        tool: String,
        access: Access,
        sandbox: Option<String>,
        arguments: Value,
        /// Upper bound on `sampling/createMessage` requests the server may
        /// issue during this one tool call. 1..=10.
        max_requests: u64,
    },
    #[serde(rename = "elicitation")]
    Elicitation {
        id: String,
        tool: String,
        access: Access,
        sandbox: Option<String>,
        arguments: Value,
        /// Upper bound on `elicitation/create` requests during this call.
        /// 1..=10.
        max_requests: u64,
        /// How the probe answers each elicitation request.
        respond: ElicitationResponse,
    },
    #[serde(rename = "resource-subscription")]
    ResourceSubscription {
        id: String,
        access: Access,
        /// The resource URI to subscribe to; must be listed by
        /// `resources/list` unless `allow_undeclared` is set.
        uri: String,
        /// Tool whose call must trigger the update notification.
        trigger_tool: Option<String>,
        trigger_arguments: Option<Value>,
        /// Bound on how long the probe waits for the update notification.
        /// 1..=60 seconds.
        max_wait_seconds: u64,
    },
    #[serde(rename = "completion")]
    Completion {
        id: String,
        access: Access,
        /// The prompt or resource reference the completion is requested
        /// for: `{"type": "ref/prompt", "name": "..."}` or
        /// `{"type": "ref/resource", "uri": "..."}`.
        ref_uri: String,
        /// `ref/prompt` or `ref/resource`.
        ref_type: String,
        /// The argument whose completion is requested; must be declared
        /// by the referenced prompt.
        argument_name: String,
        /// Prefix text the probe offers. Identifier-shaped.
        argument_value: String,
        /// Upper bound on completion values the probe accepts per
        /// request. 1..=100.
        max_values: u64,
    },
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ElicitationResponse {
    /// Accept the elicitation with an empty action payload.
    Accept,
    /// Decline: the user declined the elicitation.
    Decline,
    /// Cancel: dismiss the elicitation.
    Cancel,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, clap::ValueEnum)]
pub enum ProbeKind {
    Contention,
    ErrorHonesty,
    StateRecovery,
    DiscoveryCost,
    TokenCost,
    SchemaGuessability,
    DegradationOverN,
    InstructionFidelity,
    LatencyBudget,
    Pagination,
    PayloadBounds,
    SurfaceListing,
    OutputSchema,
    Cancellation,
    ProtocolNegotiation,
    Sampling,
    Elicitation,
    ResourceSubscription,
    Completion,
}

impl ProbeKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Contention => "contention",
            Self::ErrorHonesty => "error-honesty",
            Self::StateRecovery => "state-recovery",
            Self::DiscoveryCost => "discovery-cost",
            Self::TokenCost => "token-cost",
            Self::SchemaGuessability => "schema-guessability",
            Self::DegradationOverN => "degradation-over-n",
            Self::InstructionFidelity => "instruction-fidelity",
            Self::LatencyBudget => "latency-budget",
            Self::Pagination => "pagination",
            Self::PayloadBounds => "payload-bounds",
            Self::SurfaceListing => "surface-listing",
            Self::OutputSchema => "output-schema",
            Self::Cancellation => "cancellation",
            Self::ProtocolNegotiation => "protocol-negotiation",
            Self::Sampling => "sampling",
            Self::Elicitation => "elicitation",
            Self::ResourceSubscription => "resource-subscription",
            Self::Completion => "completion",
        }
    }
}

impl ProbeCase {
    pub fn id(&self) -> &str {
        match self {
            Self::Contention { id, .. }
            | Self::ErrorHonesty { id, .. }
            | Self::StateRecovery { id, .. }
            | Self::DiscoveryCost { id, .. }
            | Self::TokenCost { id, .. }
            | Self::SchemaGuessability { id, .. }
            | Self::DegradationOverN { id, .. }
            | Self::InstructionFidelity { id, .. }
            | Self::LatencyBudget { id, .. }
            | Self::Pagination { id, .. }
            | Self::PayloadBounds { id, .. }
            | Self::SurfaceListing { id, .. }
            | Self::OutputSchema { id, .. }
            | Self::Cancellation { id, .. }
            | Self::ProtocolNegotiation { id, .. }
            | Self::Sampling { id, .. }
            | Self::Elicitation { id, .. }
            | Self::ResourceSubscription { id, .. }
            | Self::Completion { id, .. } => id,
        }
    }

    pub fn tool(&self) -> Option<&str> {
        match self {
            Self::DiscoveryCost { .. }
            | Self::TokenCost { .. }
            | Self::Pagination { .. }
            | Self::SurfaceListing { .. }
            | Self::ProtocolNegotiation { .. }
            | Self::ResourceSubscription { .. }
            | Self::Completion { .. } => None,
            Self::Contention { tool, .. } => Some(tool),
            Self::ErrorHonesty { tool, .. } => Some(tool),
            Self::StateRecovery { failure_tool, .. } => Some(failure_tool),
            Self::SchemaGuessability { tool, .. }
            | Self::DegradationOverN { tool, .. }
            | Self::InstructionFidelity { tool, .. }
            | Self::LatencyBudget { tool, .. }
            | Self::PayloadBounds { tool, .. }
            | Self::OutputSchema { tool, .. }
            | Self::Cancellation { tool, .. }
            | Self::Sampling { tool, .. }
            | Self::Elicitation { tool, .. } => Some(tool),
        }
    }

    pub fn access(&self) -> Access {
        match self {
            Self::Contention { access, .. }
            | Self::ErrorHonesty { access, .. }
            | Self::StateRecovery { access, .. }
            | Self::DiscoveryCost { access, .. }
            | Self::TokenCost { access, .. }
            | Self::SchemaGuessability { access, .. }
            | Self::DegradationOverN { access, .. }
            | Self::InstructionFidelity { access, .. }
            | Self::LatencyBudget { access, .. }
            | Self::Pagination { access, .. }
            | Self::PayloadBounds { access, .. }
            | Self::SurfaceListing { access, .. }
            | Self::OutputSchema { access, .. }
            | Self::Cancellation { access, .. }
            | Self::ProtocolNegotiation { access, .. }
            | Self::Sampling { access, .. }
            | Self::Elicitation { access, .. }
            | Self::ResourceSubscription { access, .. }
            | Self::Completion { access, .. } => *access,
        }
    }

    pub fn sandbox(&self) -> Option<&str> {
        match self {
            Self::DiscoveryCost { .. }
            | Self::TokenCost { .. }
            | Self::Pagination { .. }
            | Self::SurfaceListing { .. }
            | Self::ProtocolNegotiation { .. }
            | Self::ResourceSubscription { .. }
            | Self::Completion { .. } => None,
            Self::Contention { sandbox, .. } => sandbox.as_deref(),
            Self::ErrorHonesty { sandbox, .. } | Self::StateRecovery { sandbox, .. } => {
                sandbox.as_deref()
            }
            Self::SchemaGuessability { sandbox, .. }
            | Self::DegradationOverN { sandbox, .. }
            | Self::InstructionFidelity { sandbox, .. }
            | Self::LatencyBudget { sandbox, .. }
            | Self::PayloadBounds { sandbox, .. }
            | Self::OutputSchema { sandbox, .. }
            | Self::Cancellation { sandbox, .. }
            | Self::Sampling { sandbox, .. }
            | Self::Elicitation { sandbox, .. } => sandbox.as_deref(),
        }
    }

    pub fn arguments(&self) -> Option<&Value> {
        match self {
            Self::DiscoveryCost { .. }
            | Self::TokenCost { .. }
            | Self::StateRecovery { .. }
            | Self::Pagination { .. }
            | Self::SurfaceListing { .. }
            | Self::ProtocolNegotiation { .. }
            | Self::ResourceSubscription { .. }
            | Self::Completion { .. } => None,
            Self::Contention { arguments, .. } => Some(arguments),
            Self::ErrorHonesty { arguments, .. } => Some(arguments),
            Self::SchemaGuessability { arguments, .. }
            | Self::DegradationOverN { arguments, .. }
            | Self::InstructionFidelity { arguments, .. }
            | Self::LatencyBudget { arguments, .. }
            | Self::PayloadBounds { arguments, .. }
            | Self::OutputSchema { arguments, .. }
            | Self::Cancellation { arguments, .. }
            | Self::Sampling { arguments, .. }
            | Self::Elicitation { arguments, .. } => Some(arguments),
        }
    }

    pub fn kind(&self) -> ProbeKind {
        match self {
            Self::Contention { .. } => ProbeKind::Contention,
            Self::ErrorHonesty { .. } => ProbeKind::ErrorHonesty,
            Self::StateRecovery { .. } => ProbeKind::StateRecovery,
            Self::DiscoveryCost { .. } => ProbeKind::DiscoveryCost,
            Self::TokenCost { .. } => ProbeKind::TokenCost,
            Self::SchemaGuessability { .. } => ProbeKind::SchemaGuessability,
            Self::DegradationOverN { .. } => ProbeKind::DegradationOverN,
            Self::InstructionFidelity { .. } => ProbeKind::InstructionFidelity,
            Self::LatencyBudget { .. } => ProbeKind::LatencyBudget,
            Self::Pagination { .. } => ProbeKind::Pagination,
            Self::PayloadBounds { .. } => ProbeKind::PayloadBounds,
            Self::SurfaceListing { .. } => ProbeKind::SurfaceListing,
            Self::OutputSchema { .. } => ProbeKind::OutputSchema,
            Self::Cancellation { .. } => ProbeKind::Cancellation,
            Self::ProtocolNegotiation { .. } => ProbeKind::ProtocolNegotiation,
            Self::Sampling { .. } => ProbeKind::Sampling,
            Self::Elicitation { .. } => ProbeKind::Elicitation,
            Self::ResourceSubscription { .. } => ProbeKind::ResourceSubscription,
            Self::Completion { .. } => ProbeKind::Completion,
        }
    }

    pub fn max_attempts(&self) -> Option<u64> {
        match self {
            Self::DegradationOverN { max_attempts, .. }
            | Self::ErrorHonesty { max_attempts, .. }
            | Self::LatencyBudget {
                attempts: max_attempts,
                ..
            } => Some(*max_attempts),
            _ => None,
        }
    }

    pub fn required_tools(&self) -> Vec<&str> {
        match self {
            Self::DiscoveryCost { .. }
            | Self::TokenCost { .. }
            | Self::Pagination { .. }
            | Self::SurfaceListing { .. }
            | Self::ProtocolNegotiation { .. } => Vec::new(),
            Self::Completion { .. } => Vec::new(),
            Self::ResourceSubscription {
                trigger_tool: None, ..
            } => Vec::new(),
            Self::StateRecovery {
                failure_tool,
                recovery_tool,
                validation_tool,
                ..
            } => vec![failure_tool, recovery_tool, validation_tool],
            Self::ResourceSubscription {
                trigger_tool: Some(trigger_tool),
                ..
            } => vec![trigger_tool],
            _ => vec![self.tool().expect("tool probe has a primary tool")],
        }
    }

    pub fn expectation(&self) -> Option<&Expectation> {
        match self {
            Self::InstructionFidelity { expect, .. } => Some(expect),
            _ => None,
        }
    }
}

impl Manifest {
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        Self::parse(&Self::read(path)?)
    }

    /// The manifest bytes at `path`; a missing file names the path and the
    /// command that scaffolds one.
    pub fn read(path: &Path) -> anyhow::Result<Vec<u8>> {
        std::fs::read(path).map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                anyhow::anyhow!(
                    "manifest {} not found; run mcpeval init to scaffold one",
                    path.display()
                )
            } else {
                anyhow::Error::new(error).context(format!("reading manifest {}", path.display()))
            }
        })
    }

    /// Parse and validate manifest bytes.
    pub fn parse(body: &[u8]) -> anyhow::Result<Self> {
        let manifest: Self = serde_json::from_slice(body).context("parsing manifest structure")?;
        manifest.validate()?;
        Ok(manifest)
    }

    pub fn validate(&self) -> anyhow::Result<()> {
        if self.version != 1 {
            bail!("manifest version must be 1");
        }
        if self
            .timeout_ms
            .is_some_and(|timeout_ms| !(100..=600_000).contains(&timeout_ms))
        {
            bail!("manifest timeout_ms must be between 100 and 600000");
        }
        for (name, sandbox) in &self.sandboxes {
            if !privacy::valid_identifier(name) {
                bail!("sandbox name is invalid");
            }
            if sandbox.description.chars().count() > 240
                || sandbox.description.chars().any(char::is_control)
            {
                bail!("sandbox description is invalid");
            }
        }
        if self.probes.is_empty() {
            bail!("manifest must contain at least one probe case");
        }
        let mut ids = HashSet::new();
        for case in &self.probes {
            // An invalid id is not share-safe, so it cannot name its case.
            if !privacy::valid_identifier(case.id()) {
                bail!("probe id is invalid");
            }
            self.validate_case(case, &mut ids)
                .map_err(|error| anyhow::anyhow!("probe case {}: {error:#}", case.id()))?;
        }
        Ok(())
    }

    fn validate_case<'m>(
        &'m self,
        case: &'m ProbeCase,
        ids: &mut HashSet<&'m str>,
    ) -> anyhow::Result<()> {
        if !ids.insert(case.id()) {
            bail!("probe ids must be unique");
        }
        if case
            .required_tools()
            .iter()
            .any(|tool| !privacy::valid_tool(tool))
        {
            bail!("probe tool is invalid");
        }
        if case
            .arguments()
            .is_some_and(|arguments| !arguments.is_object())
        {
            bail!("probe arguments must be an object");
        }
        match (case.access(), case.sandbox()) {
            (Access::ReadOnly, None) => {}
            (Access::ReadOnly, Some(_)) => bail!("read-only probe must not name a sandbox"),
            (Access::Mutating, None) => bail!("mutating probe must name a sandbox"),
            (Access::Mutating, Some(name)) if !self.sandboxes.contains_key(name) => {
                bail!("mutating probe sandbox is not declared")
            }
            (Access::Mutating, Some(_)) => {}
        }
        match case {
            ProbeCase::Contention { .. } => {}
            ProbeCase::ErrorHonesty { max_attempts, .. } => {
                if !(2..=20).contains(max_attempts) {
                    bail!("error-honesty max_attempts must be between 2 and 20");
                }
            }
            ProbeCase::StateRecovery {
                failure_arguments,
                recovery_arguments,
                validation_arguments,
                ..
            } => {
                if !failure_arguments.is_object()
                    || !recovery_arguments.is_object()
                    || !validation_arguments.is_object()
                {
                    bail!("state-recovery arguments must be objects");
                }
            }
            ProbeCase::DiscoveryCost {
                access,
                max_tools,
                max_schema_bytes,
                ..
            } => {
                if *access != Access::ReadOnly {
                    bail!("discovery-cost must be read-only");
                }
                if !(1..=10_000).contains(max_tools) || !(1..=10_000_000).contains(max_schema_bytes)
                {
                    bail!("discovery limits are out of range");
                }
            }
            ProbeCase::TokenCost {
                access,
                max_total_tokens,
                max_tool_tokens,
                ..
            } => {
                if *access != Access::ReadOnly {
                    bail!("token-cost must be read-only");
                }
                if !(1..=1_000_000).contains(max_total_tokens) {
                    bail!("token budget is out of range");
                }
                if let Some(max_tool_tokens) = max_tool_tokens {
                    if !(1..=100_000).contains(max_tool_tokens) {
                        bail!("per-tool token budget is out of range");
                    }
                    if max_tool_tokens > max_total_tokens {
                        bail!("per-tool token budget exceeds the total budget");
                    }
                }
            }
            ProbeCase::SchemaGuessability { .. } => {}
            ProbeCase::DegradationOverN { max_attempts, .. } => {
                if !(2..=100).contains(max_attempts) {
                    bail!("max_attempts must be between 2 and 100");
                }
            }
            ProbeCase::LatencyBudget {
                access,
                attempts,
                max_latency_ms,
                ..
            } => {
                if *access != Access::ReadOnly {
                    bail!("latency-budget must be read-only");
                }
                if !(2..=20).contains(attempts) {
                    bail!("latency-budget attempts must be between 2 and 20");
                }
                if !(1..=600_000).contains(max_latency_ms) {
                    bail!("latency budget is out of range");
                }
            }
            ProbeCase::Pagination {
                access, max_pages, ..
            } => {
                if *access != Access::ReadOnly {
                    bail!("pagination must be read-only");
                }
                if !(1..=1000).contains(max_pages) {
                    bail!("pagination max_pages must be between 1 and 1000");
                }
            }
            ProbeCase::PayloadBounds {
                access,
                field,
                size_bytes,
                ..
            } => {
                if *access != Access::ReadOnly {
                    bail!("payload-bounds must be read-only");
                }
                if !privacy::valid_identifier(field) {
                    bail!("payload field is invalid");
                }
                if !(1..=16_000_000).contains(size_bytes) {
                    bail!("payload size is out of range");
                }
            }
            ProbeCase::SurfaceListing {
                access, max_pages, ..
            } => {
                if *access != Access::ReadOnly {
                    bail!("surface-listing must be read-only");
                }
                if !(1..=1000).contains(max_pages) {
                    bail!("surface-listing max_pages must be between 1 and 1000");
                }
            }
            ProbeCase::OutputSchema { access, .. } => {
                if *access != Access::ReadOnly {
                    bail!("output-schema must be read-only");
                }
            }
            ProbeCase::Cancellation {
                access,
                grace_seconds,
                reason,
                ..
            } => {
                if *access != Access::ReadOnly {
                    bail!("cancellation must be read-only");
                }
                if !(1..=60).contains(grace_seconds) {
                    bail!("cancellation grace_seconds must be between 1 and 60");
                }
                if !privacy::valid_identifier(reason) {
                    bail!("cancellation reason is invalid");
                }
            }
            ProbeCase::ProtocolNegotiation {
                access,
                bogus_version,
                ..
            } => {
                if *access != Access::ReadOnly {
                    bail!("protocol-negotiation must be read-only");
                }
                // The offered bogus version must be date-shaped so the
                // case asserts version selection, not envelope junk.
                if bogus_version.len() != 10
                    || !bogus_version
                        .bytes()
                        .enumerate()
                        .all(|(index, byte)| match index {
                            4 | 7 => byte == b'-',
                            _ => byte.is_ascii_digit(),
                        })
                    || bogus_version == crate::http_client::PROTOCOL_VERSION
                {
                    bail!("protocol-negotiation bogus_version must be a date-shaped version other than the supported one");
                }
            }
            ProbeCase::Sampling {
                access,
                max_requests,
                ..
            } => {
                if *access != Access::ReadOnly {
                    bail!("sampling must be read-only");
                }
                if !(1..=10).contains(max_requests) {
                    bail!("sampling max_requests must be between 1 and 10");
                }
            }
            ProbeCase::Elicitation {
                access,
                max_requests,
                ..
            } => {
                if *access != Access::ReadOnly {
                    bail!("elicitation must be read-only");
                }
                if !(1..=10).contains(max_requests) {
                    bail!("elicitation max_requests must be between 1 and 10");
                }
            }
            ProbeCase::ResourceSubscription {
                access,
                uri,
                trigger_arguments,
                max_wait_seconds,
                ..
            } => {
                if *access != Access::ReadOnly {
                    bail!("resource-subscription must be read-only");
                }
                if uri.is_empty() || uri.len() > 512 {
                    bail!("resource-subscription uri is invalid");
                }
                if trigger_arguments
                    .as_ref()
                    .is_some_and(|arguments| !arguments.is_object())
                {
                    bail!("resource-subscription trigger arguments must be an object");
                }
                if !(1..=60).contains(max_wait_seconds) {
                    bail!("resource-subscription max_wait_seconds must be between 1 and 60");
                }
            }
            ProbeCase::Completion {
                access,
                ref_uri,
                ref_type,
                argument_name,
                argument_value,
                max_values,
                ..
            } => {
                if *access != Access::ReadOnly {
                    bail!("completion must be read-only");
                }
                if ref_type != "ref/prompt" && ref_type != "ref/resource" {
                    bail!("completion ref_type must be ref/prompt or ref/resource");
                }
                if ref_uri.is_empty() || ref_uri.len() > 512 {
                    bail!("completion ref_uri is invalid");
                }
                if ref_type == "ref/prompt" && !privacy::valid_identifier(argument_name) {
                    bail!("completion argument_name is invalid");
                }
                if !privacy::valid_identifier(argument_value) {
                    bail!("completion argument_value is invalid");
                }
                if !(1..=100).contains(max_values) {
                    bail!("completion max_values must be between 1 and 100");
                }
            }
            ProbeCase::InstructionFidelity { expect, .. } => validate_expectation(expect)?,
        }
        Ok(())
    }
}

fn validate_expectation(expect: &Expectation) -> anyhow::Result<()> {
    let mut fields = HashSet::new();
    for field in &expect.required_result_fields {
        if !privacy::valid_identifier(field) {
            bail!("expected result field is invalid");
        }
        if !fields.insert(field) {
            bail!("expected result fields must be unique");
        }
    }
    for field in expect.equals.keys() {
        if !privacy::valid_identifier(field) {
            bail!("expected result field is invalid");
        }
    }
    for value in expect.equals.values() {
        let valid = match value {
            Value::Null | Value::Bool(_) | Value::Number(_) => true,
            Value::String(value) => privacy::valid_identifier(value),
            Value::Array(_) | Value::Object(_) => false,
        };
        if !valid {
            bail!("expected equality value is invalid");
        }
    }
    match expect.outcome {
        OutcomeExpectation::Ok if expect.error_code.is_some() => {
            bail!("ok expectation must not declare error_code")
        }
        OutcomeExpectation::Error
            if !expect.required_result_fields.is_empty() || !expect.equals.is_empty() =>
        {
            bail!("error expectation must not declare result fields")
        }
        _ => Ok(()),
    }
}
