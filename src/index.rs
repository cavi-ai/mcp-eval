use std::fs::DirEntry;
use std::path::{Path, PathBuf};

use anyhow::Context;
use rusqlite::{params, Connection, OptionalExtension};

use crate::record::{AnnotationRecord, CallRecord};

#[derive(Debug, Eq, PartialEq)]
pub struct Stats {
    pub calls: usize,
    pub failures: usize,
    pub annotations: usize,
    pub replayed_events: usize,
}

/// Dropped ahead of `SCHEMA` on every rebuild, oldest-dependent-first: a
/// Phase 1 `index.db` predates `err_template_id` and `annotations` entirely,
/// so `CREATE TABLE IF NOT EXISTS` is a no-op against it and the INSERT
/// below would fail on an unrecognized column. The index is pure derived
/// state — nothing here is ever read before this function rebuilds it — so
/// dropping and recreating is safe. `windows` and `annotations` precede `calls`
/// because they hold `REFERENCES calls(id)` foreign keys and `foreign_keys`
/// is on for this connection.
const DROP_SCHEMA: &str = "
DROP TABLE IF EXISTS findings;
DROP TABLE IF EXISTS issues;
DROP INDEX IF EXISTS calls_issue;
DROP INDEX IF EXISTS annotations_call;
DROP TABLE IF EXISTS annotations;
DROP TABLE IF EXISTS windows;
DROP TABLE IF EXISTS calls;
";

const SCHEMA: &str = "
CREATE TABLE calls (
  id INTEGER PRIMARY KEY,
  capture_id TEXT, event_id TEXT UNIQUE,
  ts TEXT NOT NULL, session TEXT NOT NULL, seq INTEGER NOT NULL,
  server TEXT NOT NULL, method TEXT NOT NULL, tool TEXT,
  latency_ms INTEGER, outcome TEXT NOT NULL,
  err_code TEXT, err_template TEXT, err_template_id TEXT, err_retryable INTEGER,
  args TEXT, kind TEXT NOT NULL
);
CREATE TABLE windows (
  failure_id INTEGER NOT NULL REFERENCES calls(id),
  neighbour_id INTEGER NOT NULL REFERENCES calls(id),
  offset INTEGER NOT NULL,
  PRIMARY KEY (failure_id, neighbour_id)
);
CREATE INDEX calls_issue ON calls (server, tool, err_code, err_template_id);
CREATE TABLE annotations (
  call_id INTEGER REFERENCES calls(id), event_id TEXT,
  session TEXT, seq INTEGER, ts TEXT NOT NULL,
  kind TEXT NOT NULL, note TEXT NOT NULL
);
CREATE INDEX annotations_call ON annotations (call_id);
CREATE UNIQUE INDEX calls_capture_sequence ON calls (capture_id, seq) WHERE capture_id IS NOT NULL;
";

