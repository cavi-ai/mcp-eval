use anyhow::{bail, Context};
use chrono::{DateTime, SecondsFormat, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use sha2::{Digest, Sha256};
use std::path::Path;

pub const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS finding_lifecycle (
  finding_id TEXT PRIMARY KEY,
  server TEXT NOT NULL,
  tool TEXT,
  err_code TEXT,
  err_template_id TEXT,
  probe_id TEXT,
  state TEXT NOT NULL CHECK(state IN ('open','fix-claimed','verifying','closed')),
  consecutive_passes INTEGER NOT NULL DEFAULT 0,
  updated_at TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS probe_history (
  id INTEGER PRIMARY KEY,
  finding_id TEXT NOT NULL,
  probe_id TEXT NOT NULL,
  passed INTEGER NOT NULL CHECK(passed IN (0,1)),
  ts TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS probe_history_finding ON probe_history(finding_id, id);
";

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
    let mut hash = Sha256::new();
    for value in [Some(server), tool, err_code, err_template_id] {
        let bytes = value.unwrap_or("").as_bytes();
        hash.update((bytes.len() as u64).to_be_bytes());
        hash.update(bytes);
    }
    format!("finding-{}", &format!("{:x}", hash.finalize())[..16])
}

pub fn prepare(
    root: &Path,
    finding_id: &str,
    probe_id: &str,
    tool: &str,
) -> anyhow::Result<String> {
    let db = Connection::open(root.join("index.db"))?;
    let row: Option<(String, Option<String>)> = db
        .query_row(
            "SELECT l.server,l.tool FROM finding_lifecycle l
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
    passed: bool,
    now: DateTime<Utc>,
) -> anyhow::Result<Status> {
    let mut db = Connection::open(root.join("index.db"))?;
    let transaction = db.transaction()?;
    let current: Option<(String, Option<String>, u64)> = transaction
        .query_row(
            "SELECT state,probe_id,consecutive_passes FROM finding_lifecycle
             WHERE finding_id=?1",
            [finding_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()?;
    let Some((state, current_probe, consecutive)) = current else {
        bail!("finding is unavailable; run `mcpeval promote` first");
    };
    let state = State::parse(&state)?;
    let same_probe = current_probe.as_deref() == Some(probe_id);
    let (state, consecutive_passes) = transition(state, same_probe, consecutive, passed);
    let timestamp = now.to_rfc3339_opts(SecondsFormat::Millis, true);
    transaction.execute(
        "UPDATE finding_lifecycle SET probe_id=?2,state=?3,consecutive_passes=?4,updated_at=?5
         WHERE finding_id=?1",
        params![
            finding_id,
            probe_id,
            state.as_str(),
            consecutive_passes,
            timestamp
        ],
    )?;
    transaction.execute(
        "INSERT INTO probe_history(finding_id,probe_id,passed,ts) VALUES (?1,?2,?3,?4)",
        params![finding_id, probe_id, passed, timestamp],
    )?;
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
        } else if same_probe && matches!(state, State::Verifying | State::Closed) {
            (State::Open, 0)
        } else {
            (State::FixClaimed, 0)
        };
    }
    let consecutive = if same_probe { consecutive + 1 } else { 1 };
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
