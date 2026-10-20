//! The catalog's contract with its callers: which tools exist, each tool's
//! class, which parameters it requires and accepts, how each parameter is
//! constrained, and its output schema. Parameter names and schemas appear
//! only as SHA-256 digests, so a report carries no schema text, yet two
//! reports' contracts can be compared for changes that break callers.

use std::collections::{BTreeMap, BTreeSet};

use anyhow::{bail, Context};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::mcp_client::{ToolCatalog, ToolDefinition};
use crate::standard::ToolClass;

/// Hex digits kept from each SHA-256: enough to tell contracts apart.
const DIGEST_HEX: usize = 16;
/// `$ref` hops followed while digesting a constraint.
const MAX_REF_DEPTH: usize = 8;
/// Schema keywords that carry prose, not constraints.
const PROSE: [&str; 5] = ["description", "title", "examples", "$comment", "default"];
/// Keywords whose object keys are names, not keywords.
const NAME_MAPS: [&str; 5] = [
    "properties",
    "patternProperties",
    "$defs",
    "definitions",
    "dependentSchemas",
];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolContract {
    pub name: String,
    pub class: ToolClass,
    /// Digests of the required parameter names, sorted.
    pub required: BTreeSet<String>,
    /// Parameter-name digest to constraint digest.
    pub parameters: BTreeMap<String, String>,
    /// Constraint digest of the declared output schema.
    pub output_schema: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CatalogContract {
    /// Sorted by name.
    pub tools: Vec<ToolContract>,
}

/// How a change affects a caller written against the baseline.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    /// A call that worked against the baseline can now fail or do harm.
    Breaking,
    /// A constraint changed; whether it narrowed needs a reviewer.
    Review,
    /// Additive or relaxing.
    Compatible,
}

impl Severity {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Breaking => "breaking",
            Self::Review => "review",
            Self::Compatible => "compatible",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChangeKind {
    ToolRemoved,
    ToolAdded,
    /// A parameter joined the required set.
    ParameterRequired,
    /// A required parameter became optional.
    ParameterOptional,
    ParameterRemoved,
    /// A new optional parameter.
    ParameterAdded,
    ParameterChanged,
    ClassChanged {
        from: ToolClass,
        to: ToolClass,
    },
    OutputSchemaRemoved,
    OutputSchemaAdded,
    OutputSchemaChanged,
}

impl ChangeKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ToolRemoved => "tool-removed",
            Self::ToolAdded => "tool-added",
            Self::ParameterRequired => "parameter-required",
            Self::ParameterOptional => "parameter-optional",
            Self::ParameterRemoved => "parameter-removed",
            Self::ParameterAdded => "parameter-added",
            Self::ParameterChanged => "parameter-changed",
            Self::ClassChanged { .. } => "class-changed",
            Self::OutputSchemaRemoved => "output-schema-removed",
            Self::OutputSchemaAdded => "output-schema-added",
            Self::OutputSchemaChanged => "output-schema-changed",
        }
    }

    pub fn severity(self) -> Severity {
        match self {
            Self::ToolRemoved
            | Self::ParameterRequired
            | Self::ParameterRemoved
            | Self::OutputSchemaRemoved => Severity::Breaking,
            // A tool that becomes a writer, or loses its read-only
            // declaration, is no longer safe to call unattended.
            Self::ClassChanged {
                to: ToolClass::Writer,
                ..
            }
            | Self::ClassChanged {
                from: ToolClass::ReadOnly,
                to: ToolClass::Unannotated,
            } => Severity::Breaking,
            Self::ClassChanged { .. } => Severity::Compatible,
            Self::ParameterChanged | Self::OutputSchemaChanged => Severity::Review,
            Self::ToolAdded
            | Self::ParameterOptional
            | Self::ParameterAdded
            | Self::OutputSchemaAdded => Severity::Compatible,
        }
    }
}

/// One change to one tool; `count` parameters changed the same way.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Change {
    pub tool: String,
    pub kind: ChangeKind,
    pub count: u64,
}

impl Change {
    pub fn label(&self) -> String {
        let mut label = self.kind.as_str().to_owned();
        if let ChangeKind::ClassChanged { from, to } = self.kind {
            label.push_str(&format!(" {} → {}", class_label(from), class_label(to)));
        }
        if self.count > 1 {
            label.push_str(&format!(" ×{}", self.count));
        }
        label
    }
}

pub fn class_label(class: ToolClass) -> &'static str {
    match class {
        ToolClass::ReadOnly => "read-only",
        ToolClass::Writer => "writer",
        ToolClass::Unannotated => "unannotated",
    }
}

