//! Compare two committed `mcpeval.probe-report/v1` documents — the
//! committed baseline and the current run — and classify every case as
//! regressed, fixed, changed, or unchanged. Unlike `compare` (the space axis:
//! several servers under one manifest), `diff` is the time axis: it gates
//! on regressions so a CI job can fail when the battery moves backward.
//!
//! Privacy boundary: both documents are share-safe by construction, so the
//! diff output carries only case IDs, verdicts, fixed reason labels, and
//! measurement numbers.

use std::path::Path;

use anyhow::{bail, Context};

use crate::probe::{ProbeReport, TokenUsage};

/// One case in a report, keyed for matching. Cases are matched by
/// (id, probe) so a renamed case id with the same probe reads as one
/// removed and one added, never as a silent verdict change.
struct Key {
    id: String,
    probe: String,
}

/// Verdict of one matched case pair.
#[derive(Debug, PartialEq, Eq)]
pub enum Verdict {
    /// Passed in the baseline, fails now.
    Regressed,
    /// Failed in the baseline, passes now.
    Fixed,
    /// Fails in both documents, for a different reason.
    Changed,
    /// Same verdict and reason in both documents.
    Unchanged,
}

/// One matched case pair with its measurement movement.
pub struct CaseDiff {
    pub id: String,
    pub probe: String,
    pub verdict: Verdict,
    pub baseline_reason: Option<String>,
    pub current_reason: Option<String>,
    /// Catalog total tokens: (baseline, current) when both cases carry the
    /// measurement.
    pub total_tokens: Option<(u64, u64)>,
    /// Slowest observed latency: (baseline, current) when both carry it.
    pub latency_ms: Option<(u64, u64)>,
}

/// The full comparison of two report documents.
pub struct Diff {
    pub cases: Vec<CaseDiff>,
    pub baseline_score: u64,
    pub current_score: u64,
    /// Case IDs present only in the baseline.
    pub missing: Vec<String>,
    /// Case IDs present only in the current run.
    pub added: Vec<String>,
    /// Measurement movement for the catalog: (baseline, current) when both
    /// sides carry it on a token-cost case.
    pub total_tokens: Option<(u64, u64)>,
}

impl Diff {
    pub fn regressed(&self) -> usize {
        self.cases
            .iter()
            .filter(|case| case.verdict == Verdict::Regressed)
            .count()
    }

    pub fn fixed(&self) -> usize {
        self.cases
            .iter()
            .filter(|case| case.verdict == Verdict::Fixed)
            .count()
    }

    pub fn changed(&self) -> usize {
        self.cases
            .iter()
            .filter(|case| case.verdict == Verdict::Changed)
            .count()
    }

    pub fn unchanged(&self) -> usize {
        self.cases
            .iter()
            .filter(|case| case.verdict == Verdict::Unchanged)
            .count()
    }

    /// A diff gates when any matched case regressed. Added and removed
    /// cases are informational: manifests legitimately grow and shrink,
    /// and the committed baseline is regenerated deliberately.
    pub fn gated(&self) -> bool {
        self.regressed() > 0
    }
}

fn token_usage(case: &crate::probe::CaseReport) -> Option<&TokenUsage> {
    case.token_usage.as_ref()
}

