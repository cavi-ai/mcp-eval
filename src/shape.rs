use std::collections::{HashMap, HashSet};

use serde_json::{json, Map, Value};

/// Enum values declared by each tool's own input schema, keyed by tool then
/// dotted property path. Only values learned here are ever stored verbatim.
#[derive(Debug, Default)]
pub struct EnumIndex {
    by_tool: HashMap<String, HashMap<String, HashSet<String>>>,
}

impl EnumIndex {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn learn(&mut self, tool: &str, schema: &Value) {
        let mut found: HashMap<String, HashSet<String>> = HashMap::new();
        walk_schema(schema, String::new(), &mut found);
        self.by_tool.insert(tool.to_string(), found);
    }

    pub fn is_enum(&self, tool: &str, path: &str, value: &str) -> bool {
        self.by_tool
            .get(tool)
            .and_then(|paths| paths.get(path))
            .is_some_and(|vals| vals.contains(value))
    }
}

fn walk_schema(schema: &Value, path: String, out: &mut HashMap<String, HashSet<String>>) {
    if let Some(values) = schema.get("enum").and_then(Value::as_array) {
        let set: HashSet<String> = values
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect();
        if !set.is_empty() {
            out.insert(path.clone(), set);
        }
    }
    for key in ["oneOf", "anyOf", "allOf"] {
        if let Some(list) = schema.get(key).and_then(Value::as_array) {
            for branch in list {
                walk_schema(branch, path.clone(), out);
            }
        }
    }
    if let Some(props) = schema.get("properties").and_then(Value::as_object) {
        for (name, sub) in props {
            let child = if path.is_empty() {
                name.clone()
            } else {
                format!("{path}.{name}")
            };
            walk_schema(sub, child, out);
        }
    }
    if let Some(items) = schema.get("items") {
        walk_schema(items, format!("{path}[]"), out);
    }
}

pub fn of(value: &Value, tool: &str, enums: &EnumIndex) -> Value {
    shape_at(value, tool, "", enums)
}

fn shape_at(value: &Value, tool: &str, path: &str, enums: &EnumIndex) -> Value {
    match value {
        Value::Null => json!("null"),
        Value::Bool(b) => json!(format!("bool:{b}")),
        Value::Number(n) => json!(format!("num:{n}")),
        Value::String(s) => json!(string_shape(s, tool, path, enums)),
        Value::Array(items) => {
            let first = items
                .first()
                .map(|v| shape_at(v, tool, &format!("{path}[]"), enums))
                .unwrap_or_else(|| json!("empty"));
            json!({ "array": items.len(), "items": first })
        }
        Value::Object(map) => {
            let mut out = Map::new();
            for (key, sub) in map {
                let child = if path.is_empty() {
                    key.clone()
                } else {
                    format!("{path}.{key}")
                };
                out.insert(key.clone(), shape_at(sub, tool, &child, enums));
            }
            Value::Object(out)
        }
    }
}

fn string_shape(s: &str, tool: &str, path: &str, enums: &EnumIndex) -> String {
    if enums.is_enum(tool, path, s) {
        return format!("enum:{s}");
    }
    if is_uuid(s) {
        return "uuid".to_string();
    }
    if let Some(domain) = registrable_domain(s) {
        return format!("url:{domain}");
    }
    for bucket in [8usize, 32, 128, 512, 4096] {
        if s.len() <= bucket {
            return format!("str<{bucket}");
        }
    }
    "str>4096".to_string()
}

fn is_uuid(s: &str) -> bool {
    uuid::Uuid::parse_str(s).is_ok()
}

fn registrable_domain(s: &str) -> Option<String> {
    let rest = s
        .strip_prefix("https://")
        .or_else(|| s.strip_prefix("http://"))?;
    let host = rest
        .split(['/', '?', '#'])
        .next()
        .unwrap_or("")
        .split('@')
        .next_back()
        .unwrap_or("")
        .split(':')
        .next()
        .unwrap_or("");
    if host.is_empty() {
        return None;
    }
    Some(host.strip_prefix("www.").unwrap_or(host).to_string())
}