fn class_from_label(label: &str) -> Option<ToolClass> {
    [
        ToolClass::ReadOnly,
        ToolClass::Writer,
        ToolClass::Unannotated,
    ]
    .into_iter()
    .find(|class| class_label(*class) == label)
}

fn digest(bytes: &[u8]) -> String {
    Sha256::digest(bytes)[..DIGEST_HEX / 2]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Canonical JSON of the constraints in `schema`: sorted keys, prose
/// keywords dropped, local `$ref`s replaced by their targets.
fn constraints(schema: &Value, root: &Value, depth: usize, names: bool) -> Value {
    match schema {
        Value::Object(map) => {
            if let Some(target) = map
                .get("$ref")
                .and_then(Value::as_str)
                .and_then(|reference| reference.strip_prefix('#'))
                .filter(|_| depth < MAX_REF_DEPTH && !names)
                .and_then(|pointer| root.pointer(pointer))
            {
                return constraints(target, root, depth + 1, false);
            }
            let sorted: BTreeMap<&String, Value> = map
                .iter()
                .filter(|(key, _)| names || !PROSE.contains(&key.as_str()))
                .map(|(key, value)| {
                    let child_names = !names && NAME_MAPS.contains(&key.as_str());
                    (key, constraints(value, root, depth, child_names))
                })
                .collect();
            Value::Object(sorted.into_iter().map(|(k, v)| (k.clone(), v)).collect())
        }
        Value::Array(items) => Value::Array(
            items
                .iter()
                .map(|item| constraints(item, root, depth, false))
                .collect(),
        ),
        other => other.clone(),
    }
}

/// Serialize with keys in sorted order regardless of how the map stores
/// them.
fn canonical(value: &Value, out: &mut String) {
    match value {
        Value::Object(map) => {
            let sorted: BTreeMap<&String, &Value> = map.iter().collect();
            out.push('{');
            for (index, (key, value)) in sorted.into_iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                out.push_str(&Value::String(key.clone()).to_string());
                out.push(':');
                canonical(value, out);
            }
            out.push('}');
        }
        Value::Array(items) => {
            out.push('[');
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                canonical(item, out);
            }
            out.push(']');
        }
        other => out.push_str(&other.to_string()),
    }
}

fn constraint_digest(schema: &Value, root: &Value) -> String {
    let mut text = String::new();
    canonical(&constraints(schema, root, 0, false), &mut text);
    digest(text.as_bytes())
}

impl ToolContract {
    pub fn of(tool: &ToolDefinition) -> Self {
        let schema = &tool.input_schema;
        let required = schema
            .get("required")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(|name| digest(name.as_bytes()))
            .collect();
        let parameters = schema
            .get("properties")
            .and_then(Value::as_object)
            .into_iter()
            .flatten()
            .map(|(name, property)| (digest(name.as_bytes()), constraint_digest(property, schema)))
            .collect();
        Self {
            name: tool.name.clone(),
            class: ToolClass::of(tool),
            required,
            parameters,
            output_schema: tool
                .declared_output_schema()
                .map(|output| constraint_digest(output, output)),
        }
    }
}

impl CatalogContract {
    pub fn of(catalog: &ToolCatalog) -> Self {
        let mut tools: Vec<ToolContract> = catalog.tools.iter().map(ToolContract::of).collect();
        tools.sort_by(|a, b| a.name.cmp(&b.name));
        Self { tools }
    }

    pub fn to_json(&self) -> Value {
        json!({
            "tools": self.tools.iter().map(|tool| json!({
                "name": tool.name,
                "class": class_label(tool.class),
                "required": tool.required,
                "parameters": tool.parameters,
                "output_schema": tool.output_schema,
            })).collect::<Vec<_>>(),
        })
    }