pub fn diff(baseline: &ProbeReport, current: &ProbeReport) -> Diff {
    let baseline_keys: Vec<(Key, &crate::probe::CaseReport)> = baseline
        .cases
        .iter()
        .map(|case| {
            (
                Key {
                    id: case.id.clone(),
                    probe: case.probe.as_str().to_owned(),
                },
                case,
            )
        })
        .collect();
    let current_keys: Vec<(Key, &crate::probe::CaseReport)> = current
        .cases
        .iter()
        .map(|case| {
            (
                Key {
                    id: case.id.clone(),
                    probe: case.probe.as_str().to_owned(),
                },
                case,
            )
        })
        .collect();

    let mut cases = Vec::new();
    let mut total_tokens = None;
    for (current_key, current_case) in &current_keys {
        let Some((_, baseline_case)) = baseline_keys
            .iter()
            .find(|(k, _)| k.id == current_key.id && k.probe == current_key.probe)
        else {
            continue;
        };
        let verdict = match (baseline_case.reason, current_case.reason) {
            (None, Some(_)) => Verdict::Regressed,
            (Some(_), None) => Verdict::Fixed,
            (Some(baseline_reason), Some(current_reason)) if baseline_reason != current_reason => {
                Verdict::Changed
            }
            (None, None) | (Some(_), Some(_)) => Verdict::Unchanged,
        };
        let tokens = match (token_usage(baseline_case), token_usage(current_case)) {
            (Some(b), Some(c)) => Some((b.total_tokens, c.total_tokens)),
            _ => None,
        };
        if current_case.probe == crate::manifest::ProbeKind::TokenCost {
            total_tokens = tokens;
        }
        cases.push(CaseDiff {
            id: current_case.id.clone(),
            probe: current_key.probe.clone(),
            verdict,
            baseline_reason: baseline_case
                .reason
                .map(|reason| reason.as_str().to_owned()),
            current_reason: current_case.reason.map(|reason| reason.as_str().to_owned()),
            total_tokens: tokens,
            latency_ms: match (baseline_case.latency_ms, current_case.latency_ms) {
                (Some(b), Some(c)) => Some((b, c)),
                _ => None,
            },
        });
    }
    let missing = baseline_keys
        .iter()
        .filter(|(k, _)| {
            !current_keys
                .iter()
                .any(|(c, _)| c.id == k.id && c.probe == k.probe)
        })
        .map(|(k, _)| k.id.clone())
        .collect();
    let added = current_keys
        .iter()
        .filter(|(k, _)| {
            !baseline_keys
                .iter()
                .any(|(b, _)| b.id == k.id && b.probe == k.probe)
        })
        .map(|(k, _)| k.id.clone())
        .collect();
    Diff {
        baseline_score: crate::score::readiness(baseline).overall,
        current_score: crate::score::readiness(current).overall,
        cases,
        missing,
        added,
        total_tokens,
    }
}

/// A loaded report document: the server label it names and its cases.
pub struct Document {
    pub server: Option<String>,
    pub report: ProbeReport,
}

/// Load a report document from a path or stdin (`-`).
pub fn load_document(document: &Path) -> anyhow::Result<Document> {
    let body = if document.as_os_str() == "-" {
        use std::io::Read;
        let mut buffer = String::new();
        std::io::stdin().read_to_string(&mut buffer)?;
        buffer
    } else {
        std::fs::read_to_string(document)
            .with_context(|| format!("reading {}", document.display()))?
    };
    let parsed: serde_json::Value =
        serde_json::from_str(&body).context("report document is not valid JSON")?;
    let report = ProbeReport::from_json_document(&parsed).context("not a usable probe report")?;
    let server = parsed
        .get("server")
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned);
    Ok(Document { server, report })
}

/// A diff is the time axis of one server: reports from two different
/// servers belong to `compare`, and diffing them would gate on noise.
pub fn ensure_same_server(baseline: &Document, current: &Document) -> anyhow::Result<()> {
    if let (Some(baseline), Some(current)) = (&baseline.server, &current.server) {
        if baseline != current {
            bail!(
                "baseline is for server `{baseline}` but current is for server `{current}`; \
                 diff compares one server over time (use `compare` across servers)"
            );
        }
    }
    Ok(())
}

pub fn run(
    baseline_path: &Path,
    current_path: &Path,
    fail_on_regression: bool,
) -> anyhow::Result<String> {
    let baseline = load_document(baseline_path)?;
    let current = load_document(current_path)?;
    ensure_same_server(&baseline, &current)?;
    let outcome = diff(&baseline.report, &current.report);
    let rendered = render(&outcome);
    if fail_on_regression && outcome.gated() {
        print!("{rendered}");
        bail!(
            "{} regressed case{}, {} fixed",
            outcome.regressed(),
            if outcome.regressed() == 1 { "" } else { "s" },
            outcome.fixed()
        );
    }
    Ok(rendered)
}

