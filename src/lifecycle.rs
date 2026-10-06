use crate::manifest::{Manifest, ProbeCase};
use anyhow::{bail, Context};
use chrono::{DateTime, SecondsFormat, Utc};
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::Path;
use std::time::Duration;

pub const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS finding_lifecycle (
  finding_id TEXT PRIMARY KEY,
  server TEXT NOT NULL,
  tool TEXT,
  err_code TEXT,
  err_template_id TEXT,
  probe_id TEXT,
  definition_id TEXT,
  state TEXT NOT NULL CHECK(state IN ('open','fix-claimed','verifying','closed')),
  consecutive_passes INTEGER NOT NULL DEFAULT 0,
  updated_at TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS probe_history (
  id INTEGER PRIMARY KEY,
  finding_id TEXT NOT NULL,
  probe_id TEXT NOT NULL,
  passed INTEGER NOT NULL CHECK(passed IN (0,1)),
  definition_id TEXT,
  run_id TEXT,
  ts TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS probe_history_finding ON probe_history(finding_id, id);
CREATE UNIQUE INDEX IF NOT EXISTS probe_history_run ON probe_history(run_id);
";

/// Includes only the selected case and the context that can affect its verdict.
/// Unrelated manifest edits and JSON formatting do not invalidate evidence.
pub fn definition_id(
    root: &Path,
    manifest: &Manifest,
    case: &ProbeCase,
    target: Value,
) -> anyhow::Result<String> {
    let mut executable =
        std::fs::File::open(std::env::current_exe()?).context("opening evaluator executable")?;
    let mut evaluator = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let count = executable.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        evaluator.update(&buffer[..count]);
    }
    let evaluator_sha256: String = evaluator
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    let sandbox = case.sandbox().and_then(|name| manifest.sandboxes.get(name));
    let definition = serde_json::json!({
        "contract": "mcpeval-verification/1", "evaluator": env!("CARGO_PKG_VERSION"),
        "evaluator_sha256": evaluator_sha256,
        "manifest_version": manifest.version, "timeout_ms": manifest.timeout_ms,
        "case": case, "sandbox": sandbox, "target": target,
    });
    Ok(crate::fingerprint::Salt::load(root)?
        .definition_id(&serde_json::to_vec(&canonical(definition))?))
}

fn canonical(value: Value) -> Value {
    match value {
        Value::Object(fields) => Value::Object(
            fields
                .into_iter()
                .collect::<std::collections::BTreeMap<_, _>>()
                .into_iter()
                .map(|(key, value)| (key, canonical(value)))
                .collect(),
        ),
        Value::Array(values) => Value::Array(values.into_iter().map(canonical).collect()),
        value => value,
    }
}

/// Attach durable evidence and import pre-upgrade index evidence exactly once.
/// All subsequent lifecycle writes use the attached database, never the cache.
pub(crate) fn attach(db: &mut Connection, root: &Path) -> anyhow::Result<()> {
    let path = root.join("lifecycle.db");
    let mut evidence = Connection::open(&path).context("opening lifecycle.db")?;
    evidence.busy_timeout(Duration::from_secs(5))?;
    let init = evidence.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let version: u32 = init.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if version > 1 {
        bail!("lifecycle database schema is newer than this evaluator supports");
    }
    init.execute_batch(SCHEMA)?;
    init.execute_batch(
        "CREATE TABLE IF NOT EXISTS evidence_metadata (key TEXT PRIMARY KEY, value TEXT NOT NULL)",
    )?;
    init.pragma_update(None, "user_version", 1)?;
    init.commit()?;
    drop(evidence);
    db.busy_timeout(Duration::from_secs(5))?;
    db.execute(
        "ATTACH DATABASE ?1 AS evidence",
        [path.to_str().context("invalid lifecycle database path")?],
    )?;
    db.execute_batch("PRAGMA evidence.synchronous=FULL")?;
    let migration = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let imported: bool = migration.query_row(
        "SELECT EXISTS(SELECT 1 FROM evidence.evidence_metadata WHERE key='legacy-imported')",
        [],
        |row| row.get(0),
    )?;
    if !imported {
        let present: bool = migration.query_row("SELECT EXISTS(SELECT 1 FROM main.sqlite_master WHERE type='table' AND name='finding_lifecycle')", [], |row| row.get(0))?;
        if present {
            migration.execute_batch("INSERT OR IGNORE INTO evidence.finding_lifecycle
                (finding_id,server,tool,err_code,err_template_id,probe_id,state,consecutive_passes,updated_at)
                SELECT finding_id,server,tool,err_code,err_template_id,probe_id,state,consecutive_passes,updated_at FROM main.finding_lifecycle")?;
        }
        let present: bool = migration.query_row("SELECT EXISTS(SELECT 1 FROM main.sqlite_master WHERE type='table' AND name='probe_history')", [], |row| row.get(0))?;
        if present {
            migration.execute_batch(
                "INSERT INTO evidence.probe_history (finding_id,probe_id,passed,ts)
                SELECT finding_id,probe_id,passed,ts FROM main.probe_history ORDER BY id",
            )?;
        }
        migration.execute(
            "INSERT INTO evidence.evidence_metadata VALUES ('legacy-imported','1')",
            [],
        )?;
    }
    migration.commit()?;
    Ok(())
}

/// Rebuild compatibility query tables from authoritative lifecycle evidence.
pub(crate) fn project(db: &Connection) -> anyhow::Result<()> {
    db.execute_batch(
        "DROP TABLE IF EXISTS main.probe_history; DROP TABLE IF EXISTS main.finding_lifecycle;",
    )?;
    db.execute_batch(SCHEMA)?;
    db.execute_batch(
        "INSERT INTO main.finding_lifecycle SELECT * FROM evidence.finding_lifecycle;
        INSERT INTO main.probe_history SELECT * FROM evidence.probe_history;",
    )?;
    Ok(())
}

fn project_result(db: &Connection, finding: &str, run: &str) -> anyhow::Result<()> {
    let columns: Vec<String> = db
        .prepare("PRAGMA main.table_info(probe_history)")?
        .query_map([], |row| row.get(1))?
        .collect::<Result<_, _>>()?;
    if !columns.iter().any(|column| column == "run_id") {
        return project(db);
    }
    db.execute(
        "INSERT INTO main.finding_lifecycle
        SELECT * FROM evidence.finding_lifecycle WHERE finding_id=?1
        ON CONFLICT(finding_id) DO UPDATE SET probe_id=excluded.probe_id,
        definition_id=excluded.definition_id,state=excluded.state,
        consecutive_passes=excluded.consecutive_passes,updated_at=excluded.updated_at",
        [finding],
    )?;
    db.execute(
        "INSERT OR IGNORE INTO main.probe_history
        SELECT * FROM evidence.probe_history WHERE run_id=?1",
        [run],
    )?;
    Ok(())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum State {
    Open,
    FixClaimed,
    Verifying,
    Closed,
}

impl State {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::FixClaimed => "fix-claimed",
            Self::Verifying => "verifying",
            Self::Closed => "closed",
        }
    }

    fn parse(value: &str) -> anyhow::Result<Self> {
        match value {
            "open" => Ok(Self::Open),
            "fix-claimed" => Ok(Self::FixClaimed),
            "verifying" => Ok(Self::Verifying),
            "closed" => Ok(Self::Closed),
            _ => bail!("invalid finding lifecycle state"),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Status {
    pub state: State,
    pub probe_id: Option<String>,
    pub consecutive_passes: u64,
}

pub fn finding_id(
    server: &str,
    tool: Option<&str>,
    err_code: Option<&str>,
    err_template_id: Option<&str>,
) -> String {
    hash_finding_id(&[], &[Some(server), tool, err_code, err_template_id])
}

/// An observed semantic failure is distinct from a structured error, even
/// when both have no error template. Existing error finding IDs stay unchanged.
pub(crate) fn false_success_id(server: &str, tool: Option<&str>) -> String {
    hash_finding_id(
        b"mcpeval/annotated-success/false-success/v1",
        &[Some(server), tool],
    )
}

fn hash_finding_id(domain: &[u8], values: &[Option<&str>]) -> String {
    let mut hash = Sha256::new();
    hash.update(domain);
    for value in values {
        let bytes = value.unwrap_or("").as_bytes();
        hash.update((bytes.len() as u64).to_be_bytes());
        hash.update(bytes);
    }
    let finalized = hash.finalize();
    let hex: String = finalized.iter().map(|byte| format!("{byte:02x}")).collect();
    format!("finding-{}", &hex[..16])
}

pub fn prepare(
    root: &Path,
    finding_id: &str,
    probe_id: &str,
    tool: &str,
) -> anyhow::Result<String> {
    let mut db = Connection::open(root.join("index.db"))?;
    attach(&mut db, root)?;
    let row: Option<(String, Option<String>)> = db
        .query_row(
            "SELECT l.server,l.tool FROM evidence.finding_lifecycle l
             JOIN findings f ON f.finding_id=l.finding_id
             WHERE l.finding_id=?1",
            [finding_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .context("looking up finding")?;
    let Some((server, finding_tool)) = row else {
        bail!("finding is unavailable; run `mcpeval promote` and use a current finding ID");
    };
    if finding_tool.as_deref() != Some(tool) {
        bail!("probe case tool does not match the finding tool");
    }
    if !crate::privacy::valid_identifier(probe_id) {
        bail!("probe id is invalid");
    }
    Ok(server)
}

pub fn record(
    root: &Path,
    finding_id: &str,
    probe_id: &str,
    definition_id: &str,
    run_id: &str,
    passed: bool,
    now: DateTime<Utc>,
) -> anyhow::Result<Status> {
    let mut db = Connection::open(root.join("index.db"))?;
    if definition_id.len() != 64
        || !definition_id.bytes().all(|byte| byte.is_ascii_hexdigit())
        || uuid::Uuid::parse_str(run_id)
            .map(|id| id.to_string() != run_id)
            .unwrap_or(true)
    {
        bail!("invalid verification evidence identity");
    }
    attach(&mut db, root)?;
    let transaction = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let current: Option<(String, Option<String>, i64, Option<String>)> = transaction
        .query_row(
            "SELECT state,probe_id,consecutive_passes,definition_id FROM evidence.finding_lifecycle
             WHERE finding_id=?1",
            [finding_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .optional()?;
    let Some((state, current_probe, consecutive, current_definition)) = current else {
        bail!("finding is unavailable; run `mcpeval promote` first");
    };
    let state = State::parse(&state)?;
    let prior: Option<(String, String, String, bool)> = transaction.query_row(
        "SELECT finding_id,probe_id,definition_id,passed FROM evidence.probe_history WHERE run_id=?1",
        [run_id], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?))
    ).optional()?;
    if let Some(prior) = prior {
        if prior
            != (
                finding_id.into(),
                probe_id.into(),
                definition_id.into(),
                passed,
            )
        {
            bail!("verification run ID conflicts with recorded evidence");
        }
        return Ok(Status {
            state,
            probe_id: current_probe,
            consecutive_passes: consecutive.max(0) as u64,
        });
    }
    let same_probe = current_probe.as_deref() == Some(probe_id)
        && current_definition.as_deref() == Some(definition_id);
    let (state, consecutive_passes) =
        transition(state, same_probe, consecutive.max(0) as u64, passed);
    let timestamp = now.to_rfc3339_opts(SecondsFormat::Millis, true);
    transaction.execute(
        "UPDATE evidence.finding_lifecycle SET probe_id=?2,state=?3,consecutive_passes=?4,updated_at=?5,definition_id=?6
         WHERE finding_id=?1",
        params![
            finding_id,
            probe_id,
            state.as_str(),
            consecutive_passes as i64,
            timestamp,
            definition_id,
        ],
    )?;
    transaction.execute(
        "INSERT INTO evidence.probe_history(finding_id,probe_id,passed,ts,definition_id,run_id) VALUES (?1,?2,?3,?4,?5,?6)",
        params![finding_id, probe_id, passed, timestamp, definition_id, run_id],
    )?;
    project_result(&transaction, finding_id, run_id)?;
    transaction.commit()?;
    Ok(Status {
        state,
        probe_id: Some(probe_id.to_owned()),
        consecutive_passes,
    })
}

fn transition(state: State, same_probe: bool, consecutive: u64, passed: bool) -> (State, u64) {
    if !passed {
        return if same_probe && state == State::FixClaimed {
            (State::FixClaimed, 0)
        } else if matches!(state, State::Verifying | State::Closed) {
            (State::Open, 0)
        } else {
            (State::FixClaimed, 0)
        };
    }
    let consecutive = if same_probe {
        consecutive.saturating_add(1).min(i64::MAX as u64)
    } else {
        1
    };
    if consecutive >= 3 {
        (State::Closed, consecutive)
    } else {
        (State::Verifying, consecutive)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transition_requires_three_consecutive_greens_and_reopens_on_red() {
        assert_eq!(
            transition(State::Open, false, 0, false),
            (State::FixClaimed, 0)
        );
        assert_eq!(
            transition(State::FixClaimed, true, 0, true),
            (State::Verifying, 1)
        );
        assert_eq!(
            transition(State::Verifying, true, 1, true),
            (State::Verifying, 2)
        );
        assert_eq!(
            transition(State::Verifying, true, 2, true),
            (State::Closed, 3)
        );
        assert_eq!(transition(State::Closed, true, 3, false), (State::Open, 0));
        assert_eq!(
            transition(State::Closed, false, 3, true),
            (State::Verifying, 1)
        );
    }
}