    /// Parse the `catalog` object of an `mcpeval.probe-report/v2` document.
    pub fn from_json(value: &Value) -> anyhow::Result<Self> {
        let hex = |value: &Value| -> anyhow::Result<String> {
            let text = value.as_str().context("catalog digest must be a string")?;
            if text.len() != DIGEST_HEX || !text.bytes().all(|b| b.is_ascii_hexdigit()) {
                bail!("catalog digest must be {DIGEST_HEX} hex digits");
            }
            Ok(text.to_owned())
        };
        let tools = value
            .get("tools")
            .and_then(Value::as_array)
            .context("catalog is missing tools")?
            .iter()
            .map(|tool| {
                let name = tool
                    .get("name")
                    .and_then(Value::as_str)
                    .filter(|name| crate::privacy::valid_tool(name))
                    .context("catalog tool has no valid name")?;
                let class = tool
                    .get("class")
                    .and_then(Value::as_str)
                    .and_then(class_from_label)
                    .context("catalog tool has no known class")?;
                let required = tool
                    .get("required")
                    .and_then(Value::as_array)
                    .context("catalog tool is missing required")?
                    .iter()
                    .map(hex)
                    .collect::<anyhow::Result<_>>()?;
                let parameters = tool
                    .get("parameters")
                    .and_then(Value::as_object)
                    .context("catalog tool is missing parameters")?
                    .iter()
                    .map(|(name, constraint)| Ok((hex(&json!(name))?, hex(constraint)?)))
                    .collect::<anyhow::Result<_>>()?;
                let output_schema = match tool.get("output_schema") {
                    None | Some(Value::Null) => None,
                    Some(value) => Some(hex(value)?),
                };
                Ok(ToolContract {
                    name: name.to_owned(),
                    class,
                    required,
                    parameters,
                    output_schema,
                })
            })
            .collect::<anyhow::Result<Vec<_>>>()?;
        Ok(Self { tools })
    }
}

