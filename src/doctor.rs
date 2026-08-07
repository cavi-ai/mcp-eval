use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use anyhow::Context;
use regex::Regex;
use serde_json::Value;

use crate::fingerprint::SALT_FILENAME;

/// Result of a redaction sweep over every `*.jsonl` file under `<root>/store`.
///
/// `findings` names each offending file and line number only — never the
/// matched text, so a leak report cannot itself leak the thing it found.
pub struct Report {
    pub files: usize,
    pub findings: Vec<String>,
    /// Count of non-empty annotation `note` fields seen across every
    /// `annotations-*.jsonl` file. `note` is deliberately exempt from every
    /// detector below (it is free-form prose by design), so this is not a
    /// failing check — it is a heads-up count so a mistyped or omitted
    /// review step doesn't silently pass on prose nobody read.
    pub notes_requiring_review: usize,
    /// Path to the fingerprint salt. Never inside `store/`; must never
    /// accompany the store when it is shared.
    pub salt_path: PathBuf,
}

static EMAIL_AT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\w@\w").expect("valid email-at regex"));

/// Strings at or under this length are never flagged by the oversized-string
/// check. This is an arbitrary judgment call — the size above which an
/// unrecognized string is presumed to be unredacted content — not a value
/// derived from `shape::string_bucket`'s bucket ladder (`[8, 32, 128, 512,
/// 4096]`), of which 128 is merely the third of five, not the smallest.
const MAX_SAFE_STRING: usize = 128;

/// Exact shaped-string forms with no variable component: `null`, `uuid`, and
/// every rung of `shape::string_bucket`'s closed bucket ladder. These must
/// match exactly, not by prefix — the bucket ladder is a fixed, finite set
/// (`str<8`, `str<32`, `str<128`, `str<512`, `str<4096`, `str>4096`), so a
/// prefix match would let arbitrary trailing bytes hide behind a
/// legitimate-looking token, e.g. `str<8` followed by 4KB of unredacted text.
const SHAPE_TOKEN_EXACT: [&str; 8] = [
    "null", "uuid", "str<8", "str<32", "str<128", "str<512", "str<4096", "str>4096",
];

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
    let mut notes_requiring_review = 0usize;
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
                if line_has_nonempty_note(line) {
                    notes_requiring_review += 1;
                }
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
        notes_requiring_review,
        salt_path: root.join(SALT_FILENAME),
    })
}

/// Whether this annotation line carries a non-empty `note`. Used only to
/// count prose that automated redaction cannot vouch for — never to decide
/// pass/fail, since `note` is exempt from every detector by design.
fn line_has_nonempty_note(line: &str) -> bool {
    let Ok(value) = serde_json::from_str::<Value>(line) else {
        return false;
    };
    value
        .get("note")
        .and_then(Value::as_str)
        .is_some_and(|note| !note.trim().is_empty())
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

/// Whether `s` is content-free shaped metadata rather than raw payload.
/// The closed forms (`null`, `uuid`, the bucket ladder) must match exactly.
/// `bool:`, `num:`, and `url:` are validated past the prefix because their
/// suffix is meaningful, not free text: `shape::of` only ever emits `true`
/// or `false` after `bool:`, a JSON-number literal after `num:`, and a
/// host — `ip`, `localhost`, `host`, or a registrable domain — after `url:`.
/// `enum:` stays prefix-based: a schema-declared enum member is retained
/// verbatim by design and can legitimately be long.
fn is_shape_token(s: &str) -> bool {
    if SHAPE_TOKEN_EXACT.contains(&s) {
        return true;
    }
    if let Some(rest) = s.strip_prefix("bool:") {
        return rest == "true" || rest == "false";
    }
    if let Some(rest) = s.strip_prefix("num:") {
        return rest.parse::<f64>().is_ok();
    }
    if let Some(rest) = s.strip_prefix("url:") {
        return looks_like_host(rest);
    }
    s.starts_with("enum:")
}

/// Whether `s` is `ip`, `localhost`, `host`, or a plausible DNS name: one or
/// more dot-separated labels of ASCII letters, digits, and hyphens, each
/// 1-63 bytes, within the 253-byte overall DNS limit. Not a full RFC 1035
/// validator — just enough to reject a `url:`-prefixed value that is really
/// unredacted content rather than the short host strings `shape::of` emits.
fn looks_like_host(s: &str) -> bool {
    if matches!(s, "ip" | "localhost" | "host") {
        return true;
    }
    !s.is_empty()
        && s.len() <= 253
        && s.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && label
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        })
}
