use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{json, Value};

use crate::fingerprint::{self, Salt};
use crate::privacy;
use crate::{errtemplate, shape};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CallRecord {
    pub ts: String,
    pub session: String,
    pub seq: u64,
    pub server: String,
    pub method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub args: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latency_ms: Option<u64>,
    pub outcome: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<ErrorInfo>,
    pub shim_self_us: u64,
    pub kind: String,
}

impl CallRecord {
    /// Returns the only representation allowed to cross a persistence boundary.
    /// Applying this projection repeatedly is safe, so both JSONL writes and
    /// later index rebuilds enforce the same boundary independently.
    pub fn sanitized(&self) -> Self {
        let mut safe = self.clone();
        if chrono::DateTime::parse_from_rfc3339(&safe.ts).is_err() {
            safe.ts = "unknown".into();
        }
        safe.session = privacy::opaque_session(&safe.session);
        if !privacy::valid_server(&safe.server) {
            safe.server = "invalid".into();
        }
        if !privacy::valid_method(&safe.method) {
            safe.method = "unparsed/metadata".into();
        }
        if safe
            .tool
            .as_deref()
            .is_some_and(|tool| !privacy::valid_tool(tool))
        {
            safe.tool = None;
        }
        safe.args = safe.args.as_ref().map(sanitize_shape);
        if !matches!(
            safe.outcome.as_str(),
            "ok" | "error" | "unparsed" | "unknown"
        ) {
            safe.outcome = "unknown".into();
        }
        safe.kind = match safe.kind.as_str() {
            "real" => "real".into(),
            "unparsed" => "unparsed".into(),
            _ => "unparsed".into(),
        };
        safe.error = safe.error.as_ref().map(ErrorInfo::sanitized);
        safe
    }
}

/// The kinds of finding only an agent can observe — a call succeeded but
/// changed nothing, a documented path turned out to be blocked, and so on.
pub const ANNOTATION_KINDS: [&str; 5] = [
    "blocked-optimal-path",
    "undocumented-behavior",
    "false-success",
    "instruction-divergence",
    "workaround",
];

const MAX_NOTE_LEN: usize = 240;

/// An agent-authored observation about a call, identified by `(session, seq)`.
/// Unlike every other stored field, `note` is free-form prose by design; it is
/// bounded and scrubbed of control characters so it can neither smuggle a
/// payload nor corrupt the JSONL framing it is written into.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnnotationRecord {
    pub ts: String,
    pub session: String,
    pub seq: u64,
    pub kind: String,
    #[serde(serialize_with = "serialize_bounded_note")]
    pub note: String,
}

/// Re-derives a safe projection of `note` at serialize time regardless of how
/// the record was constructed: strips control characters (newlines included)
/// and truncates to `MAX_NOTE_LEN` characters. Mirrors `ErrorInfo`'s
/// serializers, which never trust that validation already ran upstream.
fn serialize_bounded_note<S>(value: &str, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    let bounded: String = value
        .chars()
        .filter(|c| !c.is_control())
        .take(MAX_NOTE_LEN)
        .collect();
    bounded.serialize(serializer)
}

impl AnnotationRecord {
    /// Checks `kind` against `ANNOTATION_KINDS` and `note` against the length
    /// and control-character bounds. Returns a descriptive error listing the
    /// valid kinds when `kind` is unrecognized.
    pub fn validate(&self) -> anyhow::Result<()> {
        if !ANNOTATION_KINDS.contains(&self.kind.as_str()) {
            anyhow::bail!(
                "unknown annotation kind {:?}; valid kinds are: {}",
                self.kind,
                ANNOTATION_KINDS.join(", ")
            );
        }
        if self.note.chars().count() > MAX_NOTE_LEN {
            anyhow::bail!(
                "note exceeds {MAX_NOTE_LEN} characters (got {})",
                self.note.chars().count()
            );
        }
        if self.note.chars().any(char::is_control) {
            anyhow::bail!("note must not contain control characters or newlines");
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct ErrorInfo {
    #[serde(
        default,
        deserialize_with = "deserialize_code",
        skip_serializing_if = "Option::is_none",
        serialize_with = "serialize_code"
    )]
    pub code: Option<Value>,
    #[serde(
        skip_serializing_if = "Option::is_none",
        serialize_with = "serialize_bucketed_string"
    )]
    pub layer: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retryable: Option<bool>,
    #[serde(
        skip_serializing_if = "Option::is_none",
        serialize_with = "serialize_bucketed_string"
    )]
    pub kind: Option<String>,
    #[serde(
        skip_serializing_if = "Option::is_none",
        serialize_with = "serialize_template"
    )]
    pub template: Option<String>,
    #[serde(
        skip_serializing_if = "is_missing_or_invalid_template_id",
        serialize_with = "serialize_template_id"
    )]
    pub template_id: Option<String>,
}

impl ErrorInfo {
    fn sanitized(&self) -> Self {
        Self {
            code: self.code.as_ref().map(privacy_safe_code),
            layer: self.layer.as_deref().map(safe_string_bucket),
            retryable: self.retryable,
            kind: self.kind.as_deref().map(safe_string_bucket),
            template: self.template.as_deref().map(errtemplate::normalize),
            template_id: self
                .template_id
                .as_deref()
                .filter(|id| is_valid_template_id(id))
                .map(str::to_owned),
        }
    }
}