fn movement(measurement: Option<(u64, u64)>, unit: &str) -> String {
    match measurement {
        None => String::new(),
        Some((baseline, current)) if baseline == current => {
            format!("{baseline}{unit} unchanged")
        }
        Some((baseline, current)) => {
            let delta = current as i64 - baseline as i64;
            let direction = if delta > 0 { "+" } else { "" };
            format!("{baseline}{unit} → {current}{unit} ({direction}{delta})")
        }
    }
}

fn verdict_label(verdict: &Verdict) -> &'static str {
    match verdict {
        Verdict::Regressed => "regressed",
        Verdict::Fixed => "fixed",
        Verdict::Changed => "changed",
        Verdict::Unchanged => "unchanged",
    }
}

/// Versioned, deterministic diff document: no timestamps, sessions, or
/// payloads — safe to attach to a CI artifact beside the reports.
pub fn to_json(outcome: &Diff) -> serde_json::Value {
    serde_json::json!({
        "schema": "mcpeval.probe-diff/v1",
        "readiness": {
            "baseline": outcome.baseline_score,
            "current": outcome.current_score,
        },
        "summary": {
            "regressed": outcome.regressed(),
            "fixed": outcome.fixed(),
            "changed": outcome.changed(),
            "unchanged": outcome.unchanged(),
            "removed": outcome.missing.len(),
            "added": outcome.added.len(),
        },
        "cases": outcome.cases.iter().map(|case| {
            let measurements = match (&case.total_tokens, &case.latency_ms) {
                (Some((baseline, current)), _) if case.probe == "token-cost" => {
                    serde_json::json!({"total_tokens": {"baseline": baseline, "current": current}})
                }
                (_, Some((baseline, current))) => {
                    serde_json::json!({"latency_ms": {"baseline": baseline, "current": current}})
                }
                _ => serde_json::json!({}),
            };
            serde_json::json!({
                "id": case.id,
                "probe": case.probe,
                "verdict": verdict_label(&case.verdict),
                "baseline_reason": case.baseline_reason,
                "current_reason": case.current_reason,
                "measurements": measurements,
            })
        }).collect::<Vec<_>>(),
        "removed": outcome.missing,
        "added": outcome.added,
    })
}

pub fn render(outcome: &Diff) -> String {
    let mut width = "case".len();
    for case in &outcome.cases {
        width = width.max(case.id.len());
    }
    for id in outcome.missing.iter().chain(outcome.added.iter()) {
        width = width.max(id.len());
    }
    width = width.max("readiness".len());
    let mut out = String::new();
    for case in &outcome.cases {
        let label = match case.verdict {
            Verdict::Regressed => {
                let reason = case.current_reason.as_deref().unwrap_or("failed");
                format!("REGRESSED {reason}")
            }
            Verdict::Fixed => "fixed".to_string(),
            Verdict::Changed => format!(
                "CHANGED {} → {}",
                case.baseline_reason.as_deref().unwrap_or("failed"),
                case.current_reason.as_deref().unwrap_or("failed")
            ),
            Verdict::Unchanged => "unchanged".to_string(),
        };
        let measurement = match (&case.total_tokens, &case.latency_ms) {
            (Some(tokens), _) if case.probe == "token-cost" => movement(Some(*tokens), " tok"),
            (_, Some(latency)) => movement(Some(*latency), " ms"),
            _ => String::new(),
        };
        let measurement = if measurement.is_empty() {
            String::new()
        } else {
            format!("  {measurement}")
        };
        out.push_str(&format!(
            "{:<width$}  {}{}\n",
            case.id,
            label,
            measurement,
            width = width
        ));
    }
    for id in &outcome.missing {
        out.push_str(&format!("{:<width$}  removed\n", id, width = width));
    }
    for id in &outcome.added {
        out.push_str(&format!("{:<width$}  added\n", id, width = width));
    }
    out.push_str(&format!(
        "readiness  {} → {}\n",
        outcome.baseline_score, outcome.current_score
    ));
    out.push_str(&format!(
        "{} regressed, {} fixed, {} changed, {} unchanged, {} removed, {} added\n",
        outcome.regressed(),
        outcome.fixed(),
        outcome.changed(),
        outcome.unchanged(),
        outcome.missing.len(),
        outcome.added.len()
    ));
    out
}

