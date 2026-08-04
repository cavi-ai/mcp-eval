use std::collections::{HashMap, HashSet};

use serde_json::{json, Map, Value};
use url::{Host, Url};

type Path = Vec<PathSegment>;

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
enum PathSegment {
    Key(String),
    Item,
}

/// Enum values declared by each tool's own input schema, keyed by tool then
/// dotted property path. Only values learned here are ever stored verbatim.
#[derive(Debug, Default)]
pub struct EnumIndex {
    by_tool: HashMap<String, HashMap<Path, HashSet<String>>>,
}

impl EnumIndex {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn learn(&mut self, tool: &str, schema: &Value) {
        let mut found: HashMap<Path, HashSet<String>> = HashMap::new();
        walk_schema(schema, &[], &mut found);
        self.by_tool.insert(tool.to_string(), found);
    }

    /// Returns whether `value` is an enum member at `path` in `tool`'s schema.
    ///
    /// A non-slash path is a literal top-level property name, preserving simple
    /// paths such as `waitUntil`; `[]` is the root-array item path. Nested
    /// paths use typed slash segments: `/k/<key>` for a property and `/i` for
    /// an array item. Within a key, escape `~` as `~0` and `/` as `~1`.
    pub fn is_enum(&self, tool: &str, path: &str, value: &str) -> bool {
        public_path(path).is_some_and(|path| self.is_enum_at(tool, &path, value))
    }

    fn is_enum_at(&self, tool: &str, path: &[PathSegment], value: &str) -> bool {
        self.by_tool
            .get(tool)
            .and_then(|paths| paths.get(path))
            .is_some_and(|vals| vals.contains(value))
    }
}

fn public_path(path: &str) -> Option<Path> {
    if path.is_empty() {
        return Some(Vec::new());
    }
    if path == "[]" {
        return Some(vec![PathSegment::Item]);
    }
    let Some(encoded) = path.strip_prefix('/') else {
        return Some(vec![PathSegment::Key(path.to_string())]);
    };

    let mut segments = Vec::new();
    let mut parts = encoded.split('/');
    while let Some(kind) = parts.next() {
        match kind {
            "k" => segments.push(PathSegment::Key(decode_key(parts.next()?)?)),
            "i" => segments.push(PathSegment::Item),
            _ => return None,
        }
    }
    Some(segments)
}

fn decode_key(key: &str) -> Option<String> {
    let mut decoded = String::new();
    let mut chars = key.chars();
    while let Some(ch) = chars.next() {
        if ch != '~' {
            decoded.push(ch);
            continue;
        }
        match chars.next()? {
            '0' => decoded.push('~'),
            '1' => decoded.push('/'),
            _ => return None,
        }
    }
    Some(decoded)
}

fn walk_schema(schema: &Value, path: &[PathSegment], out: &mut HashMap<Path, HashSet<String>>) {
    if let Some(values) = schema.get("enum").and_then(Value::as_array) {
        let set: HashSet<String> = values
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect();
        if !set.is_empty() {
            out.entry(path.to_vec()).or_default().extend(set);
        }
    }
    for key in ["oneOf", "anyOf", "allOf"] {
        if let Some(list) = schema.get(key).and_then(Value::as_array) {
            for branch in list {
                walk_schema(branch, path, out);
            }
        }
    }
    if let Some(props) = schema.get("properties").and_then(Value::as_object) {
        for (name, sub) in props {
            let mut child = path.to_vec();
            child.push(PathSegment::Key(name.clone()));
            walk_schema(sub, &child, out);
        }
    }
    if let Some(items) = schema.get("items") {
        let mut item_path = path.to_vec();
        item_path.push(PathSegment::Item);
        walk_schema(items, &item_path, out);
    }
}

pub fn of(value: &Value, tool: &str, enums: &EnumIndex) -> Value {
    shape_at(value, tool, &[], enums)
}

fn shape_at(value: &Value, tool: &str, path: &[PathSegment], enums: &EnumIndex) -> Value {
    match value {
        Value::Null => json!("null"),
        Value::Bool(b) => json!(format!("bool:{b}")),
        Value::Number(n) => json!(format!("num:{n}")),
        Value::String(s) => json!(string_shape(s, tool, path, enums)),
        Value::Array(items) => {
            let mut item_path = path.to_vec();
            item_path.push(PathSegment::Item);
            let first = items
                .first()
                .map(|v| shape_at(v, tool, &item_path, enums))
                .unwrap_or_else(|| json!("empty"));
            json!({ "array": items.len(), "items": first })
        }
        Value::Object(map) => {
            let mut out = Map::new();
            for (key, sub) in map {
                let mut child = path.to_vec();
                child.push(PathSegment::Key(key.clone()));
                out.insert(key.clone(), shape_at(sub, tool, &child, enums));
            }
            Value::Object(out)
        }
    }
}

fn string_shape(s: &str, tool: &str, path: &[PathSegment], enums: &EnumIndex) -> String {
    if enums.is_enum_at(tool, path, s) {
        return format!("enum:{s}");
    }
    if is_uuid(s) {
        return "uuid".to_string();
    }
    if let Some(domain) = registrable_domain(s) {
        return format!("url:{domain}");
    }
    string_bucket(s)
}

pub(crate) fn string_bucket(s: &str) -> String {
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
    if s.chars().any(char::is_control) {
        return None;
    }
    let url = Url::parse(s).ok()?;
    if !matches!(url.scheme(), "http" | "https") {
        return None;
    }
    match url.host()? {
        Host::Ipv4(_) | Host::Ipv6(_) => Some("ip".to_string()),
        Host::Domain(host) if host.eq_ignore_ascii_case("localhost") => {
            Some("localhost".to_string())
        }
        Host::Domain(host) => {
            let Some(info) = psl2::analyze(host) else {
                return Some("host".to_string());
            };
            if !info.is_known() {
                return Some("host".to_string());
            }
            Some(
                info.registrable_domain()
                    .unwrap_or("public-suffix")
                    .to_string(),
            )
        }
    }
}
