use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::Path;

use anyhow::{bail, Context};
use rusqlite::{Connection, OpenFlags, OptionalExtension};
use serde_json::{Map, Value};

use crate::manifest::{Access, Manifest, ProbeCase};

const NIL_UUID: &str = "00000000-0000-0000-0000-000000000000";
const UNKNOWN_SHAPE: &str = "finding arguments are not a recorded argument shape";

/// A written manifest: the probe chosen from the finding's evidence and the
/// argument placeholders the operator must fill before `mcpeval verify`.
#[derive(Debug)]
pub struct Generated {
    pub probe_id: String,
    pub probe: ProbeCase,
    /// Key path and recorded shape of each placeholder, in key-path order.
    pub placeholders: Vec<(String, String)>,
}

impl Generated {
    pub fn summary(&self, output: &Path) -> String {
        let mut text = format!("{}\nprobe={}", self.probe_id, self.probe.kind().as_str());
        if let Some(attempts) = self.probe.max_attempts() {
            write!(text, " max_attempts={attempts}").expect("writing to a string");
        }
        text.push('\n');
        for (path, shape) in &self.placeholders {
            writeln!(text, "fill: {path} ({shape})").expect("writing to a string");
        }
        if !self.placeholders.is_empty() {
            writeln!(
                text,
                "fill the placeholders in {} before mcpeval verify",
                output.display()
            )
            .expect("writing to a string");
        }
        text
    }
}

