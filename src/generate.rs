use std::collections::BTreeMap;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::Path;

use anyhow::{bail, Context};
use rusqlite::{Connection, OpenFlags, OptionalExtension};
use serde_json::Value;

use crate::manifest::{Access, Manifest, ProbeCase};

pub fn run(
    root: &Path,
    finding_id: &str,
    output: &Path,
    force: bool,
    confirm_read_only: bool,
) -> anyhow::Result<String> {
    if !confirm_read_only {
        bail!("read-only generation requires explicit --confirm-read-only attestation");
    }

    let db = Connection::open_with_flags(root.join("index.db"), OpenFlags::SQLITE_OPEN_READ_ONLY)
        .context("opening index.db")?;
    let finding: Option<(Option<String>, Option<String>)> = db
        .query_row(
            "SELECT i.tool,i.args FROM findings f
             JOIN issues i ON i.id=f.issue_id
             WHERE f.finding_id=?1",
            [finding_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .context("looking up finding")?;
    let Some((tool, args)) = finding else {
        bail!("finding is unavailable; run `mcpeval promote` and use a current finding ID");
    };
    let Some(tool) = tool.filter(|tool| crate::privacy::valid_tool(tool)) else {
        bail!("finding has no valid tool");
    };
    let arguments: Value = args
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("finding arguments must be exactly {{}}"))
        .and_then(|args| {
            serde_json::from_str(args)
                .map_err(|_| anyhow::anyhow!("finding arguments must be exactly {{}}"))
        })?;
    if !matches!(&arguments, Value::Object(values) if values.is_empty()) {
        bail!("finding arguments must be exactly {{}}");
    }

    let manifest = Manifest {
        version: 1,
        sandboxes: BTreeMap::new(),
        probes: vec![ProbeCase::DegradationOverN {
            id: finding_id.to_owned(),
            tool,
            access: Access::ReadOnly,
            sandbox: None,
            arguments,
            max_attempts: 3,
        }],
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

    Ok(finding_id.to_owned())
}
