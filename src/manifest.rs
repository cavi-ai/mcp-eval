use std::collections::{BTreeMap, HashSet};
use std::path::Path;

use anyhow::{bail, Context};
use serde::Deserialize;
use serde_json::Value;

use crate::privacy;

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub version: u64,
    #[serde(default)]
    pub sandboxes: BTreeMap<String, Sandbox>,
    pub probes: Vec<ProbeCase>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Sandbox {
    pub description: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum Access {
    ReadOnly,
    Mutating,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
pub enum OutcomeExpectation {
    #[serde(rename = "ok")]
    Ok,
    #[serde(rename = "error")]
    Error,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Expectation {
    pub outcome: OutcomeExpectation,
    #[serde(default)]
    pub required_result_fields: Vec<String>,
    #[serde(default)]
    pub equals: BTreeMap<String, Value>,
    pub error_code: Option<i64>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "probe", deny_unknown_fields)]
pub enum ProbeCase {
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
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProbeKind {
    DegradationOverN,
    InstructionFidelity,
}

impl ProbeCase {
    pub fn id(&self) -> &str {
        match self {
            Self::DegradationOverN { id, .. } | Self::InstructionFidelity { id, .. } => id,
        }
    }

    pub fn tool(&self) -> &str {
        match self {
            Self::DegradationOverN { tool, .. } | Self::InstructionFidelity { tool, .. } => tool,
        }
    }

    pub fn access(&self) -> Access {
        match self {
            Self::DegradationOverN { access, .. } | Self::InstructionFidelity { access, .. } => {
                *access
            }
        }
    }

    pub fn sandbox(&self) -> Option<&str> {
        match self {
            Self::DegradationOverN { sandbox, .. } | Self::InstructionFidelity { sandbox, .. } => {
                sandbox.as_deref()
            }
        }
    }

    pub fn arguments(&self) -> &Value {
        match self {
            Self::DegradationOverN { arguments, .. }
            | Self::InstructionFidelity { arguments, .. } => arguments,
        }
    }

    pub fn kind(&self) -> ProbeKind {
        match self {
            Self::DegradationOverN { .. } => ProbeKind::DegradationOverN,
            Self::InstructionFidelity { .. } => ProbeKind::InstructionFidelity,
        }
    }

    pub fn max_attempts(&self) -> Option<u64> {
        match self {
            Self::DegradationOverN { max_attempts, .. } => Some(*max_attempts),
            Self::InstructionFidelity { .. } => None,
        }
    }

    pub fn expectation(&self) -> Option<&Expectation> {
        match self {
            Self::InstructionFidelity { expect, .. } => Some(expect),
            Self::DegradationOverN { .. } => None,
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
            if !privacy::valid_tool(case.tool()) {
                bail!("probe tool is invalid");
            }
            if !case.arguments().is_object() {
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
                ProbeCase::DegradationOverN { max_attempts, .. } => {
                    if !(2..=100).contains(max_attempts) {
                        bail!("max_attempts must be between 2 and 100");
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
