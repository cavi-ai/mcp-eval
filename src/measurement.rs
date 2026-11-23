//! Public, content-minimized conditions required for readiness comparisons.
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MeasurementProfile {
    pub schema: String,
    pub evaluator_version: String,
    pub platform: String,
    pub architecture: String,
    pub client_capabilities: String,
    pub attested_read_only: bool,
    pub skip_tools: Vec<String>,
    pub call_timeout_ms: u64,
    pub repeats: usize,
    /// SHA-256 of the `--arguments` file the battery called tools with.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arguments_sha256: Option<String>,
    /// The server ran in a write-contained sandbox and every tool was called.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub sandboxed: bool,
    /// The sandbox allowed network access (`--sandbox-network`).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub sandbox_network: bool,
}

/// The conditions of one readiness run. The profile is this value; nothing
/// fills a field in afterwards.
pub struct MeasurementConditions<'a> {
    pub attested_read_only: bool,
    pub skip_tools: &'a [String],
    pub arguments_sha256: Option<&'a str>,
    pub sandboxed: bool,
    pub sandbox_network: bool,
    pub repeats: usize,
}

impl MeasurementProfile {
    /// Build a profile with the standard repeat count and no sandbox.
    pub fn current(
        attested_read_only: bool,
        skip_tools: &[String],
        arguments_sha256: Option<&str>,
    ) -> Self {
        Self::from_conditions(MeasurementConditions {
            attested_read_only,
            skip_tools,
            arguments_sha256,
            sandboxed: false,
            sandbox_network: false,
            repeats: crate::standard::REPEATS,
        })
    }

    pub fn from_conditions(conditions: MeasurementConditions<'_>) -> Self {
        let mut skip_tools = conditions.skip_tools.to_vec();
        skip_tools.sort();
        skip_tools.dedup();
        Self {
            schema: "mcpeval.measurement-profile/v1".into(),
            evaluator_version: env!("CARGO_PKG_VERSION").into(),
            platform: std::env::consts::OS.into(),
            architecture: std::env::consts::ARCH.into(),
            client_capabilities: "none".into(),
            attested_read_only: conditions.attested_read_only,
            skip_tools,
            call_timeout_ms: crate::standard::CALL_TIMEOUT.as_millis() as u64,
            repeats: conditions.repeats,
            arguments_sha256: conditions.arguments_sha256.map(str::to_owned),
            sandboxed: conditions.sandboxed,
            sandbox_network: conditions.sandbox_network,
        }
    }

    pub fn validate(&self) -> anyhow::Result<()> {
        static VERSION: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
        anyhow::ensure!(
            self.schema == "mcpeval.measurement-profile/v1",
            "unsupported measurement profile"
        );
        anyhow::ensure!(
            self.evaluator_version.len() <= 64
                && VERSION
                    .get_or_init(|| regex::Regex::new(
                        r"^[0-9]+\.[0-9]+\.[0-9]+(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?$"
                    )
                    .expect("fixed evaluator version pattern"))
                    .is_match(&self.evaluator_version),
            "invalid measurement evaluator version"
        );
        anyhow::ensure!(
            crate::privacy::valid_identifier(&self.platform)
                && crate::privacy::valid_identifier(&self.architecture)
                && self.client_capabilities == "none"
                && self.call_timeout_ms > 0
                && self.repeats > 0,
            "invalid measurement conditions"
        );
        anyhow::ensure!(
            self.skip_tools
                .iter()
                .all(|tool| crate::privacy::valid_tool(tool))
                && self.skip_tools.windows(2).all(|pair| pair[0] < pair[1]),
            "invalid measurement skip tools"
        );
        anyhow::ensure!(
            self.arguments_sha256.as_ref().is_none_or(|digest| {
                digest.len() == 64
                    && digest
                        .bytes()
                        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            }),
            "invalid measurement arguments digest"
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_conditions_records_the_run_it_was_given() {
        let profile = MeasurementProfile::from_conditions(MeasurementConditions {
            attested_read_only: true,
            skip_tools: &["b".into(), "a".into(), "a".into()],
            arguments_sha256: Some("ab"),
            sandboxed: true,
            sandbox_network: true,
            repeats: 7,
        });
        assert!(profile.attested_read_only);
        assert_eq!(profile.skip_tools, ["a", "b"]);
        assert_eq!(profile.arguments_sha256.as_deref(), Some("ab"));
        assert!(profile.sandboxed);
        assert!(profile.sandbox_network);
        assert_eq!(profile.repeats, 7);
    }
}