pub fn render_markdown(outcome: &Diff) -> String {
    let mut out = String::new();
    out.push_str("## mcp-eval baseline diff\n\n");
    out.push_str(&format!(
        "Readiness: {} → {}\n\n",
        outcome.baseline_score, outcome.current_score
    ));
    out.push_str("| Case | Verdict | Baseline | Current |\n");
    out.push_str("| --- | --- | --- | --- |\n");
    for case in &outcome.cases {
        let verdict = match case.verdict {
            Verdict::Regressed => {
                let reason = case.current_reason.as_deref().unwrap_or("failed");
                format!("**regressed** (`{reason}`)")
            }
            Verdict::Fixed => "fixed".to_string(),
            Verdict::Changed => "**changed**".to_string(),
            Verdict::Unchanged => "unchanged".to_string(),
        };
        let baseline_cell = case
            .baseline_reason
            .as_deref()
            .map(|reason| format!("fail (`{reason}`)"))
            .unwrap_or_else(|| "pass".into());
        let current_cell = case
            .current_reason
            .as_deref()
            .map(|reason| format!("fail (`{reason}`)"))
            .unwrap_or_else(|| "pass".into());
        out.push_str(&format!(
            "| {} | {} | {} | {} |\n",
            case.id, verdict, baseline_cell, current_cell
        ));
    }
    for id in &outcome.missing {
        out.push_str(&format!("| {id} | removed | pass | — |\n"));
    }
    for id in &outcome.added {
        out.push_str(&format!("| {id} | added | — | pass |\n"));
    }
    out.push_str(&format!(
        "\n{} regressed, {} fixed, {} changed, {} removed, {} added. *Deterministic diff of share-safe report documents; no payloads or error prose are included.*\n",
        outcome.regressed(),
        outcome.fixed(),
        outcome.changed(),
        outcome.missing.len(),
        outcome.added.len()
    ));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::probe::{CaseReport, FailureReason};
    use crate::score::readiness;

    fn case(
        id: &str,
        probe: crate::manifest::ProbeKind,
        reason: Option<FailureReason>,
    ) -> CaseReport {
        CaseReport {
            id: id.to_owned(),
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
    fn classifies_regression_fix_and_unchanged() {
        let baseline = ProbeReport {
            cases: vec![
                case("a", crate::manifest::ProbeKind::DiscoveryCost, None),
                case(
                    "b",
                    crate::manifest::ProbeKind::ErrorHonesty,
                    Some(FailureReason::UnexpectedOutcome),
                ),
                case("c", crate::manifest::ProbeKind::Contention, None),
            ],
        };
        let current = ProbeReport {
            cases: vec![
                case(
                    "a",
                    crate::manifest::ProbeKind::DiscoveryCost,
                    Some(FailureReason::UnexpectedOutcome),
                ),
                case("b", crate::manifest::ProbeKind::ErrorHonesty, None),
                case("c", crate::manifest::ProbeKind::Contention, None),
            ],
        };
        let outcome = diff(&baseline, &current);
        assert_eq!(outcome.regressed(), 1);
        assert_eq!(outcome.fixed(), 1);
        assert_eq!(outcome.cases.len(), 3);
        assert!(outcome.gated());
        assert_eq!(outcome.missing.len(), 0);
        assert_eq!(outcome.added.len(), 0);
    }

    #[test]
    fn failing_for_a_different_reason_is_changed_not_regressed() {
        let baseline = ProbeReport {
            cases: vec![case(
                "a",
                crate::manifest::ProbeKind::Pagination,
                Some(FailureReason::PaginationDuplicateTool),
            )],
        };
        let current = ProbeReport {
            cases: vec![case(
                "a",
                crate::manifest::ProbeKind::Pagination,
                Some(FailureReason::PaginationStalledCursor),
            )],
        };
        let outcome = diff(&baseline, &current);
        assert_eq!(outcome.cases[0].verdict, Verdict::Changed);
        assert_eq!(outcome.changed(), 1);
        assert!(!outcome.gated());
        assert!(render(&outcome)
            .contains("CHANGED pagination-duplicate-tool → pagination-stalled-cursor"));
        assert_eq!(to_json(&outcome)["cases"][0]["verdict"], "changed");

        let same = diff(&baseline, &baseline);
        assert_eq!(same.cases[0].verdict, Verdict::Unchanged);
    }

    #[test]
    fn added_and_removed_cases_are_informational_only() {
        let baseline = ProbeReport {
            cases: vec![case("old", crate::manifest::ProbeKind::DiscoveryCost, None)],
        };
        let current = ProbeReport {
            cases: vec![case("new", crate::manifest::ProbeKind::DiscoveryCost, None)],
        };
        let outcome = diff(&baseline, &current);
        assert_eq!(outcome.missing, vec!["old".to_owned()]);
        assert_eq!(outcome.added, vec!["new".to_owned()]);
        assert!(!outcome.gated());
        assert!(outcome.cases.is_empty());
    }

    #[test]
    fn readiness_movement_is_reported() {
        let baseline = ProbeReport {
            cases: vec![case("a", crate::manifest::ProbeKind::DiscoveryCost, None)],
        };
        let current = ProbeReport {
            cases: vec![case(
                "a",
                crate::manifest::ProbeKind::DiscoveryCost,
                Some(FailureReason::UnexpectedOutcome),
            )],
        };
        let outcome = diff(&baseline, &current);
        assert_eq!(outcome.baseline_score, readiness(&baseline).overall);
        assert_eq!(outcome.current_score, 0);
        assert_eq!(outcome.baseline_score, 100);
    }

    #[test]
    fn token_movement_is_paired_by_case() {
        let mut baseline_case = case("t", crate::manifest::ProbeKind::TokenCost, None);
        baseline_case.token_usage = Some(TokenUsage {
            total_tokens: 100,
            per_tool: vec![],
        });
        let mut current_case = case("t", crate::manifest::ProbeKind::TokenCost, None);
        current_case.token_usage = Some(TokenUsage {
            total_tokens: 260,
            per_tool: vec![],
        });
        let outcome = diff(
            &ProbeReport {
                cases: vec![baseline_case],
            },
            &ProbeReport {
                cases: vec![current_case],
            },
        );
        assert_eq!(outcome.total_tokens, Some((100, 260)));
        assert_eq!(outcome.cases[0].total_tokens, Some((100, 260)));
        assert!(render(&outcome).contains("+160"));
    }

    #[test]
    fn a_one_sided_token_measurement_reports_no_movement() {
        let mut baseline_case = case("t", crate::manifest::ProbeKind::TokenCost, None);
        baseline_case.token_usage = Some(TokenUsage {
            total_tokens: 100,
            per_tool: vec![],
        });
        let current_case = case("t", crate::manifest::ProbeKind::TokenCost, None);
        let outcome = diff(
            &ProbeReport {
                cases: vec![baseline_case],
            },
            &ProbeReport {
                cases: vec![current_case],
            },
        );
        assert_eq!(outcome.total_tokens, None);
        assert_eq!(outcome.cases[0].total_tokens, None);
        assert!(!render(&outcome).contains("tok"));
    }
}