pub fn run(
    root: &Path,
    finding_id: &str,
    output: &Path,
    force: bool,
    confirm_read_only: bool,
) -> anyhow::Result<Generated> {
    if !confirm_read_only {
        bail!("read-only generation requires explicit --confirm-read-only attestation");
    }

    let db = Connection::open_with_flags(root.join("index.db"), OpenFlags::SQLITE_OPEN_READ_ONLY)
        .context("opening index.db")?;
    type Row = (Option<String>, Option<String>, f64);
    let finding: Option<Row> = db
        .query_row(
            "SELECT i.tool,i.args,i.rate FROM findings f
             JOIN issues i ON i.id=f.issue_id
             WHERE f.finding_id=?1",
            [finding_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()
        .context("looking up finding")?;
    let Some((tool, args, rate)) = finding else {
        bail!("finding is unavailable; run `mcpeval promote` and use a current finding ID");
    };
    let Some(tool) = tool.filter(|tool| crate::privacy::valid_tool(tool)) else {
        bail!("finding has no valid tool");
    };
    let shape: Value = match args.as_deref() {
        Some(args) => serde_json::from_str(args).map_err(|_| anyhow::anyhow!(UNKNOWN_SHAPE))?,
        None => Value::Object(Map::new()),
    };
    let mut placeholders = Vec::new();
    let arguments = skeleton(&shape, "", &mut placeholders)?;

    let probe = ProbeCase::DegradationOverN {
        id: finding_id.to_owned(),
        tool,
        access: Access::ReadOnly,
        sandbox: None,
        arguments,
        max_attempts: attempts_to_observe(rate),
    };
    let manifest = Manifest {
        version: 1,
        timeout_ms: None,
        sandboxes: BTreeMap::new(),
        probes: vec![probe],
    };
    manifest.validate()?;
    let mut body = serde_json::to_string_pretty(&manifest)?;
    body.push('\n');

    let mut file = if force {
        OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(output)
    } else {
        OpenOptions::new().create_new(true).write(true).open(output)
    }
    .context("writing generated manifest")?;
    file.write_all(body.as_bytes())
        .context("writing generated manifest")?;

    Ok(Generated {
        probe_id: finding_id.to_owned(),
        probe: manifest
            .probes
            .into_iter()
            .next()
            .expect("one generated probe"),
        placeholders,
    })
}

/// Attempts that observe a failure occurring at `rate` with 95% probability,
/// clamped to 3..=100.
fn attempts_to_observe(rate: f64) -> u64 {
    if rate <= 0.0 {
        return 3;
    }
    ((0.05_f64.ln() / (1.0 - rate).ln()).ceil() as u64).clamp(3, 100)
}

/// Builds call arguments from a shape written by `crate::shape`: recorded
/// enum members, numbers, booleans, and nulls are kept; strings, UUIDs, and
/// non-empty arrays become placeholders listed in `placeholders`.
fn skeleton(
    shape: &Value,
    path: &str,
    placeholders: &mut Vec<(String, String)>,
) -> anyhow::Result<Value> {
    match shape {
        Value::Object(fields) if is_array_shape(fields) => {
            if fields["array"] != 0 {
                placeholders.push((path.to_owned(), shape.to_string()));
            }
            Ok(Value::Array(Vec::new()))
        }
        Value::Object(fields) => fields
            .iter()
            .map(|(key, value)| {
                let child = if path.is_empty() {
                    key.clone()
                } else {
                    format!("{path}.{key}")
                };
                Ok((key.clone(), skeleton(value, &child, placeholders)?))
            })
            .collect::<anyhow::Result<Map<_, _>>>()
            .map(Value::Object),
        Value::String(marker) => leaf(marker, path, placeholders),
        _ => bail!(UNKNOWN_SHAPE),
    }
}

fn is_array_shape(fields: &Map<String, Value>) -> bool {
    fields.len() == 2
        && fields.get("array").is_some_and(Value::is_u64)
        && fields.contains_key("items")
}

fn leaf(
    marker: &str,
    path: &str,
    placeholders: &mut Vec<(String, String)>,
) -> anyhow::Result<Value> {
    if marker == "null" {
        return Ok(Value::Null);
    }
    if let Some(member) = marker.strip_prefix("enum:") {
        return Ok(Value::String(member.to_owned()));
    }
    if let Some(flag) = marker.strip_prefix("bool:") {
        return flag
            .parse()
            .map(Value::Bool)
            .map_err(|_| anyhow::anyhow!(UNKNOWN_SHAPE));
    }
    if let Some(number) = marker.strip_prefix("num:") {
        return serde_json::from_str(number)
            .map(Value::Number)
            .map_err(|_| anyhow::anyhow!(UNKNOWN_SHAPE));
    }
    let placeholder = if marker == "uuid" {
        NIL_UUID.to_owned()
    } else if ["str<", "str>", "url:"]
        .iter()
        .any(|prefix| marker.starts_with(prefix))
    {
        String::new()
    } else {
        bail!(UNKNOWN_SHAPE);
    };
    placeholders.push((path.to_owned(), marker.to_owned()));
    Ok(Value::String(placeholder))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn attempts_observe_the_failure_rate_with_95_percent_probability() {
        let cases = [
            (0.0, 3),
            (0.17, 17),
            (1.0 / 3.0, 8),
            (0.5, 5),
            (0.89, 3),
            (0.001, 100),
        ];
        for (rate, attempts) in cases {
            assert_eq!(attempts_to_observe(rate), attempts, "rate {rate}");
        }
    }

    #[test]
    fn skeleton_keeps_recorded_values_and_lists_placeholders_in_key_path_order() {
        let shape = json!({
            "city": "str<8",
            "n": "num:3",
            "on": "bool:true",
            "gone": "null",
            "kind": "enum:fast",
            "id": "uuid",
            "site": "url:example.com",
            "tags": {"array": 1, "items": "str<32"},
            "none": {"array": 0, "items": "empty"},
            "nested": {"array": "num:1", "items": "str>4096"}
        });
        let mut placeholders = Vec::new();

        let arguments = skeleton(&shape, "", &mut placeholders).unwrap();

        assert_eq!(
            arguments,
            json!({
                "city": "", "n": 3, "on": true, "gone": null, "kind": "fast",
                "id": NIL_UUID, "site": "", "tags": [], "none": [],
                "nested": {"array": 1, "items": ""}
            })
        );
        assert_eq!(
            placeholders,
            [
                ("city", "str<8"),
                ("id", "uuid"),
                ("nested.items", "str>4096"),
                ("site", "url:example.com"),
                ("tags", r#"{"array":1,"items":"str<32"}"#),
            ]
            .map(|(path, shape)| (path.to_owned(), shape.to_owned()))
        );
    }

    #[test]
    fn skeleton_rejects_values_that_are_not_shapes_without_echoing_them() {
        for shape in [json!({"target": "raw-value"}), json!({"n": 3})] {
            let error = skeleton(&shape, "", &mut Vec::new())
                .unwrap_err()
                .to_string();
            assert_eq!(error, UNKNOWN_SHAPE);
        }
    }
}
