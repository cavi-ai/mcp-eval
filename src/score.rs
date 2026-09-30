//! Readiness under mcpeval's standard: an absolute 0-100 score mcpeval
//! defines, comparable across servers. The standard battery
//! ([`crate::standard`]) observes the server; this module folds those
//! observations into area scores on fixed curves and weights. Nothing a
//! manifest declares enters the score: the manifest is the gate.
//!
//! Every constant below is part of the standard. Changing a weight,
//! curve, band, or check changes what a score means and bumps [`STANDARD`].

use serde_json::{json, Value};

use crate::probe::ProbeReport;
use crate::standard::{Exercise, Observations, ToolClass, ToolObservation};

/// The standard this build scores against. `-draft` until every area of
/// standard/1 has landed; no release carries a draft.
pub const STANDARD: &str = "mcpeval-standard/1-draft";

/// Catalog tokens at or under this earn full marks (1% of a 200k window).
pub const CATALOG_FULL_TOKENS: u64 = 2_000;
/// Catalog tokens at or over this earn nothing (20% of a 200k window).
pub const CATALOG_ZERO_TOKENS: u64 = 40_000;
pub const TOOL_FULL_TOKENS: u64 = 500;
pub const TOOL_ZERO_TOKENS: u64 = 5_000;
/// Share of the context area carried by the catalog total; the heaviest
/// tool carries the rest.
const CATALOG_SHARE: f64 = 0.75;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Area {
    Context,
    Reliability,
    Coverage,
}

impl Area {
    /// Every area of the standard, in report order.
    pub const ALL: &'static [Area] = &[Area::Context, Area::Reliability, Area::Coverage];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Context => "context",
            Self::Reliability => "reliability",
            Self::Coverage => "coverage",
        }
    }

    /// Weight in the overall score; standard/1's six areas sum to 100.
    pub fn weight(self) -> u64 {
        match self {
            Self::Context => 15,
            Self::Reliability => 20,
            Self::Coverage => 15,
        }
    }

    pub fn from_label(label: &str) -> Option<Self> {
        Self::ALL
            .iter()
            .copied()
            .find(|area| area.as_str() == label)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CheckId {
    ContextCatalog,
    ContextHeaviestTool,
    ReliabilityConsistent,
    ReliabilityLatency,
    ReliabilityOutputSchema,
    ReliabilityContention,
    ReliabilityPayload,
    ReliabilityNoneExercised,
    CoverageExercised,
}

impl CheckId {
    pub const ALL: &'static [CheckId] = &[
        Self::ContextCatalog,
        Self::ContextHeaviestTool,
        Self::ReliabilityConsistent,
        Self::ReliabilityLatency,
        Self::ReliabilityOutputSchema,
        Self::ReliabilityContention,
        Self::ReliabilityPayload,
        Self::ReliabilityNoneExercised,
        Self::CoverageExercised,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::ContextCatalog => "context.catalog",
            Self::ContextHeaviestTool => "context.heaviest-tool",
            Self::ReliabilityConsistent => "reliability.consistent",
            Self::ReliabilityLatency => "reliability.latency",
            Self::ReliabilityOutputSchema => "reliability.output-schema",
            Self::ReliabilityContention => "reliability.contention",
            Self::ReliabilityPayload => "reliability.payload",
            Self::ReliabilityNoneExercised => "reliability.none-exercised",
            Self::CoverageExercised => "coverage.exercised",
        }
    }

    pub fn from_label(label: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|id| id.as_str() == label)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CheckReason {
    ContextCatalogHeavy,
    ContextToolHeavy,
    ReliabilityInconsistent,
    ReliabilitySlow,
    ReliabilityOutputSchemaBroken,
    ReliabilityContentionFailed,
    ReliabilityPayloadUnhandled,
    ReliabilityNoneExercised,
    CoverageUnannotated,
    CoverageRequiredArguments,
    CoverageRejectedArguments,
    CoverageCallFailed,
    NoReadOnlyTools,
}

