//! Deterministic, privacy-safe readiness scoring over a probe report.
//!
//! The score is computed only from case verdicts already present in the
//! report: no measurements, payloads, or server details enter it, so the
//! same report always produces the same score. Categories that have no
//! cases in the manifest are excluded from both numerator and denominator,
//! which keeps partial manifests meaningful instead of punishing them for
//! probes they never declared.

use crate::manifest::ProbeKind;
use crate::probe::ProbeReport;

pub struct CategoryScore {
    pub name: &'static str,
    pub passed: u64,
    pub total: u64,
}

impl CategoryScore {
    pub fn fraction(&self) -> f64 {
        if self.total == 0 {
            return 0.0;
        }
        self.passed as f64 / self.total as f64
    }
}

pub struct ReadinessScore {
    /// Integer 0-100.
    pub overall: u64,
    pub categories: Vec<CategoryScore>,
}

/// Category weights. Only categories present in the report are averaged.
const WEIGHTS: &[(&str, f64, &[ProbeKind])] = &[
    (
        "discovery",
        0.25,
        &[
            ProbeKind::DiscoveryCost,
            ProbeKind::TokenCost,
            ProbeKind::Pagination,
            ProbeKind::SurfaceListing,
        ],
    ),
    (
        "reliability",
        0.35,
        &[
            ProbeKind::DegradationOverN,
            ProbeKind::ErrorHonesty,
            ProbeKind::StateRecovery,
            ProbeKind::LatencyBudget,
            ProbeKind::PayloadBounds,
        ],
    ),
    (
        "contract",
        0.30,
        &[
            ProbeKind::SchemaGuessability,
            ProbeKind::InstructionFidelity,
            ProbeKind::OutputSchema,
        ],
    ),
    ("concurrency", 0.10, &[ProbeKind::Contention]),
];

pub fn readiness(report: &ProbeReport) -> ReadinessScore {
    let mut categories = Vec::new();
    let mut weighted = 0.0;
    let mut weights = 0.0;
    for (name, weight, kinds) in WEIGHTS {
        let total = report
            .cases
            .iter()
            .filter(|case| kinds.contains(&case.probe))
            .count() as u64;
        if total == 0 {
            continue;
        }
        let passed = report
            .cases
            .iter()
            .filter(|case| kinds.contains(&case.probe) && case.passed())
            .count() as u64;
        weighted += weight * (passed as f64 / total as f64);
        weights += weight;
        categories.push(CategoryScore {
            name,
            passed,
            total,
        });
    }
    let overall = if weights == 0.0 {
        0
    } else {
        ((weighted / weights * 100.0).round() as i64).clamp(0, 100) as u64
    };
    ReadinessScore {
        overall,
        categories,
    }
}

impl ReadinessScore {
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "score": self.overall,
            "categories": self.categories.iter().map(|category| serde_json::json!({
                "name": category.name,
                "passed": category.passed,
                "total": category.total,
            })).collect::<Vec<_>>(),
            "badge": badge_url(self.overall),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::probe::{CaseReport, FailureReason};

    fn case(probe: ProbeKind, reason: Option<FailureReason>) -> CaseReport {
        CaseReport {
            id: format!("{}-case", probe.as_str()),
            probe,
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

    #[test]
    fn empty_report_scores_zero() {
        let score = readiness(&ProbeReport { cases: vec![] });
        assert_eq!(score.overall, 0);
        assert!(score.categories.is_empty());
    }

    #[test]
    fn all_passing_scores_hundred_with_all_categories() {
        let cases: Vec<CaseReport> = [
            ProbeKind::DiscoveryCost,
            ProbeKind::TokenCost,
            ProbeKind::Pagination,
            ProbeKind::DegradationOverN,
            ProbeKind::ErrorHonesty,
            ProbeKind::StateRecovery,
            ProbeKind::LatencyBudget,
            ProbeKind::SchemaGuessability,
            ProbeKind::InstructionFidelity,
            ProbeKind::Contention,
        ]
        .map(|probe| case(probe, None))
        .into();
        let score = readiness(&ProbeReport { cases });
        assert_eq!(score.overall, 100);
        assert_eq!(score.categories.len(), 4);
        assert!(score.to_json()["badge"]
            .as_str()
            .unwrap()
            .contains("brightgreen"));
    }

    #[test]
    fn partial_manifests_omit_missing_categories_without_penalty() {
        // A manifest with only contract cases still scores 100 when they pass.
        let cases: Vec<CaseReport> = [
            ProbeKind::SchemaGuessability,
            ProbeKind::InstructionFidelity,
        ]
        .map(|probe| case(probe, None))
        .into();
        assert_eq!(readiness(&ProbeReport { cases }).overall, 100);

        // One failing discovery case out of one: discovery is half of its
        // category pair... a single failing case drags only its category.
        let cases = vec![case(
            ProbeKind::TokenCost,
            Some(FailureReason::UnexpectedOutcome),
        )];
        let score = readiness(&ProbeReport { cases });
        assert_eq!(score.overall, 0);
        assert_eq!(score.categories[0].total, 1);
    }

    #[test]
    fn mixed_report_is_weighted_mean_of_present_categories() {
        // discovery 1/2 pass (0.5 * 0.3), reliability 1/1 (0.35),
        // contract 1/2 (0.25 * 0.5). No concurrency cases.
        let cases = vec![
            case(ProbeKind::DiscoveryCost, None),
            case(ProbeKind::TokenCost, Some(FailureReason::UnexpectedOutcome)),
            case(ProbeKind::ErrorHonesty, None),
            case(ProbeKind::SchemaGuessability, None),
            case(
                ProbeKind::InstructionFidelity,
                Some(FailureReason::ValueMismatch),
            ),
        ];
        let expected = ((0.3 * 0.5_f64 + 0.35 + 0.125) / 0.9 * 100.0).round() as u64;
        assert_eq!(readiness(&ProbeReport { cases }).overall, expected);
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
}
