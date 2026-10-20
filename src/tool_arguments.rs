//! Caller-supplied tool arguments, for tools whose working inputs (a path
//! inside a root, an existing ID) no schema implies. The file is a JSON
//! object mapping each tool name to the arguments object to call it with.
//! Values are used in memory and in the manifest `init` writes; a report
//! carries only the digest of the whole file.

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{bail, Context};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::mcp_client::ToolCatalog;
use crate::standard::ToolClass;

/// Largest arguments file read.
const MAX_FILE_BYTES: u64 = 1024 * 1024;

#[derive(Clone, Debug, PartialEq)]
pub struct ToolArguments {
    by_tool: BTreeMap<String, Value>,
    sha256: String,
}

impl ToolArguments {
    /// Read and shape-check an arguments file. Errors never quote values.
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        let size = std::fs::metadata(path)
            .with_context(|| format!("reading --arguments {}", path.display()))?
            .len();
        if size > MAX_FILE_BYTES {
            bail!("--arguments {} is larger than 1 MiB", path.display());
        }
        let bytes = std::fs::read(path)
            .with_context(|| format!("reading --arguments {}", path.display()))?;
        let document: Value = serde_json::from_slice(&bytes)
            .map_err(|_| anyhow::anyhow!("--arguments {} is not valid JSON", path.display()))?;
        Self::from_value(document)
    }

    pub fn from_value(document: Value) -> anyhow::Result<Self> {
        let Value::Object(entries) = document else {
            bail!("--arguments must be a JSON object mapping tool names to arguments objects");
        };
        let mut by_tool = BTreeMap::new();
        for (tool, arguments) in entries {
            if !crate::privacy::valid_tool(&tool) {
                bail!("--arguments names an invalid tool name");
            }
            if !arguments.is_object() {
                bail!("--arguments for {tool} must be a JSON object");
            }
            by_tool.insert(tool, arguments);
        }
        let mut canonical = String::new();
        crate::catalog_contract::canonical(
            &Value::Object(by_tool.clone().into_iter().collect()),
            &mut canonical,
        );
        let sha256 = Sha256::digest(canonical.as_bytes())
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        Ok(Self { by_tool, sha256 })
    }

    pub fn get(&self, tool: &str) -> Option<&Value> {
        self.by_tool.get(tool)
    }

    /// Lowercase hex SHA-256 of the canonical document.
    pub fn sha256(&self) -> &str {
        &self.sha256
    }

    /// Every named tool must be one the server lists and mcpeval may call,
    /// and its arguments must satisfy the tool's input schema.
    pub fn validate(&self, catalog: &ToolCatalog, confirm_read_only: bool) -> anyhow::Result<()> {
        for (name, arguments) in &self.by_tool {
            let Some(tool) = catalog.tools.iter().find(|tool| &tool.name == name) else {
                bail!("--arguments names {name}, which is not a tool the server lists");
            };
            match ToolClass::of(tool) {
                ToolClass::Writer => bail!(
                    "--arguments names {name}, which the server annotates as a writer; mcpeval never calls it"
                ),
                ToolClass::Unannotated if !confirm_read_only => bail!(
                    "--arguments names {name}, which carries no readOnlyHint; pass --confirm-read-only to attest it is read-only"
                ),
                _ => {}
            }
            if !crate::schema::conforms(&tool.input_schema, arguments) {
                bail!("--arguments for {name} do not satisfy its input schema");
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_digest_ignores_key_order_and_moves_with_any_value() {
        let a = ToolArguments::from_value(json!({"b": {"y": 1, "x": "v"}, "a": {}})).unwrap();
        let b = ToolArguments::from_value(json!({"a": {}, "b": {"x": "v", "y": 1}})).unwrap();
        let c = ToolArguments::from_value(json!({"a": {}, "b": {"x": "w", "y": 1}})).unwrap();
        assert_eq!(a.sha256(), b.sha256());
        assert_ne!(a.sha256(), c.sha256());
        assert_eq!(a.sha256().len(), 64);
    }

    #[test]
    fn shape_errors_never_quote_values() {
        for document in [
            json!(["secret-value"]),
            json!({"read": "secret-value"}),
            json!({"bad name": {"path": "secret-value"}}),
        ] {
            let error = ToolArguments::from_value(document).unwrap_err().to_string();
            assert!(!error.contains("secret-value"), "{error}");
        }
    }
}
