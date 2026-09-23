//! SARIF 2.1.0 rendering of a probe report for GitHub code scanning.
//!
//! GitHub ingests a SARIF upload as code-scanning alerts, so a red gate
//! becomes an alert anchored to the manifest case that failed instead of a
//! line buried in a CI log. Everything here is derived from the
//! already-sanitized report plus the manifest's location: case ids, probe
//! kinds, fixed reason labels, remediation hints, and the line each case
//! starts on. No payloads, no timestamps — the document is deterministic for
//! a given report and manifest.

use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

use crate::probe::ProbeReport;

/// The manifest a report was produced from, used to locate each result.
pub struct ManifestSource<'a> {
    /// Repository-relative URI of the manifest file (see [`manifest_uri`]).
    pub uri: &'a str,
    /// Manifest text; each result's region is the line of its case's `id`.
    /// `None` locates results at the file without a region.
    pub text: Option<&'a str>,
}

/// Render the report as a SARIF 2.1.0 log. Each failing case is a result
/// whose rule id is the probe kind, whose message carries the fixed reason
/// plus its remediation hint, and whose location is the case in the
/// manifest.
pub fn render_sarif(server: &str, report: &ProbeReport, manifest: &ManifestSource) -> String {
    let mut all_rules = Vec::new();
    let mut seen_kinds = std::collections::BTreeSet::new();
    for case in &report.cases {
        if seen_kinds.insert(case.probe.as_str()) {
            all_rules.push(rule_for(case.probe.as_str()));
        }
    }
    let lines = manifest.text.map(case_lines).unwrap_or_default();
    let results: Vec<serde_json::Value> = report
        .cases
        .iter()
        .filter_map(|case| {
            let reason = case.reason?;
            let hint = crate::remediation::hint(reason);
            let mut physical_location = serde_json::json!({
                "artifactLocation": {"uri": manifest.uri},
            });
            if let Some(line) = lines.get(case.id.as_str()) {
                physical_location["region"] = serde_json::json!({"startLine": line});
            }
            Some(serde_json::json!({
                "ruleId": case.probe.as_str(),
                "level": "error",
                "message": {
                    "text": format!(
                        "{} ({}): {} — {}",
                        case.id, server, reason.as_str(), hint
                    )
                },
                "locations": [{"physicalLocation": physical_location}],
                // A stable partial fingerprint lets GitHub suppress and
                // re-open alerts as the manifest evolves without depending
                // on line numbers; the server keeps two servers probed with
                // one manifest apart.
                "partialFingerprints": {
                    "mcpevalCaseId": format!("{server}/{}", case.id)
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
            // One code-scanning category per server, so uploads for two
            // servers in one repository never close each other's alerts.
            "automationDetails": {"id": format!("mcpeval/{server}/")},
            "results": results,
        }],
    });
    serde_json::to_string_pretty(&document).unwrap_or_else(|_| "{}".into())
}

/// SARIF `artifactLocation.uri` for a manifest path: relative to `cwd`
/// (the repository checkout in CI) when the path lies under it, a `file:`
/// URI when it is absolute elsewhere, percent-encoded either way.
pub fn manifest_uri(path: &Path, cwd: Option<&Path>) -> String {
    let relative = match cwd {
        Some(cwd) if path.is_absolute() => relative_to(path, cwd),
        _ => None,
    };
    let path = relative.as_deref().unwrap_or(path);
    let mut segments = Vec::new();
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => {
                segments.push(percent_encode(&prefix.as_os_str().to_string_lossy(), true))
            }
            Component::RootDir | Component::CurDir => {}
            Component::ParentDir => segments.push("..".to_owned()),
            Component::Normal(part) => {
                segments.push(percent_encode(&part.to_string_lossy(), false))
            }
        }
    }
    let joined = segments.join("/");
    if path.has_root() {
        format!("file:///{joined}")
    } else {
        joined
    }
}

fn relative_to(path: &Path, cwd: &Path) -> Option<PathBuf> {
    if let Ok(rest) = path.strip_prefix(cwd) {
        return Some(rest.to_path_buf());
    }
    let path = std::fs::canonicalize(path).ok()?;
    let cwd = std::fs::canonicalize(cwd).ok()?;
    path.strip_prefix(&cwd).ok().map(Path::to_path_buf)
}