/// Every change from `baseline` to `current`, breaking first, then by tool.
pub fn compare(baseline: &CatalogContract, current: &CatalogContract) -> Vec<Change> {
    let mut changes = Vec::new();
    let mut push = |tool: &str, kind: ChangeKind, count: usize| {
        if count > 0 {
            changes.push(Change {
                tool: tool.to_owned(),
                kind,
                count: count as u64,
            });
        }
    };
    for before in &baseline.tools {
        let Some(after) = current.tools.iter().find(|tool| tool.name == before.name) else {
            push(&before.name, ChangeKind::ToolRemoved, 1);
            continue;
        };
        let name = before.name.as_str();
        if before.class != after.class {
            push(
                name,
                ChangeKind::ClassChanged {
                    from: before.class,
                    to: after.class,
                },
                1,
            );
        }
        push(
            name,
            ChangeKind::ParameterRequired,
            after.required.difference(&before.required).count(),
        );
        push(
            name,
            ChangeKind::ParameterOptional,
            before
                .required
                .difference(&after.required)
                .filter(|parameter| after.parameters.contains_key(*parameter))
                .count(),
        );
        push(
            name,
            ChangeKind::ParameterRemoved,
            before
                .parameters
                .keys()
                .filter(|parameter| !after.parameters.contains_key(*parameter))
                .count(),
        );
        push(
            name,
            ChangeKind::ParameterAdded,
            after
                .parameters
                .keys()
                .filter(|parameter| {
                    !before.parameters.contains_key(*parameter)
                        && !after.required.contains(*parameter)
                })
                .count(),
        );
        push(
            name,
            ChangeKind::ParameterChanged,
            before
                .parameters
                .iter()
                .filter(|(parameter, constraint)| {
                    after
                        .parameters
                        .get(*parameter)
                        .is_some_and(|now| now != *constraint)
                })
                .count(),
        );
        match (&before.output_schema, &after.output_schema) {
            (Some(_), None) => push(name, ChangeKind::OutputSchemaRemoved, 1),
            (None, Some(_)) => push(name, ChangeKind::OutputSchemaAdded, 1),
            (Some(a), Some(b)) if a != b => push(name, ChangeKind::OutputSchemaChanged, 1),
            _ => {}
        }
    }
    for after in &current.tools {
        if !baseline.tools.iter().any(|tool| tool.name == after.name) {
            push(&after.name, ChangeKind::ToolAdded, 1);
        }
    }
    changes.sort_by(|a, b| {
        a.kind
            .severity()
            .cmp(&b.kind.severity())
            .then_with(|| a.tool.cmp(&b.tool))
    });
    changes
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tool(
        name: &str,
        schema: Value,
        annotations: (Option<bool>, Option<bool>),
    ) -> ToolDefinition {
        ToolDefinition {
            name: name.into(),
            input_schema: schema,
            entry_bytes: 0,
            description_chars: 0,
            output_schema: None,
            read_only_hint: annotations.0,
            destructive_hint: annotations.1,
        }
    }

    fn contract(tools: Vec<ToolDefinition>) -> CatalogContract {
        CatalogContract::of(&ToolCatalog {
            tools,
            encoded_bytes: 0,
        })
    }

    fn labels(changes: &[Change]) -> Vec<(String, String, &'static str)> {
        changes
            .iter()
            .map(|change| {
                (
                    change.tool.clone(),
                    change.label(),
                    change.kind.severity().as_str(),
                )
            })
            .collect()
    }

    const READ: (Option<bool>, Option<bool>) = (Some(true), None);

    #[test]
    fn prose_does_not_move_a_constraint_digest_and_a_type_does() {
        let schema = |description: &str, kind: &str| {
            json!({
                "type": "object",
                "properties": {
                    "path": {"type": kind, "description": description},
                    "description": {"type": "string"}
                },
                "required": ["path"]
            })
        };
        let a = ToolContract::of(&tool("read", schema("a path", "string"), READ));
        let b = ToolContract::of(&tool("read", schema("the file path", "string"), READ));
        let c = ToolContract::of(&tool("read", schema("a path", "integer"), READ));
        assert_eq!(a, b);
        assert_ne!(a.parameters, c.parameters);
        // A property named `description` is a parameter, not prose.
        assert_eq!(a.parameters.len(), 2);
        let text = CatalogContract { tools: vec![a] }.to_json().to_string();
        assert!(!text.contains("path") && !text.contains("string"), "{text}");
    }

    #[test]
    fn refs_are_followed_into_the_digest() {
        let schema = |kind: &str| {
            json!({
                "type": "object",
                "$defs": {"Path": {"type": kind}},
                "properties": {"path": {"$ref": "#/$defs/Path"}}
            })
        };
        let a = ToolContract::of(&tool("read", schema("string"), READ));
        let b = ToolContract::of(&tool("read", schema("integer"), READ));
        assert_ne!(a.parameters, b.parameters);
    }

    #[test]
    fn removals_new_requirements_and_writer_flips_break_callers() {
        let baseline = contract(vec![
            tool(
                "read",
                json!({"type": "object", "properties": {"path": {"type": "string"}, "depth": {"type": "integer"}}, "required": ["path"]}),
                READ,
            ),
            tool("status", json!({"type": "object"}), READ),
            tool("gone", json!({"type": "object"}), READ),
            tool("publish", json!({"type": "object"}), READ),
        ]);
        let current = contract(vec![
            tool(
                "read",
                json!({"type": "object", "properties": {"path": {"type": "integer"}, "mode": {"type": "string"}, "limit": {"type": "integer"}}, "required": ["path", "mode"]}),
                READ,
            ),
            tool("status", json!({"type": "object"}), (None, None)),
            tool("publish", json!({"type": "object"}), (None, Some(true))),
            tool("added", json!({"type": "object"}), READ),
        ]);
        assert_eq!(
            labels(&compare(&baseline, &current)),
            vec![
                ("gone".into(), "tool-removed".into(), "breaking"),
                (
                    "publish".into(),
                    "class-changed read-only → writer".into(),
                    "breaking"
                ),
                ("read".into(), "parameter-required".into(), "breaking"),
                ("read".into(), "parameter-removed".into(), "breaking"),
                (
                    "status".into(),
                    "class-changed read-only → unannotated".into(),
                    "breaking"
                ),
                ("read".into(), "parameter-changed".into(), "review"),
                ("added".into(), "tool-added".into(), "compatible"),
                ("read".into(), "parameter-added".into(), "compatible"),
            ]
        );
        assert!(compare(&current, &current).is_empty());
    }

    #[test]
    fn relaxing_a_requirement_is_compatible() {
        let strict = contract(vec![tool(
            "read",
            json!({"type": "object", "properties": {"a": {}, "b": {}}, "required": ["a", "b"]}),
            READ,
        )]);
        let relaxed = contract(vec![tool(
            "read",
            json!({"type": "object", "properties": {"a": {}, "b": {}}}),
            READ,
        )]);
        assert_eq!(
            labels(&compare(&strict, &relaxed)),
            vec![("read".into(), "parameter-optional ×2".into(), "compatible")]
        );
        assert_eq!(
            labels(&compare(&relaxed, &strict)),
            vec![("read".into(), "parameter-required ×2".into(), "breaking")]
        );
    }

    #[test]
    fn a_contract_round_trips_through_json() {
        let original = contract(vec![
            tool(
                "read",
                json!({"type": "object", "properties": {"path": {"type": "string"}}, "required": ["path"]}),
                READ,
            ),
            tool("write", json!({"type": "object"}), (Some(false), None)),
        ]);
        assert_eq!(
            CatalogContract::from_json(&original.to_json()).unwrap(),
            original
        );
        assert!(CatalogContract::from_json(
            &json!({"tools": [{"name": "x", "class": "reader", "required": [], "parameters": {}}]})
        )
        .is_err());
    }
}
