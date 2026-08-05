use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{json, Value};

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
}

/// Lifts the four allowed keys out of an error payload and replaces every
/// string with content-free metadata. Every other key is dropped.
pub fn error_info(payload: &Value) -> ErrorInfo {
    let inner = payload.get("error").unwrap_or(payload);
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
        template: inner
            .get("message")
            .and_then(Value::as_str)
            .map(errtemplate::normalize),
    }
}

fn privacy_safe_code(value: &Value) -> Value {
    match value {
        Value::String(value) => Value::String(safe_string_bucket(value)),
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