impl CheckReason {
    pub const ALL: &'static [CheckReason] = &[
        Self::ContextCatalogHeavy,
        Self::ContextToolHeavy,
        Self::ReliabilityInconsistent,
        Self::ReliabilitySlow,
        Self::ReliabilityOutputSchemaBroken,
        Self::ReliabilityContentionFailed,
        Self::ReliabilityPayloadUnhandled,
        Self::ReliabilityNoneExercised,
        Self::CoverageUnannotated,
        Self::CoverageRequiredArguments,
        Self::CoverageRejectedArguments,
        Self::CoverageCallFailed,
        Self::NoReadOnlyTools,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::ContextCatalogHeavy => "context-catalog-heavy",
            Self::ContextToolHeavy => "context-tool-heavy",
            Self::ReliabilityInconsistent => "reliability-inconsistent",
            Self::ReliabilitySlow => "reliability-slow",
            Self::ReliabilityOutputSchemaBroken => "reliability-output-schema-broken",
            Self::ReliabilityContentionFailed => "reliability-contention-failed",
            Self::ReliabilityPayloadUnhandled => "reliability-payload-unhandled",
            Self::ReliabilityNoneExercised => "reliability-none-exercised",
            Self::CoverageUnannotated => "coverage-unannotated",
            Self::CoverageRequiredArguments => "coverage-required-arguments",
            Self::CoverageRejectedArguments => "coverage-rejected-arguments",
            Self::CoverageCallFailed => "coverage-call-failed",
            Self::NoReadOnlyTools => "no-read-only-tools",
        }
    }

    pub fn from_label(label: &str) -> Option<Self> {
        Self::ALL
            .iter()
            .copied()
            .find(|reason| reason.as_str() == label)
    }
}

