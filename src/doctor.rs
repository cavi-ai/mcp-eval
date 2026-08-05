use std::path::Path;
use std::sync::LazyLock;

use anyhow::Context;
use regex::Regex;
use serde_json::Value;

/// Result of a redaction sweep over every `*.jsonl` file under `<root>/store`.
///
/// `findings` names each offending file and line number only — never the
/// matched text, so a leak report cannot itself leak the thing it found.
pub struct Report {
    pub files: usize,
    pub findings: Vec<String>,
}

static EMAIL_AT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\w@\w").expect("valid email-at regex"));

/// Strings at or under this length are never flagged by the oversized-string
/// check, matching `shape::string_bucket`'s smallest non-trivial bucket.
const MAX_SAFE_STRING: usize = 128;

/// Prefixes of every shaped-string form produced by `shape::of` and
/// `record::safe_string_bucket`. A string starting with one of these is
/// content-free metadata, not raw payload, however long it is.
const SHAPE_TOKEN_PREFIXES: [&str; 6] = ["bool:", "num:", "str<", "str>", "enum:", "url:"];
/// Exact shaped-string forms with no length component.
const SHAPE_TOKEN_EXACT: [&str; 2] = ["null", "uuid"];

/// Scans every `*.jsonl` file directly under `<root>/store` for text that
/// looks unredacted: an `@` between word characters, a home-directory path,
/// `token=`, `password`, or a string value longer than 128 bytes that isn't
/// one of the known shape tokens. This is a minimum smoke scan, not proof
/// that arbitrary stored metadata is non-sensitive.
pub fn check_redaction(root: &Path) -> anyhow::Result<Report> {
    let store_dir = root.join("store");
    let mut paths: Vec<_> = std::fs::read_dir(&store_dir)
        .with_context(|| format!("reading {}", store_dir.display()))?
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| path.is_file())
        .filter(|path| path.extension().and_then(|ext| ext.to_str()) == Some("jsonl"))
        .collect();
    paths.sort();

    let mut findings = Vec::new();
    for path in &paths {
        let body =
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        for (index, line) in body.lines().enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            if line_looks_unredacted(line) {
                findings.push(format!("{}:{}", path.display(), index + 1));
            }
        }
    }

    Ok(Report {
        files: paths.len(),
        findings,
    })
}

fn line_looks_unredacted(line: &str) -> bool {
    if EMAIL_AT.is_match(line) {
        return true;
    }
    let lower = line.to_ascii_lowercase();
    if lower.contains("/users/") || lower.contains("/home/") {
        return true;
    }
    if lower.contains("token=") || lower.contains("password") {
        return true;
    }
    serde_json::from_str::<Value>(line)
        .map(|value| has_oversized_string(&value))
        .unwrap_or(false)
}

fn has_oversized_string(value: &Value) -> bool {
    match value {
        Value::String(s) => s.len() > MAX_SAFE_STRING && !is_shape_token(s),
        Value::Array(items) => items.iter().any(has_oversized_string),
        Value::Object(map) => map.values().any(has_oversized_string),
        Value::Null | Value::Bool(_) | Value::Number(_) => false,
    }
}

fn is_shape_token(s: &str) -> bool {
    SHAPE_TOKEN_EXACT.contains(&s)
        || SHAPE_TOKEN_PREFIXES
            .iter()
            .any(|prefix| s.starts_with(prefix))
}
