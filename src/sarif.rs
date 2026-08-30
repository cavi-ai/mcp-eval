//! SARIF 2.1.0 rendering of a probe report for GitHub code scanning.
//!
//! GitHub ingests a SARIF upload as inline pull-request annotations, so a
//! red gate becomes a comment pinned to the diff the engineer is already
//! reading instead of a line buried in a CI log. Everything here is
//! derived from the already-sanitized report: case ids, probe kinds, fixed
//! reason labels, and remediation hints. No payloads, no timestamps — the
//! document is deterministic for a given report.

use crate::probe::ProbeReport;

/// Render the report as a SARIF 2.1.0 log. Each failing case is a result
/// whose rule id is the probe kind and whose message carries the fixed
/// reason plus its remediation hint.
pub fn render_sarif(server: &str, report: &ProbeReport) -> String {
    let mut all_rules = Vec::new();
    let mut seen_kinds = std::collections::BTreeSet::new();
    for case in &report.cases {
        if seen_kinds.insert(case.probe.as_str()) {
            all_rules.push(rule_for(case.probe.as_str()));
        }
    }
    let results: Vec<serde_json::Value> = report
        .cases
        .iter()
        .filter_map(|case| {
            let reason = case.reason?;
            let hint = crate::remediation::hint(reason);
            Some(serde_json::json!({
                "ruleId": case.probe.as_str(),
                "level": "error",
                "message": {
                    "text": format!(
                        "{} ({}): {} — {}",
                        case.id, server, reason.as_str(), hint
                    )
                },
                // A stable partial fingerprint lets GitHub suppress and
                // re-open annotations as the manifest evolves without
                // depending on line numbers.
                "partialFingerprints": {
                    "mcpevalCaseId": case.id
                },
            }))
        })
        .collect();
    let document = serde_json::json!({
        "$schema": "https://json.schemastore.org/sarif-2.1.0.json",
        "version": "2.1.0",
        "runs": [{
            "tool": {
                "driver": {
                    "name": "mcpeval",
                    "informationUri": "https://github.com/cavi-ai/mcp-eval",
                    "version": env!("CARGO_PKG_VERSION"),
                    "rules": all_rules,
                }
            },
            "results": results,
        }],
    });
    serde_json::to_string_pretty(&document).unwrap_or_else(|_| "{}".into())
}

fn rule_for(probe: &str) -> serde_json::Value {
    serde_json::json!({
        "id": probe,
        "name": probe,
        "shortDescription": {"text": format!("MCP server fails the {probe} probe")},
        "helpUri": "https://github.com/cavi-ai/mcp-eval/blob/main/docs/mcp-eval/source/pages/reference/evaluation-dimensions.md",
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::ProbeKind;
    use crate::probe::{CaseReport, FailureReason};

    fn case(id: &str, probe: ProbeKind, reason: Option<FailureReason>) -> CaseReport {
        CaseReport {
            id: id.into(),
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
    fn sarif_document_is_spec_shaped_and_deterministic() {
        let report = ProbeReport {
            cases: vec![
                case(
                    "a",
                    ProbeKind::Pagination,
                    Some(FailureReason::PaginationStalledCursor),
                ),
                case("ok", ProbeKind::TokenCost, None),
                case(
                    "b",
                    ProbeKind::TokenCost,
                    Some(FailureReason::UnexpectedOutcome),
                ),
            ],
        };
        let first = render_sarif("demo", &report);
        let second = render_sarif("demo", &report);
        assert_eq!(first, second, "SARIF must be deterministic");

        let document: serde_json::Value = serde_json::from_str(&first).unwrap();
        assert_eq!(document["version"], "2.1.0");
        assert_eq!(
            document["$schema"],
            "https://json.schemastore.org/sarif-2.1.0.json"
        );
        let run = &document["runs"][0];
        assert_eq!(run["tool"]["driver"]["name"], "mcpeval");
        let rules = run["tool"]["driver"]["rules"].as_array().unwrap();
        assert_eq!(rules.len(), 2, "one rule per distinct probe kind");
        let results = run["results"].as_array().unwrap();
        assert_eq!(results.len(), 2, "only failing cases become results");
        assert_eq!(results[0]["ruleId"], "pagination");
        let text = results[0]["message"]["text"].as_str().unwrap();
        assert!(text.contains("pagination-stalled-cursor"));
        assert!(text.contains("never re-serve a page"));
        assert!(text.contains("(demo)"));
    }
}