/// Stream records into an atomic disk-backed rebuild. Identity checks and
/// correlation ordering use SQLite rather than retaining the whole journal.
pub fn build(root: &Path) -> anyhow::Result<Stats> {
    let mut db = Connection::open(root.join("index.db")).context("opening index.db")?;
    db.busy_timeout(std::time::Duration::from_secs(5))?;
    db.pragma_update(None, "foreign_keys", "ON")?;
    db.pragma_update(None, "temp_store", "FILE")?;
    let transaction = db.transaction().context("starting index rebuild")?;
    transaction.execute_batch(DROP_SCHEMA)?;
    transaction.execute_batch(SCHEMA)?;
    transaction.execute_batch(
        "CREATE TEMP TABLE seen_events(event TEXT PRIMARY KEY, record TEXT NOT NULL);
        CREATE TEMP TABLE seen_positions(capture TEXT, seq INTEGER, PRIMARY KEY(capture,seq));
        CREATE TEMP TABLE seen_captures(capture TEXT PRIMARY KEY, session TEXT, server TEXT);",
    )?;
    let mut stats = Stats {
        calls: 0,
        failures: 0,
        annotations: 0,
        replayed_events: 0,
    };
    visit_jsonl::<CallRecord>(root, "calls-", |record| {
        let record = record.sanitized();
        if let Some(identity) = &record.identity {
            identity.validate()?;
            let canonical = serde_json::to_string(&record)?;
            let previous: Option<String> = transaction
                .query_row(
                    "SELECT record FROM seen_events WHERE event=?1",
                    [identity.event_id.to_string()],
                    |row| row.get(0),
                )
                .optional()?;
            if let Some(previous) = previous {
                anyhow::ensure!(
                    previous == canonical,
                    "conflicting records for one event ID"
                );
                stats.replayed_events += 1;
                return Ok(());
            }
            let capture = identity.capture_id.to_string();
            let prior: Option<(String, String)> = transaction
                .query_row(
                    "SELECT session,server FROM seen_captures WHERE capture=?1",
                    [&capture],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()?;
            if let Some((session, server)) = prior {
                anyhow::ensure!(
                    session == record.session && server == record.server,
                    "capture ID crosses session or server boundaries"
                );
            } else {
                transaction.execute(
                    "INSERT INTO seen_captures VALUES (?1,?2,?3)",
                    params![capture, record.session, record.server],
                )?;
            }
            transaction
                .execute(
                    "INSERT INTO seen_positions VALUES (?1,?2)",
                    params![capture, record.seq as i64],
                )
                .context("multiple event IDs claim one capture sequence")?;
            transaction.execute(
                "INSERT INTO seen_events VALUES (?1,?2)",
                params![identity.event_id.to_string(), canonical],
            )?;
        }
        let error = record.error.as_ref();
        transaction.execute("INSERT INTO calls
            (ts,session,seq,server,method,tool,latency_ms,outcome,err_code,err_template,err_template_id,err_retryable,args,kind,capture_id,event_id)
            VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16)", params![
            record.ts, record.session, record.seq as i64, record.server, record.method, record.tool,
            record.latency_ms.map(|value|value as i64), record.outcome,
            error.and_then(|value|value.code.as_ref()).map(serde_json::to_string).transpose()?,
            error.and_then(|value|value.template.as_ref()), error.and_then(|value|value.template_id.as_ref()),
            error.and_then(|value|value.retryable).map(i64::from), record.args.as_ref().map(serde_json::to_string).transpose()?,
            record.kind, record.identity.as_ref().map(|i|i.capture_id.to_string()),record.identity.as_ref().map(|i|i.event_id.to_string())])?;
        stats.calls += 1;
        stats.failures += usize::from(record.outcome == "error");
        Ok(())
    })?;
    transaction.execute_batch("CREATE INDEX calls_event ON calls(event_id);
        CREATE INDEX calls_coordinates ON calls(session,seq);
        CREATE TEMP TABLE ranked AS SELECT id,outcome,capture_id,session,server,
            ROW_NUMBER() OVER (PARTITION BY capture_id,CASE WHEN capture_id IS NULL THEN session END,CASE WHEN capture_id IS NULL THEN server END ORDER BY seq,id) AS position FROM calls;
        CREATE INDEX ranked_context ON ranked(capture_id,session,server,position);
        CREATE INDEX ranked_capture ON ranked(capture_id,position);
        CREATE TEMP TABLE ambiguous AS SELECT session,server FROM calls WHERE capture_id IS NULL GROUP BY session,server,seq HAVING COUNT(*) > 1;
        CREATE INDEX ambiguous_context ON ambiguous(session,server);
        INSERT INTO windows(failure_id,neighbour_id,offset)
        SELECT f.id,n.id,n.position-f.position FROM ranked f JOIN ranked n
        ON n.capture_id=f.capture_id AND n.position BETWEEN f.position-5 AND f.position+3 AND n.position != f.position
        WHERE f.outcome='error' AND f.capture_id IS NOT NULL
        UNION ALL
        SELECT f.id,n.id,n.position-f.position FROM ranked f JOIN ranked n
        ON n.capture_id IS NULL AND n.session=f.session AND n.server=f.server
        AND n.position BETWEEN f.position-5 AND f.position+3 AND n.position != f.position
        WHERE f.outcome='error' AND f.capture_id IS NULL AND NOT EXISTS
        (SELECT 1 FROM ambiguous a WHERE a.session=f.session AND a.server=f.server);")?;
    visit_jsonl::<AnnotationRecord>(root, "annotations-", |annotation| {
        let annotation = annotation.sanitized();
        annotation.validate_target()?;
        let target: Option<i64> = if let Some(event) = annotation.event_id {
            transaction
                .query_row(
                    "SELECT id FROM calls WHERE event_id=?1",
                    [event.to_string()],
                    |row| row.get(0),
                )
                .optional()?
        } else {
            transaction.query_row("SELECT CASE WHEN COUNT(*)=1 THEN MIN(id) END FROM calls WHERE session=?1 AND seq=?2", params![annotation.session,annotation.seq.map(|seq|seq as i64)], |row|row.get(0))?
        };
        transaction.execute("INSERT INTO annotations(session,seq,ts,kind,note,event_id,call_id) VALUES (?1,?2,?3,?4,?5,?6,?7)", params![annotation.session,annotation.seq.map(|seq|seq as i64),annotation.ts,annotation.kind,annotation.note,annotation.event_id.map(|id|id.to_string()),target])?;
        stats.annotations += 1;
        Ok(())
    })?;
    transaction.commit().context("committing index rebuild")?;
    Ok(stats)
}

fn visit_jsonl<T: serde::de::DeserializeOwned>(
    root: &Path,
    prefix: &str,
    mut accept: impl FnMut(T) -> anyhow::Result<()>,
) -> anyhow::Result<()> {
    let store_dir = root.join("store");
    let mut paths = std::fs::read_dir(&store_dir)?
        .filter_map(|entry| match entry {
            Ok(entry) => match is_prefixed_jsonl_file(&entry, prefix) {
                Ok(true) => Some(Ok(entry.path())),
                Ok(false) => None,
                Err(error) => Some(Err(error)),
            },
            Err(error) => Some(Err(error.into())),
        })
        .collect::<anyhow::Result<Vec<PathBuf>>>()?;
    paths.sort();
    for path in paths {
        let mut reader = std::io::BufReader::new(std::fs::File::open(&path)?);
        crate::jsonl::visit(&mut reader, &mut accept)
            .with_context(|| format!("reading {}", path.display()))?;
    }
    Ok(())
}

fn is_prefixed_jsonl_file(entry: &DirEntry, prefix: &str) -> anyhow::Result<bool> {
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
    Ok(name.starts_with(prefix) && name.ends_with(".jsonl"))
}