fn sanitize_shape(value: &Value) -> Value {
    match value {
        Value::String(value) if is_canonical_shape(value) => value.clone().into(),
        Value::String(value) => Value::String(shape::string_bucket(value)),
        Value::Null => json!("null"),
        Value::Bool(value) => json!(format!("bool:{value}")),
        Value::Number(value) => json!(format!("num:{value}")),
        Value::Array(items) => json!({
            "array": items.len(),
            "items": items.first().map(sanitize_shape).unwrap_or_else(|| json!("empty"))
        }),
        Value::Object(fields) => Value::Object(
            fields
                .iter()
                .map(|(key, value)| {
                    let safe_value = if matches!(key.as_str(), "array" | "object") && value.is_u64()
                    {
                        value.clone()
                    } else {
                        sanitize_shape(value)
                    };
                    (key.clone(), safe_value)
                })
                .collect(),
        ),
    }
}

fn is_canonical_shape(value: &str) -> bool {
    if matches!(
        value,
        "null"
            | "bool:true"
            | "bool:false"
            | "uuid"
            | "empty"
            | "str<8"
            | "str<32"
            | "str<128"
            | "str<512"
            | "str<4096"
            | "str>4096"
    ) {
        return true;
    }
    if let Some(number) = value.strip_prefix("num:") {
        return number.parse::<serde_json::Number>().is_ok();
    }
    if let Some(domain) = value.strip_prefix("url:") {
        return !domain.is_empty()
            && domain.len() <= 253
            && domain
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-'));
    }
    value.strip_prefix("enum:").is_some_and(privacy::valid_tool)
}

/// Lifts the four allowed keys out of an error payload and replaces every
/// string with content-free metadata. Every other key is dropped. The salt
/// is used only to derive `template_id`; the raw message never leaves this
/// function.
pub fn error_info(payload: &Value, salt: &Salt) -> ErrorInfo {
    let inner = payload.get("error").unwrap_or(payload);
    let message = inner.get("message").and_then(Value::as_str);
    ErrorInfo {
        code: inner.get("code").map(privacy_safe_code),
        layer: inner
            .get("layer")
            .and_then(Value::as_str)
            .map(safe_string_bucket),
        retryable: inner.get("retryable").and_then(Value::as_bool),
        kind: inner
            .get("kind")
            .and_then(Value::as_str)
            .map(safe_string_bucket),
        template: message.map(errtemplate::normalize),
        template_id: message.map(|message| fingerprint::template_id(salt, message)),
    }
}

fn privacy_safe_code(value: &Value) -> Value {
    match value {
        Value::String(value) => Value::String(if privacy::valid_identifier(value) {
            value.clone()
        } else {
            safe_string_bucket(value)
        }),
        Value::Array(items) => json!({ "array": items.len() }),
        Value::Object(fields) if is_container_shape(fields) => value.clone(),
        Value::Object(fields) => json!({ "object": fields.len() }),
        Value::Null | Value::Bool(_) | Value::Number(_) => value.clone(),
    }
}

fn is_container_shape(fields: &serde_json::Map<String, Value>) -> bool {
    fields.len() == 1
        && ["array", "object"]
            .into_iter()
            .any(|key| fields.get(key).is_some_and(Value::is_u64))
}

fn safe_string_bucket(value: &str) -> String {
    if matches!(
        value,
        "str<8" | "str<32" | "str<128" | "str<512" | "str<4096" | "str>4096"
    ) {
        value.to_owned()
    } else {
        shape::string_bucket(value)
    }
}

fn serialize_code<S>(value: &Option<Value>, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    value.as_ref().map(privacy_safe_code).serialize(serializer)
}

fn deserialize_code<'de, D>(deserializer: D) -> Result<Option<Value>, D::Error>
where
    D: Deserializer<'de>,
{
    Value::deserialize(deserializer).map(Some)
}

fn serialize_bucketed_string<S>(value: &Option<String>, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    value
        .as_deref()
        .map(safe_string_bucket)
        .serialize(serializer)
}

fn serialize_template<S>(value: &Option<String>, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    value
        .as_deref()
        .map(errtemplate::normalize)
        .serialize(serializer)
}

/// A `template_id` is only ever a fingerprint minted by
/// `fingerprint::template_id`: exactly 16 lowercase hex characters. A
/// directly-constructed value that doesn't match this shape could carry
/// arbitrary content, so it is dropped rather than serialized verbatim.
fn is_valid_template_id(value: &str) -> bool {
    value.len() == 16
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn serialize_template_id<S>(value: &Option<String>, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    value
        .as_deref()
        .filter(|id| is_valid_template_id(id))
        .serialize(serializer)
}

/// `skip_serializing_if` must agree with what `serialize_template_id`
/// actually serializes, or an invalid value is written as an explicit
/// `null` instead of being omitted like every other dropped field: `serde`
/// only consults `skip_serializing_if` on the raw field, before
/// `serialize_with` runs, so it has to independently reject the same
/// invalid shapes.
fn is_missing_or_invalid_template_id(value: &Option<String>) -> bool {
    !value.as_deref().is_some_and(is_valid_template_id)
}