/// A check that lost points. Checks at full marks are not listed.
#[derive(Clone, Debug, PartialEq)]
pub struct Check {
    pub id: CheckId,
    pub tool: Option<String>,
    /// What the check earned, 0-99.
    pub score: u64,
    /// The measurement behind a graded check: tokens or milliseconds.
    pub observed: Option<u64>,
    pub reason: CheckReason,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AreaScore {
    pub area: Area,
    pub score: u64,
    pub checks: Vec<Check>,
    /// Share-safe numbers behind the area (context: token counts).
    pub measurements: serde_json::Map<String, Value>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Surface {
    pub tools: u64,
    /// Every tool that is not a writer, annotated or not.
    pub read_only: u64,
    pub writers: u64,
    pub exercised: u64,
}

impl Surface {
    fn of(tools: &[ToolObservation]) -> Self {
        let writers = tools
            .iter()
            .filter(|tool| tool.class == ToolClass::Writer)
            .count() as u64;
        Self {
            tools: tools.len() as u64,
            read_only: tools.len() as u64 - writers,
            writers,
            exercised: tools
                .iter()
                .filter(|tool| matches!(tool.exercise, Some(Exercise::Exercised { .. })))
                .count() as u64,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Readiness {
    pub standard: String,
    pub score: u64,
    pub attested_read_only: bool,
    pub surface: Surface,
    pub areas: Vec<AreaScore>,
}

/// Fold the standard battery's observations into readiness. Every area in
/// [`Area::ALL`] is always scored: none drops out of the denominator.
pub fn fold(observations: &Observations) -> Readiness {
    let areas: Vec<AreaScore> = Area::ALL
        .iter()
        .map(|area| match area {
            Area::Context => context(&observations.tools),
            Area::Reliability => reliability(observations),
            Area::Coverage => coverage(&observations.tools),
        })
        .collect();
    let weights: u64 = areas.iter().map(|area| area.area.weight()).sum();
    let weighted: u64 = areas
        .iter()
        .map(|area| area.area.weight() * area.score)
        .sum();
    Readiness {
        standard: STANDARD.to_owned(),
        score: round(weighted as f64 / weights as f64),
        attested_read_only: observations.attested_read_only,
        surface: Surface::of(&observations.tools),
        areas,
    }
}

fn round(value: f64) -> u64 {
    value.round().clamp(0.0, 100.0) as u64
}

/// 100 at or under `full`, 0 at or over `zero`, logarithmic between.
pub fn log_curve(value: u64, full: u64, zero: u64) -> f64 {
    if value <= full {
        return 100.0;
    }
    if value >= zero {
        return 0.0;
    }
    let (value, full, zero) = (value as f64, full as f64, zero as f64);
    100.0 * (zero.ln() - value.ln()) / (zero.ln() - full.ln())
}

/// Median-latency bands, coarse so a rerun of one build lands in one band.
pub fn latency_band(median_ms: u64) -> u64 {
    match median_ms {
        0..=100 => 100,
        101..=300 => 80,
        301..=1_000 => 50,
        1_001..=3_000 => 20,
        _ => 0,
    }
}

/// The check, when it lost points.
fn lost(
    id: CheckId,
    tool: Option<&str>,
    score: f64,
    observed: Option<u64>,
    reason: CheckReason,
) -> Option<Check> {
    let score = round(score);
    (score < 100).then(|| Check {
        id,
        tool: tool.map(str::to_owned),
        score,
        observed,
        reason,
    })
}

fn read_only_surface(tools: &[ToolObservation]) -> usize {
    tools
        .iter()
        .filter(|tool| tool.class != ToolClass::Writer)
        .count()
}

fn context(tools: &[ToolObservation]) -> AreaScore {
    let catalog_tokens: u64 = tools.iter().map(|tool| tool.tokens).sum();
    // Heaviest first; a tie goes to the name that sorts first.
    let heaviest = tools
        .iter()
        .max_by(|a, b| a.tokens.cmp(&b.tokens).then_with(|| b.name.cmp(&a.name)));
    let heaviest_tokens = heaviest.map_or(0, |tool| tool.tokens);
    let catalog = log_curve(catalog_tokens, CATALOG_FULL_TOKENS, CATALOG_ZERO_TOKENS);
    let tool = log_curve(heaviest_tokens, TOOL_FULL_TOKENS, TOOL_ZERO_TOKENS);
    let checks = [
        lost(
            CheckId::ContextCatalog,
            None,
            catalog,
            Some(catalog_tokens),
            CheckReason::ContextCatalogHeavy,
        ),
        lost(
            CheckId::ContextHeaviestTool,
            heaviest.map(|tool| tool.name.as_str()),
            tool,
            Some(heaviest_tokens),
            CheckReason::ContextToolHeavy,
        ),
    ]
    .into_iter()
    .flatten()
    .collect();
    let mut measurements = serde_json::Map::new();
    measurements.insert("catalog_tokens".into(), catalog_tokens.into());
    measurements.insert("heaviest_tool_tokens".into(), heaviest_tokens.into());
    AreaScore {
        area: Area::Context,
        score: round(CATALOG_SHARE * catalog + (1.0 - CATALOG_SHARE) * tool),
        checks,
        measurements,
    }
}

fn reliability(observations: &Observations) -> AreaScore {
    let mut entries: Vec<f64> = Vec::new();
    let mut checks = Vec::new();
    for tool in &observations.tools {
        let Some(Exercise::Exercised {
            consistent,
            median_latency_ms,
            output_schema,
        }) = &tool.exercise
        else {
            continue;
        };
        let name = Some(tool.name.as_str());
        let consistency = if *consistent { 100.0 } else { 0.0 };
        let latency = latency_band(*median_latency_ms) as f64;
        checks.extend(lost(
            CheckId::ReliabilityConsistent,
            name,
            consistency,
            None,
            CheckReason::ReliabilityInconsistent,
        ));
        checks.extend(lost(
            CheckId::ReliabilityLatency,
            name,
            latency,
            Some(*median_latency_ms),
            CheckReason::ReliabilitySlow,
        ));
        let mut parts = vec![consistency, latency];
        if let Some(conforms) = output_schema {
            let conformance = if *conforms { 100.0 } else { 0.0 };
            parts.push(conformance);
            checks.extend(lost(
                CheckId::ReliabilityOutputSchema,
                name,
                conformance,
                None,
                CheckReason::ReliabilityOutputSchemaBroken,
            ));
        }
        entries.push(parts.iter().sum::<f64>() / parts.len() as f64);
    }
    let exercised = entries.len();
    for (outcome, id, reason) in [
        (
            observations.contention,
            CheckId::ReliabilityContention,
            CheckReason::ReliabilityContentionFailed,
        ),
        (
            observations.payload,
            CheckId::ReliabilityPayload,
            CheckReason::ReliabilityPayloadUnhandled,
        ),
    ] {
        if let Some(passed) = outcome {
            let value = if passed { 100.0 } else { 0.0 };
            entries.push(value);
            checks.extend(lost(id, None, value, None, reason));
        }
    }
    let score = if exercised == 0 {
        let reason = if read_only_surface(&observations.tools) == 0 {
            CheckReason::NoReadOnlyTools
        } else {
            CheckReason::ReliabilityNoneExercised
        };
        checks.extend(lost(
            CheckId::ReliabilityNoneExercised,
            None,
            0.0,
            None,
            reason,
        ));
        0
    } else {
        round(entries.iter().sum::<f64>() / entries.len() as f64)
    };
    AreaScore {
        area: Area::Reliability,
        score,
        checks,
        measurements: serde_json::Map::new(),
    }
}

fn coverage(tools: &[ToolObservation]) -> AreaScore {
    let surface = read_only_surface(tools);
    let mut checks = Vec::new();
    let mut exercised = 0usize;
    for tool in tools.iter().filter(|tool| tool.class != ToolClass::Writer) {
        match &tool.exercise {
            Some(Exercise::Exercised { .. }) => exercised += 1,
            Some(Exercise::NotExercised(reason)) => checks.extend(lost(
                CheckId::CoverageExercised,
                Some(tool.name.as_str()),
                0.0,
                None,
                *reason,
            )),
            None => {}
        }
    }
    let score = if surface == 0 {
        checks.extend(lost(
            CheckId::CoverageExercised,
            None,
            0.0,
            None,
            CheckReason::NoReadOnlyTools,
        ));
        0
    } else {
        round(100.0 * exercised as f64 / surface as f64)
    };
    AreaScore {
        area: Area::Coverage,
        score,
        checks,
        measurements: serde_json::Map::new(),
    }
}

impl Readiness {
    /// Catalog tokens from the context area, for cost and corpus lines.
    pub fn catalog_tokens(&self) -> Option<u64> {
        self.areas
            .iter()
            .find(|area| area.area == Area::Context)?
            .measurements
            .get("catalog_tokens")?
            .as_u64()
    }

    pub fn to_json(&self) -> Value {
        json!({
            "standard": self.standard,
            "score": self.score,
            "badge": badge_url(self.score),
            "attested_read_only": self.attested_read_only,
            "surface": {
                "tools": self.surface.tools,
                "read_only": self.surface.read_only,
                "writers": self.surface.writers,
                "exercised": self.surface.exercised,
            },
            "areas": self.areas.iter().map(|area| json!({
                "name": area.area.as_str(),
                "weight": area.area.weight(),
                "score": area.score,
                "measurements": Value::Object(area.measurements.clone()),
                "checks": area.checks.iter().map(|check| json!({
                    "id": check.id.as_str(),
                    "tool": check.tool,
                    "score": check.score,
                    "observed": check.observed,
                    "reason": check.reason.as_str(),
                    "hint": crate::remediation::check_hint(check.reason),
                })).collect::<Vec<_>>(),
            })).collect::<Vec<_>>(),
        })
    }

    /// Parse the `readiness` object of an `mcpeval.probe-report/v2`
    /// document. Unknown areas, checks, or reasons are rejected.
    pub fn from_json(value: &Value) -> anyhow::Result<Self> {
        use anyhow::Context;
        let count = |value: &Value, field: &str| -> anyhow::Result<u64> {
            value
                .get(field)
                .and_then(Value::as_u64)
                .with_context(|| format!("readiness is missing {field}"))
        };
        let surface = value
            .get("surface")
            .context("readiness is missing surface")?;
        let areas = value
            .get("areas")
            .and_then(Value::as_array)
            .context("readiness is missing areas")?
            .iter()
            .map(|area| {
                let label = area
                    .get("name")
                    .and_then(Value::as_str)
                    .context("area is missing a name")?;
                let checks = area
                    .get("checks")
                    .and_then(Value::as_array)
                    .context("area is missing checks")?
                    .iter()
                    .map(|check| {
                        let id = check
                            .get("id")
                            .and_then(Value::as_str)
                            .context("check is missing an id")?;
                        let reason = check
                            .get("reason")
                            .and_then(Value::as_str)
                            .context("check is missing a reason")?;
                        Ok(Check {
                            id: CheckId::from_label(id)
                                .with_context(|| format!("unknown check {id}"))?,
                            tool: check.get("tool").and_then(Value::as_str).map(str::to_owned),
                            score: count(check, "score")?,
                            observed: check.get("observed").and_then(Value::as_u64),
                            reason: CheckReason::from_label(reason)
                                .with_context(|| format!("unknown check reason {reason}"))?,
                        })
                    })
                    .collect::<anyhow::Result<Vec<_>>>()?;
                Ok(AreaScore {
                    area: Area::from_label(label)
                        .with_context(|| format!("unknown area {label}"))?,
                    score: count(area, "score")?,
                    checks,
                    measurements: area
                        .get("measurements")
                        .and_then(Value::as_object)
                        .cloned()
                        .unwrap_or_default(),
                })
            })
            .collect::<anyhow::Result<Vec<_>>>()?;
        Ok(Self {
            standard: value
                .get("standard")
                .and_then(Value::as_str)
                .context("readiness is missing standard")?
                .to_owned(),
            score: count(value, "score")?,
            attested_read_only: value
                .get("attested_read_only")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            surface: Surface {
                tools: count(surface, "tools")?,
                read_only: count(surface, "read_only")?,
                writers: count(surface, "writers")?,
                exercised: count(surface, "exercised")?,
            },
            areas,
        })
    }
}

/// Static shields.io badge URL derived from the score band; no hosting or
/// service beyond shields.io's static-badge renderer is involved.
pub fn badge_url(score: u64) -> String {
    let color = match score {
        80..=100 => "brightgreen",
        50..=79 => "yellow",
        _ => "red",
    };
    format!("https://img.shields.io/badge/mcpeval-{score}%2F100-{color}")
}

/// The catalog-wide token estimate: the standard's context measurement,
/// else a token-cost case that reached a verdict.
pub fn catalog_tokens(report: &ProbeReport) -> Option<u64> {
    report
        .readiness
        .as_ref()
        .and_then(Readiness::catalog_tokens)
        .or_else(|| {
            report
                .cases
                .iter()
                .filter(|case| !case.errored())
                .filter_map(|case| case.token_usage.as_ref())
                .map(|usage| usage.total_tokens)
                .max()
        })
}

/// Tools in the listed catalog: the standard's surface, else a token-cost
/// or discovery-cost case.
pub fn catalog_tool_count(report: &ProbeReport) -> Option<u64> {
    use crate::manifest::ProbeKind;
    report
        .readiness
        .as_ref()
        .map(|readiness| readiness.surface.tools)
        .or_else(|| {
            report
                .cases
                .iter()
                .filter(|case| {
                    matches!(case.probe, ProbeKind::TokenCost | ProbeKind::DiscoveryCost)
                })
                .filter_map(|case| case.tool_count)
                .max()
        })
}

/// A model-independent estimator feeds the measurement; this is the
/// operator's interpretation layer. The catalog is charged to every
/// session before any tool fires, so the meaningful framing is cost per
/// session and per 1,000 sessions at the configured price.
pub fn cost_context(total_tokens: u64, price_per_mtok: f64) -> String {
    let per_session = total_tokens as f64 / 1_000_000.0 * price_per_mtok;
    let per_thousand = per_session * 1000.0;
    format!(
        "the catalog costs {total_tokens} tokens of every session before the first \
         tool call — ${per_session:.4} per session at ${price_per_mtok:.2}/Mtok \
         (${per_thousand:.2} per 1,000 sessions)"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::ProbeKind;
    use crate::probe::{CaseReport, FailureReason};
    use crate::standard::{Exercise, Observations, ToolClass, ToolObservation};

    fn tool(
        name: &str,
        tokens: u64,
        class: ToolClass,
        exercise: Option<Exercise>,
    ) -> ToolObservation {
        ToolObservation {
            name: name.into(),
            tokens,
            class,
            exercise,
        }
    }

    fn exercised(consistent: bool, median_latency_ms: u64) -> Option<Exercise> {
        Some(Exercise::Exercised {
            consistent,
            median_latency_ms,
            output_schema: None,
        })
    }

    fn area(readiness: &Readiness, area: Area) -> &AreaScore {
        readiness
            .areas
            .iter()
            .find(|candidate| candidate.area == area)
            .unwrap()
    }

    /// The clean demo as the standard sees it (the plan's derivation).
    fn clean_demo() -> Observations {
        let read = |name: &str, tokens| tool(name, tokens, ToolClass::ReadOnly, exercised(true, 1));
        Observations {
            tools: vec![
                read("describe_status", 40),
                read("read_counter", 40),
                read("shared_read", 56),
                tool("flaky_read", 45, ToolClass::ReadOnly, exercised(false, 1)),
                tool("slow_read", 40, ToolClass::ReadOnly, exercised(true, 402)),
                tool("break_session", 40, ToolClass::Writer, None),
                tool("recover_session", 45, ToolClass::Writer, None),
                read("session_status", 45),
                read("report_weather", 56),
                read("sampled_read", 50),
                read("elicited_read", 49),
                read("publish_status", 50),
            ],
            attested_read_only: false,
            contention: Some(true),
            payload: Some(true),
        }
    }

    #[test]
    fn log_curves_hit_their_anchors() {
        assert_eq!(
            log_curve(2_000, CATALOG_FULL_TOKENS, CATALOG_ZERO_TOKENS),
            100.0
        );
        assert_eq!(
            log_curve(40_000, CATALOG_FULL_TOKENS, CATALOG_ZERO_TOKENS),
            0.0
        );
        assert_eq!(
            log_curve(4_000, CATALOG_FULL_TOKENS, CATALOG_ZERO_TOKENS).round(),
            77.0
        );
        assert_eq!(
            log_curve(14_235, CATALOG_FULL_TOKENS, CATALOG_ZERO_TOKENS).round(),
            34.0
        );
        assert_eq!(
            log_curve(659, TOOL_FULL_TOKENS, TOOL_ZERO_TOKENS).round(),
            88.0
        );
    }

    #[test]
    fn latency_bands_are_coarse_and_monotonic() {
        for (ms, band) in [
            (0, 100),
            (100, 100),
            (101, 80),
            (300, 80),
            (301, 50),
            (1_000, 50),
            (1_001, 20),
            (3_000, 20),
            (3_001, 0),
        ] {
            assert_eq!(latency_band(ms), band, "{ms} ms");
        }
    }

    #[test]
    fn context_matches_the_ableton_core_measurement() {
        // mcpeval 0.3.0 token-cost on Ableton core: 14,235 catalog tokens,
        // heaviest analyze_audio_clip at 659.
        let mut tools: Vec<ToolObservation> = (0..21)
            .map(|index| tool(&format!("t{index:02}"), 646, ToolClass::Writer, None))
            .collect();
        tools.push(tool("t21", 10, ToolClass::Writer, None));
        tools.push(tool("analyze_audio_clip", 659, ToolClass::Writer, None));
        let readiness = fold(&Observations {
            tools,
            ..Observations::default()
        });
        let context = area(&readiness, Area::Context);
        assert_eq!(context.score, 48);
        assert_eq!(context.measurements["catalog_tokens"], 14_235);
        assert_eq!(context.measurements["heaviest_tool_tokens"], 659);
        assert_eq!(
            context.checks,
            vec![
                Check {
                    id: CheckId::ContextCatalog,
                    tool: None,
                    score: 34,
                    observed: Some(14_235),
                    reason: CheckReason::ContextCatalogHeavy,
                },
                Check {
                    id: CheckId::ContextHeaviestTool,
                    tool: Some("analyze_audio_clip".into()),
                    score: 88,
                    observed: Some(659),
                    reason: CheckReason::ContextToolHeavy,
                },
            ]
        );
    }

    #[test]
    fn the_clean_demo_folds_to_its_pinned_score() {
        let readiness = fold(&clean_demo());
        assert_eq!(readiness.standard, STANDARD);
        assert_eq!(area(&readiness, Area::Context).score, 100);
        assert_eq!(area(&readiness, Area::Reliability).score, 94);
        assert_eq!(area(&readiness, Area::Coverage).score, 100);
        assert_eq!(readiness.score, 98);
        assert_eq!(
            readiness.surface,
            Surface {
                tools: 12,
                read_only: 10,
                writers: 2,
                exercised: 10
            }
        );
        let lost: Vec<(CheckId, Option<&str>, u64)> = readiness
            .areas
            .iter()
            .flat_map(|area| &area.checks)
            .map(|check| (check.id, check.tool.as_deref(), check.score))
            .collect();
        assert_eq!(
            lost,
            vec![
                (CheckId::ReliabilityConsistent, Some("flaky_read"), 0),
                (CheckId::ReliabilityLatency, Some("slow_read"), 50),
            ]
        );
    }

    #[test]
    fn untested_surface_counts_against_the_score() {
        // Only writers: nothing can be exercised, so reliability and
        // coverage score 0 instead of dropping out.
        let readiness = fold(&Observations {
            tools: vec![tool("delete_all", 100, ToolClass::Writer, None)],
            ..Observations::default()
        });
        assert_eq!(area(&readiness, Area::Reliability).score, 0);
        assert_eq!(area(&readiness, Area::Coverage).score, 0);
        assert_eq!(readiness.score, 30);
        assert!(readiness
            .areas
            .iter()
            .flat_map(|area| &area.checks)
            .all(|check| {
                check.id == CheckId::ContextCatalog
                    || check.id == CheckId::ContextHeaviestTool
                    || check.reason == CheckReason::NoReadOnlyTools
            }));

        // Read-only tools that could not be called: coverage names each.
        let readiness = fold(&Observations {
            tools: vec![
                tool(
                    "a",
                    10,
                    ToolClass::Unannotated,
                    Some(Exercise::NotExercised(CheckReason::CoverageUnannotated)),
                ),
                tool("b", 10, ToolClass::ReadOnly, exercised(true, 1)),
            ],
            ..Observations::default()
        });
        assert_eq!(area(&readiness, Area::Coverage).score, 50);
        assert_eq!(
            area(&readiness, Area::Coverage).checks,
            vec![Check {
                id: CheckId::CoverageExercised,
                tool: Some("a".into()),
                score: 0,
                observed: None,
                reason: CheckReason::CoverageUnannotated,
            }]
        );
    }

    #[test]
    fn readiness_json_round_trips_and_rejects_unknown_labels() {
        let readiness = fold(&clean_demo());
        let document = readiness.to_json();
        assert_eq!(document["badge"], badge_url(98));
        assert_eq!(
            document["areas"][1]["checks"][0]["hint"],
            crate::remediation::check_hint(CheckReason::ReliabilityInconsistent)
        );
        assert_eq!(Readiness::from_json(&document).unwrap(), readiness);
        let mut unknown = document.clone();
        unknown["areas"][0]["name"] = "vibes".into();
        assert!(Readiness::from_json(&unknown).is_err());
        let mut unknown = document;
        unknown["areas"][1]["checks"][0]["reason"] = "vibes".into();
        assert!(Readiness::from_json(&unknown).is_err());
    }

    #[test]
    fn every_label_list_names_every_variant_once() {
        let source = include_str!("score.rs");
        for (enum_name, listed) in [
            (
                "pub enum Area {",
                Area::ALL
                    .iter()
                    .map(|value| format!("{value:?}"))
                    .collect::<Vec<_>>(),
            ),
            (
                "pub enum CheckId {",
                CheckId::ALL
                    .iter()
                    .map(|value| format!("{value:?}"))
                    .collect(),
            ),
            (
                "pub enum CheckReason {",
                CheckReason::ALL
                    .iter()
                    .map(|value| format!("{value:?}"))
                    .collect(),
            ),
        ] {
            let body = source
                .split_once(enum_name)
                .and_then(|(_, rest)| rest.split_once("\n}"))
                .map(|(body, _)| body)
                .unwrap();
            let declared: Vec<String> = body
                .lines()
                .map(str::trim)
                .filter(|line| line.ends_with(',') && !line.starts_with("//"))
                .map(|line| line.trim_end_matches(',').to_owned())
                .collect();
            assert_eq!(listed, declared, "{enum_name}");
        }
        for id in CheckId::ALL {
            assert_eq!(CheckId::from_label(id.as_str()), Some(*id));
        }
        for reason in CheckReason::ALL {
            assert_eq!(CheckReason::from_label(reason.as_str()), Some(*reason));
        }
    }

    fn case(probe: ProbeKind, reason: Option<FailureReason>) -> CaseReport {
        CaseReport {
            id: format!("{}-case", probe.as_str()),
            probe,
            tool: None,
            detail: None,
            attempts: 1,
            first_failure: reason.map(|_| 1),
            reason,
            tool_count: None,
            schema_bytes: None,
            token_usage: None,
            latency_ms: None,
            pages: None,
        }
    }

    fn report(cases: Vec<CaseReport>) -> ProbeReport {
        ProbeReport {
            cases,
            ..ProbeReport::default()
        }
    }

    #[test]
    fn catalog_measurements_come_from_cases_that_reached_a_verdict() {
        let usage = |total_tokens| {
            Some(crate::probe::TokenUsage {
                total_tokens,
                per_tool: vec![],
            })
        };
        let mut over_budget = case(
            ProbeKind::TokenCost,
            Some(FailureReason::TokenBudgetExceeded),
        );
        over_budget.token_usage = usage(566);
        over_budget.tool_count = Some(12);
        let mut discovery = case(ProbeKind::DiscoveryCost, None);
        discovery.tool_count = Some(12);
        let over_budget = report(vec![discovery, over_budget]);
        assert_eq!(catalog_tokens(&over_budget), Some(566));
        assert_eq!(catalog_tool_count(&over_budget), Some(12));

        let mut errored = case(ProbeKind::TokenCost, Some(FailureReason::TransportTimeout));
        errored.token_usage = usage(9);
        let mut discovery = case(ProbeKind::DiscoveryCost, None);
        discovery.tool_count = Some(7);
        let errored = report(vec![errored, discovery]);
        assert_eq!(catalog_tokens(&errored), None);
        assert_eq!(catalog_tool_count(&errored), Some(7));
    }

    #[test]
    fn badge_band_colors_are_stable() {
        assert!(badge_url(100).ends_with("brightgreen"));
        assert!(badge_url(80).ends_with("brightgreen"));
        assert!(badge_url(79).ends_with("yellow"));
        assert!(badge_url(50).ends_with("yellow"));
        assert!(badge_url(49).ends_with("red"));
        assert!(badge_url(0).ends_with("red"));
    }

    #[test]
    fn cli_probe_names_match_report_labels() {
        use clap::ValueEnum;
        for kind in ProbeKind::value_variants() {
            let value = kind.to_possible_value().unwrap();
            assert_eq!(value.get_name(), kind.as_str());
        }
    }
}
