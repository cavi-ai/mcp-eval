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
/// one of the known shape tokens. In `annotations-*.jsonl` files, the
/// intentionally free-form `note` field is exempt from every detector; all
/// other fields there, and every field in every other file, are in scope.
/// This is a minimum smoke scan, not proof that arbitrary stored metadata is
/// non-sensitive.
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
        // `annotations-*.jsonl` carries one intentionally free-form field
        // (`note`); every other file's content is fully in scope.
        let is_annotations = path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with("annotations-"));
        let body =
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        for (index, line) in body.lines().enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            let flagged = if is_annotations {
                annotation_line_looks_unredacted(line)
            } else {
                line_looks_unredacted(line)
            };
            if flagged {
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
    text_patterns_match(line) || value_has_oversized_string(line)
}

/// Same detectors as `line_looks_unredacted`, but with the annotation
/// record's `note` field excluded first: `note` is deliberately free-form
/// prose (see `record::AnnotationRecord`), so an `@` or a long sentence in
/// it is not a redaction bug. Every other field in the line — `ts`,
/// `session`, `seq`, `kind`, and anything else present — is still checked.
/// A line that fails to parse as a JSON object can't have its `note` safely
/// isolated, so it falls back to the full, unscoped scan.
fn annotation_line_looks_unredacted(line: &str) -> bool {
    let Ok(Value::Object(mut fields)) = serde_json::from_str::<Value>(line) else {
        return line_looks_unredacted(line);
    };
    fields.remove("note");
    let without_note = Value::Object(fields);
    let serialized = serde_json::to_string(&without_note).unwrap_or_default();
    text_patterns_match(&serialized) || has_oversized_string(&without_note)
}

fn text_patterns_match(text: &str) -> bool {
    if EMAIL_AT.is_match(text) {
        return true;
    }
    let lower = text.to_ascii_lowercase();
    lower.contains("/users/")
        || lower.contains("/home/")
        || lower.contains("token=")
        || lower.contains("password")
}

fn value_has_oversized_string(line: &str) -> bool {
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
