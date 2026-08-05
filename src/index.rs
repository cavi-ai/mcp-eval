use std::fs::DirEntry;
use std::path::{Path, PathBuf};

use anyhow::Context;
use rusqlite::{params, Connection};

use crate::record::CallRecord;

#[derive(Debug, Eq, PartialEq)]
pub struct Stats {
    pub calls: usize,
    pub failures: usize,
}

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS calls (
  id INTEGER PRIMARY KEY,
  ts TEXT NOT NULL, session TEXT NOT NULL, seq INTEGER NOT NULL,
  server TEXT NOT NULL, method TEXT NOT NULL, tool TEXT,
  latency_ms INTEGER, outcome TEXT NOT NULL,
  err_code TEXT, err_template TEXT, err_retryable INTEGER,
  args TEXT, kind TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS windows (
  failure_id INTEGER NOT NULL REFERENCES calls(id),
  neighbour_id INTEGER NOT NULL REFERENCES calls(id),
  offset INTEGER NOT NULL,
  PRIMARY KEY (failure_id, neighbour_id)
);
CREATE INDEX IF NOT EXISTS calls_issue ON calls (server, tool, err_template);
";

pub fn build(root: &Path) -> anyhow::Result<Stats> {
    let mut records = load_records(root)?;
    records.sort_by(|left, right| left.session.cmp(&right.session));

    let failures = records
        .iter()
        .filter(|record| record.outcome == "error")
        .count();
    let mut db = Connection::open(root.join("index.db")).context("opening index.db")?;
    db.pragma_update(None, "foreign_keys", "ON")?;
    let transaction = db.transaction().context("starting index rebuild")?;
    transaction.execute_batch(SCHEMA)?;
    transaction.execute("DELETE FROM windows", [])?;
    transaction.execute("DELETE FROM calls", [])?;

    let mut ids = Vec::with_capacity(records.len());
    for record in &records {
        let error = record.error.as_ref();
        let error_code = error
            .and_then(|value| value.code.as_ref())
            .map(serde_json::to_string)
            .transpose()?;
        let args = record
            .args
            .as_ref()
            .map(serde_json::to_string)
            .transpose()?;
        transaction.execute(
            "INSERT INTO calls
             (ts, session, seq, server, method, tool, latency_ms, outcome,
              err_code, err_template, err_retryable, args, kind)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13)",
            params![
                record.ts,
                record.session,
                record.seq,
                record.server,
                record.method,
                record.tool,
                record.latency_ms,
                record.outcome,
                error_code,
                error.and_then(|value| value.template.as_ref()),
                error.and_then(|value| value.retryable).map(i64::from),
                args,
                record.kind,
            ],
        )?;
        ids.push(transaction.last_insert_rowid());
    }

    for (failure_index, failure) in records.iter().enumerate() {
        if failure.outcome != "error" {
            continue;
        }
        for distance in 1..=5usize {
            let Some(neighbour_index) = failure_index.checked_sub(distance) else {
                break;
            };
            if records[neighbour_index].session != failure.session {
                break;
            }
            transaction.execute(
                "INSERT INTO windows (failure_id, neighbour_id, offset) VALUES (?1, ?2, ?3)",
                params![ids[failure_index], ids[neighbour_index], -(distance as i64)],
            )?;
        }
        for distance in 1..=3usize {
            let Some(neighbour) = records.get(failure_index + distance) else {
                break;
            };
            if neighbour.session != failure.session {
                break;
            }
            transaction.execute(
                "INSERT INTO windows (failure_id, neighbour_id, offset) VALUES (?1, ?2, ?3)",
                params![
                    ids[failure_index],
                    ids[failure_index + distance],
                    distance as i64
                ],
            )?;
        }
    }

    transaction.commit().context("committing index rebuild")?;
    Ok(Stats {
        calls: records.len(),
        failures,
    })
}

fn load_records(root: &Path) -> anyhow::Result<Vec<CallRecord>> {
    let store_dir = root.join("store");
    let mut paths = std::fs::read_dir(&store_dir)
        .with_context(|| format!("reading {}", store_dir.display()))?
        .filter_map(|entry| match entry {
            Ok(entry) => match is_call_file(&entry) {
                Ok(true) => Some(Ok(entry.path())),
                Ok(false) => None,
                Err(error) => Some(Err(error)),
            },
            Err(error) => Some(Err(error.into())),
        })
        .collect::<anyhow::Result<Vec<PathBuf>>>()?;
    paths.sort();

    let mut records = Vec::new();
    for path in paths {
        let body = std::fs::read(&path).with_context(|| format!("reading {}", path.display()))?;
        let mut line_start = 0;
        for (line_number, line_end) in body
            .iter()
            .enumerate()
            .filter_map(|(index, byte)| (*byte == b'\n').then_some(index))
            .enumerate()
        {
            let line_number = line_number + 1;
            let mut line = &body[line_start..line_end];
            if line.last() == Some(&b'\r') {
                line = &line[..line.len() - 1];
            }
            if !line.iter().all(u8::is_ascii_whitespace) {
                records.push(
                    serde_json::from_slice(line).with_context(|| {
                        format!("parsing {} line {line_number}", path.display())
                    })?,
                );
            }
            line_start = line_end + 1;
        }
    }
    Ok(records)
}

fn is_call_file(entry: &DirEntry) -> anyhow::Result<bool> {
    if !entry
        .file_type()
        .with_context(|| format!("reading file type for {}", entry.path().display()))?
        .is_file()
    {
        return Ok(false);
    }
    let name = entry.file_name();
    let Some(name) = name.to_str() else {
        return Ok(false);
    };
    Ok(name.starts_with("calls-") && name.ends_with(".jsonl"))
}
