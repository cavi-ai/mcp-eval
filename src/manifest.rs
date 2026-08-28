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
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
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
            | Self::OutputSchema { id, .. } => id,
        }
    }

    pub fn tool(&self) -> Option<&str> {
        match self {
            Self::DiscoveryCost { .. }
            | Self::TokenCost { .. }
            | Self::Pagination { .. }
            | Self::SurfaceListing { .. } => None,
            Self::Contention { tool, .. } => Some(tool),
            Self::ErrorHonesty { tool, .. } => Some(tool),
            Self::StateRecovery { failure_tool, .. } => Some(failure_tool),
            Self::SchemaGuessability { tool, .. }
            | Self::DegradationOverN { tool, .. }
            | Self::InstructionFidelity { tool, .. }
            | Self::LatencyBudget { tool, .. }
            | Self::PayloadBounds { tool, .. }
            | Self::OutputSchema { tool, .. } => Some(tool),
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
            | Self::OutputSchema { access, .. } => *access,
        }
    }

    pub fn sandbox(&self) -> Option<&str> {
        match self {
            Self::DiscoveryCost { .. }
            | Self::TokenCost { .. }
            | Self::Pagination { .. }
            | Self::SurfaceListing { .. } => None,
            Self::Contention { sandbox, .. } => sandbox.as_deref(),
            Self::ErrorHonesty { sandbox, .. } | Self::StateRecovery { sandbox, .. } => {
                sandbox.as_deref()
            }
            Self::SchemaGuessability { sandbox, .. }
            | Self::DegradationOverN { sandbox, .. }
            | Self::InstructionFidelity { sandbox, .. }
            | Self::LatencyBudget { sandbox, .. }
            | Self::PayloadBounds { sandbox, .. }
            | Self::OutputSchema { sandbox, .. } => sandbox.as_deref(),
        }
    }

    pub fn arguments(&self) -> Option<&Value> {
        match self {
            Self::DiscoveryCost { .. }
            | Self::TokenCost { .. }
            | Self::StateRecovery { .. }
            | Self::Pagination { .. }
            | Self::SurfaceListing { .. } => None,
            Self::Contention { arguments, .. } => Some(arguments),
            Self::ErrorHonesty { arguments, .. } => Some(arguments),
            Self::SchemaGuessability { arguments, .. }
            | Self::DegradationOverN { arguments, .. }
            | Self::InstructionFidelity { arguments, .. }
            | Self::LatencyBudget { arguments, .. }
            | Self::PayloadBounds { arguments, .. }
            | Self::OutputSchema { arguments, .. } => Some(arguments),
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
            | Self::SurfaceListing { .. } => Vec::new(),
            Self::StateRecovery {
                failure_tool,
                recovery_tool,
                validation_tool,
                ..
            } => vec![failure_tool, recovery_tool, validation_tool],
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
        let body = std::fs::read(path).context("reading manifest")?;
        let manifest: Self = serde_json::from_slice(&body).context("parsing manifest structure")?;
        manifest.validate()?;
        Ok(manifest)
    }

    pub fn validate(&self) -> anyhow::Result<()> {
        if self.version != 1 {
            bail!("manifest version must be 1");
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
            if !privacy::valid_identifier(case.id()) {
                bail!("probe id is invalid");
            }
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
                    if !(1..=10_000).contains(max_tools)
                        || !(1..=10_000_000).contains(max_schema_bytes)
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
                ProbeCase::InstructionFidelity { expect, .. } => validate_expectation(expect)?,
            }
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