fn percent_encode(segment: &str, keep_colon: bool) -> String {
    let mut encoded = String::with_capacity(segment.len());
    for byte in segment.bytes() {
        if byte.is_ascii_alphanumeric()
            || matches!(byte, b'-' | b'.' | b'_' | b'~')
            || (keep_colon && byte == b':')
        {
            encoded.push(byte as char);
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

/// 1-based line of each case's `"id"` key. Only keys of the objects directly
/// inside the top-level `probes` array (nesting depth 3) count, so an `"id"`
/// inside a case's arguments never shadows a real case.
fn case_lines(text: &str) -> BTreeMap<String, u64> {
    enum Token {
        Str(String),
        Colon,
        Other,
    }
    let mut tokens: Vec<(Token, usize, u64)> = Vec::new();
    let mut depth = 0usize;
    let mut line = 1u64;
    let mut chars = text.chars();
    while let Some(ch) = chars.next() {
        match ch {
            '\n' => line += 1,
            '"' => {
                let mut value = String::new();
                while let Some(ch) = chars.next() {
                    match ch {
                        '"' => break,
                        '\\' => {
                            value.push(ch);
                            if let Some(escaped) = chars.next() {
                                value.push(escaped);
                            }
                        }
                        _ => value.push(ch),
                    }
                }
                tokens.push((Token::Str(value), depth, line));
            }
            '{' | '[' => {
                depth += 1;
                tokens.push((Token::Other, depth, line));
            }
            '}' | ']' => {
                depth = depth.saturating_sub(1);
                tokens.push((Token::Other, depth, line));
            }
            ':' => tokens.push((Token::Colon, depth, line)),
            ',' => tokens.push((Token::Other, depth, line)),
            _ => {}
        }
    }
    let mut lines = BTreeMap::new();
    for window in tokens.windows(3) {
        if let [(Token::Str(key), 3, key_line), (Token::Colon, _, _), (Token::Str(value), _, _)] =
            window
        {
            if key == "id" {
                lines.entry(value.clone()).or_insert(*key_line);
            }
        }
    }
    lines
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
            manifest_sha256: None,
        };
        let manifest = ManifestSource {
            uri: "mcp-eval.manifest.json",
            text: Some("{\"probes\":[\n{\"id\":\"a\"},\n{\"id\":\"ok\"},\n{\"id\":\"b\"}]}"),
        };
        let first = render_sarif("demo", &report, &manifest);
        let second = render_sarif("demo", &report, &manifest);
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
        let location = &results[1]["locations"][0]["physicalLocation"];
        assert_eq!(
            location["artifactLocation"]["uri"],
            "mcp-eval.manifest.json"
        );
        assert_eq!(location["region"]["startLine"], 4);
        assert_eq!(results[1]["partialFingerprints"]["mcpevalCaseId"], "demo/b");
        assert_eq!(run["automationDetails"]["id"], "mcpeval/demo/");
    }

    #[test]
    fn case_lines_ignore_ids_nested_in_arguments() {
        let text = r#"{"version":1,"sandboxes":{},"probes":[
  {"probe":"latency-budget","arguments":{"id":"b"},
   "id":"a"},
  {"id":"b","probe":"token-cost","note":"id"}
]}"#;
        let lines = case_lines(text);
        assert_eq!(lines.get("a"), Some(&3));
        assert_eq!(lines.get("b"), Some(&4));
        assert_eq!(lines.len(), 2);
    }

    #[test]
    fn manifest_uri_is_repository_relative_and_percent_encoded() {
        let (root, uri_root) = if cfg!(windows) {
            ("C:\\", "file:///C:/")
        } else {
            ("/", "file:///")
        };
        let cwd = Path::new(root).join("repo");
        assert_eq!(
            manifest_uri(Path::new("./ci/mcp eval.json"), Some(&cwd)),
            "ci/mcp%20eval.json"
        );
        assert_eq!(
            manifest_uri(&cwd.join("ci").join("m.json"), Some(&cwd)),
            "ci/m.json"
        );
        assert_eq!(
            manifest_uri(
                &Path::new(root).join("else where").join("m.json"),
                Some(&cwd)
            ),
            format!("{uri_root}else%20where/m.json")
        );
        assert_eq!(manifest_uri(Path::new("../m.json"), None), "../m.json");
    }
}
