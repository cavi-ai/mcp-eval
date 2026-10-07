//! Report data and deterministic serialization, independent of execution.
use super::FailureReason;
use crate::manifest::{ProbeCase, ProbeKind};
use anyhow::{bail, Context};

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

impl TokenUsage {
    /// The three heaviest tools as `tool tokens` pairs, heaviest first.
    pub fn heaviest(&self) -> String {
        self.per_tool
            .iter()
            .take(3)
            .map(|tool| format!("{} {}", tool.tool, tool.tokens))
            .collect::<Vec<_>>()
            .join(", ")
    }
}

/// The manifest bound a failing case exceeded, beside the observed value:
/// share-safe numbers only.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BoundDetail {
    /// The manifest field that set the limit, one of [`BoundDetail::BOUNDS`].
    pub bound: &'static str,
    pub limit: u64,
    pub observed: u64,
}

impl BoundDetail {
    /// Every manifest field a bound-based failure can name.
    pub const BOUNDS: [&'static str; 6] = [
        "max_tools",
        "max_schema_bytes",
        "max_total_tokens",
        "max_tool_tokens",
        "max_latency_ms",
        "max_pages",
    ];

    /// The detail a report document carries, when it names a known bound.
    fn from_json(detail: &serde_json::Value) -> Option<Self> {
        let label = detail.get("bound")?.as_str()?;
        Some(Self {
            bound: Self::BOUNDS.into_iter().find(|bound| *bound == label)?,
            limit: detail.get("limit")?.as_u64()?,
            observed: detail.get("observed")?.as_u64()?,
        })
    }
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
    pub(super) fn for_case(case: &ProbeCase) -> Self {
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

    /// Context-aware guidance shared by every output surface. Workflow call
    /// numbers come from evaluated outcomes; payloads and assertion values do not.
    pub fn hint(&self) -> Option<String> {
        let reason = self.reason?;
        let hint = crate::remediation::hint(reason);
        if self.probe == ProbeKind::Workflow && !reason.is_transport() {
            if let Some(call) = self.first_failure {
                return Some(format!(
                    "Workflow call {call}: {hint}. Locate this call using first_failure and the \
                     manifest's ordered steps, then compare the same call in a fresh session. \
                     If it only fails after earlier steps, investigate leaked state or incomplete \
                     error recovery. Verify the repair with the full sequence."
                ));
            }
        }
        Some(hint.to_owned())
    }
}

#[derive(Debug, Default)]
pub struct ProbeReport {
    pub cases: Vec<CaseReport>,
    /// Lowercase hex SHA-256 of the manifest bytes the run parsed.
    pub manifest_sha256: Option<String>,
    /// The standard battery's readiness; None under --gate-only, for a
    /// selected probe or case, or for a v1 document.
    pub readiness: Option<crate::score::Readiness>,
    /// The standard battery could not finish: why.
    pub readiness_error: Option<FailureReason>,
    /// A v1 document's manifest pass rate, kept for display only.
    pub legacy_score: Option<u64>,
}

impl ProbeReport {
    /// Reconstruct a report from its `mcpeval.probe-report/v1` or `/v2`
    /// document — the committed-baseline format. Measurements are restored where the
    /// document carries them; the reconstructed report renders text,
    /// markdown, and SARIF identically to the run that produced it.
    pub fn from_json_document(document: &serde_json::Value) -> anyhow::Result<Self> {
        let v2 = match document.get("schema").and_then(serde_json::Value::as_str) {
            Some("mcpeval.probe-report/v2") => true,
            Some("mcpeval.probe-report/v1") => false,
            _ => bail!("document is not an mcpeval.probe-report/v1 or /v2 report"),
        };
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
                detail: case.get("detail").and_then(BoundDetail::from_json),
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
        let (readiness, readiness_error, legacy_score) = if v2 {
            let readiness = match document.get("readiness") {
                None | Some(serde_json::Value::Null) => None,
                Some(value) => Some(crate::score::Readiness::from_json(value)?),
            };
            let readiness_error = match document.get("readiness_error") {
                None | Some(serde_json::Value::Null) => None,
                Some(label) => Some(
                    FailureReason::from_report_label(
                        label.as_str().context("readiness_error must be a string")?,
                    )
                    .with_context(|| format!("unknown reason label {label}"))?,
                ),
            };
            (readiness, readiness_error, None)
        } else {
            let legacy = document
                .get("readiness")
                .and_then(|readiness| readiness.get("score"))
                .and_then(serde_json::Value::as_u64);
            (None, None, legacy)
        };
        Ok(Self {
            cases: parsed,
            manifest_sha256: document
                .get("manifest_sha256")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned),
            readiness,
            readiness_error,
            legacy_score,
        })
    }

    pub fn passed(&self) -> bool {
        self.cases.iter().all(CaseReport::passed)
    }

    /// Manifest cases passed and declared; None when no manifest ran.
    pub fn gate(&self) -> Option<(u64, u64)> {
        (self.manifest_sha256.is_some() || !self.cases.is_empty()).then(|| {
            (
                self.cases.iter().filter(|case| case.passed()).count() as u64,
                self.cases.len() as u64,
            )
        })
    }

    /// Some case, or the standard battery, could not be evaluated, so the
    /// run is incomplete.
    pub fn errored(&self) -> bool {
        self.cases.iter().any(CaseReport::errored) || self.readiness_error.is_some()
    }

    /// Versioned, deterministic JSON document: no timestamps, no session
    /// identifiers, cases in manifest order. Contains only share-safe
    /// fields — the generator, server label, manifest hash, case IDs, probe
    /// kinds, tool names, counts, fixed reason labels with their static
    /// remediation hints, declared bounds, measurement numbers, the gate
    /// counts, and the standard readiness object. Suitable for CI
    /// artifacts and committed baselines.
    /// `docs/mcp-eval.probe-report.schema.json` describes it
    /// (`mcpeval.probe-report/v2`).
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
                    "hint": case.hint(),
                    "detail": case.detail.map(|detail| serde_json::json!({
                        "bound": detail.bound,
                        "limit": detail.limit,
                        "observed": detail.observed,
                    })),
                    "measurements": serde_json::Value::Object(measurements),
                })
            })
            .collect();
        serde_json::json!({
            "schema": "mcpeval.probe-report/v2",
            "generator": {"name": "mcpeval", "version": env!("CARGO_PKG_VERSION")},
            "server": server,
            "manifest_sha256": self.manifest_sha256,
            "passed": self.passed(),
            "gate": self.gate().map(|(passed, total)| serde_json::json!({"passed": passed, "total": total})),
            "readiness": self.readiness.as_ref().map(crate::score::Readiness::to_json),
            "readiness_error": self.readiness_error.map(|reason| reason.as_str()),
            "cases": cases,
        })
    }
}
