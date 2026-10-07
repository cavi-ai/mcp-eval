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
}

impl MeasurementProfile {
    pub fn current(attested_read_only: bool, skip_tools: &[String]) -> Self {
        let mut skip_tools = skip_tools.to_vec();
        skip_tools.sort();
        skip_tools.dedup();
        Self {
            schema: "mcpeval.measurement-profile/v1".into(),
            evaluator_version: env!("CARGO_PKG_VERSION").into(),
            platform: std::env::consts::OS.into(),
            architecture: std::env::consts::ARCH.into(),
            client_capabilities: "none".into(),
            attested_read_only,
            skip_tools,
            call_timeout_ms: crate::standard::CALL_TIMEOUT.as_millis() as u64,
            repeats: crate::standard::REPEATS,
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
        Ok(())
    }
}
