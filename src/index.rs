use std::fs::DirEntry;
use std::path::Path;

mod journal;

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

/// Dropped ahead of `SCHEMA` on full rebuilds, oldest-dependent-first: a
/// Phase 1 `index.db` predates `err_template_id` and `annotations` entirely,
/// so `CREATE TABLE IF NOT EXISTS` is a no-op against it and the INSERT
/// below would fail on an unrecognized column. The index is pure derived
/// state, so reconstruction may safely replace it. `windows` and `annotations` precede `calls`
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
CREATE INDEX calls_coordinates ON calls(session,seq);
CREATE TABLE annotations (
  call_id INTEGER REFERENCES calls(id), event_id TEXT,
  session TEXT, seq INTEGER, ts TEXT NOT NULL,
  kind TEXT NOT NULL, note TEXT NOT NULL
);
CREATE INDEX annotations_call ON annotations (call_id);
CREATE UNIQUE INDEX calls_capture_sequence ON calls (capture_id, seq) WHERE capture_id IS NOT NULL;
";

/// Refresh a derived index using validated journal checkpoints. Historical
/// bytes are still hashed; only append-safe suffixes are parsed and inserted.
pub fn build(root: &Path) -> anyhow::Result<Stats> {
    build_mode(root, false)
}

/// Reconstruct all derived rows from the journals, ignoring checkpoints.
pub fn rebuild(root: &Path) -> anyhow::Result<Stats> {
    build_mode(root, true)
}

fn build_mode(root: &Path, force: bool) -> anyhow::Result<Stats> {
    let mut db = Connection::open(root.join("index.db")).context("opening index.db")?;
    db.busy_timeout(std::time::Duration::from_secs(5))?;
    db.pragma_update(None, "foreign_keys", "ON")?;
    db.pragma_update(None, "temp_store", "FILE")?;
    // Serialize checkpoint readers before they select a cached generation.
    let transaction = db
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .context("starting index refresh")?;
    let integrity: String = transaction.query_row("PRAGMA quick_check", [], |row| row.get(0))?;
    anyhow::ensure!(integrity == "ok", "index database is corrupt");
    let mut plan = journal::Plan::prepare(&transaction, root, force)?;
    if !plan.incremental {
        transaction.execute_batch(DROP_SCHEMA)?;
        transaction.execute_batch(journal::DROP_SCHEMA)?;
        transaction.execute_batch(SCHEMA)?;
        transaction.execute_batch(journal::SCHEMA)?;
    } else {
        // Promotion is derived from the refreshed index, as on full rebuilds.
        transaction.execute_batch("DROP TABLE IF EXISTS findings; DROP TABLE IF EXISTS issues;")?;
    }
    let mut replayed_events = plan.replayed_events;
    let calls_changed = plan.visit::<CallRecord>(&transaction, "calls-", |record| {
        let record = record.sanitized();
        if let Some(identity) = &record.identity {
            identity.validate()?;
            let canonical = journal::record_digest(&record)?;
            let previous: Option<String> = transaction
                .prepare_cached("SELECT digest FROM index_events WHERE event=?1")?
                .query_row([identity.event_id.to_string()], |row| row.get(0))
                .optional()?;
            if let Some(previous) = previous {
                anyhow::ensure!(
                    previous == canonical,
                    "conflicting records for one event ID"
                );
                replayed_events += 1;
                return Ok(());
            }
            let capture = identity.capture_id.to_string();
            let prior: Option<(String, String)> = transaction
                .prepare_cached("SELECT session,server FROM index_captures WHERE capture=?1")?
                .query_row([&capture], |row| Ok((row.get(0)?, row.get(1)?)))
                .optional()?;
            if let Some((session, server)) = prior {
                anyhow::ensure!(
                    session == record.session && server == record.server,
                    "capture ID crosses session or server boundaries"
                );
            } else {
                transaction
                    .prepare_cached("INSERT INTO index_captures VALUES (?1,?2,?3)")?
                    .execute(params![capture, record.session, record.server])?;
            }
            transaction
                .prepare_cached("INSERT INTO index_events VALUES (?1,?2)")?
                .execute(params![identity.event_id.to_string(), canonical])?;
        }
        let error = record.error.as_ref();
        transaction.prepare_cached("INSERT INTO calls
            (ts,session,seq,server,method,tool,latency_ms,outcome,err_code,err_template,err_template_id,err_retryable,args,kind,capture_id,event_id)
            VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16)")?.execute(params![
            record.ts, record.session, record.seq as i64, record.server, record.method, record.tool,
            record.latency_ms.map(|value|value as i64), record.outcome,
            error.and_then(|value|value.code.as_ref()).map(serde_json::to_string).transpose()?,
            error.and_then(|value|value.template.as_ref()), error.and_then(|value|value.template_id.as_ref()),
            error.and_then(|value|value.retryable).map(i64::from), record.args.as_ref().map(serde_json::to_string).transpose()?,
            record.kind, record.identity.as_ref().map(|i|i.capture_id.to_string()),record.identity.as_ref().map(|i|i.event_id.to_string())])?;
        Ok(())
    })?;
    // The unique event_id and capture-sequence indexes already enforce modern
    // identities and support event lookups; no second identity index is needed.
    if calls_changed || !plan.incremental {
        transaction.execute_batch("DELETE FROM windows;
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
    }
    let annotations_changed = plan.visit::<AnnotationRecord>(&transaction, "annotations-", |annotation| {
        let annotation = annotation.sanitized();
        annotation.validate_target()?;
        let target: Option<i64> = if let Some(event) = annotation.event_id {
            transaction
                .prepare_cached("SELECT id FROM calls WHERE event_id=?1")?
                .query_row([event.to_string()], |row| row.get(0))
                .optional()?
        } else {
            transaction.prepare_cached("SELECT CASE WHEN COUNT(*)=1 THEN MIN(id) END FROM calls WHERE session=?1 AND seq=?2")?.query_row(params![annotation.session,annotation.seq.map(|seq|seq as i64)], |row|row.get(0))?
        };
        transaction.prepare_cached("INSERT INTO annotations(session,seq,ts,kind,note,event_id,call_id) VALUES (?1,?2,?3,?4,?5,?6,?7)")?.execute(params![annotation.session,annotation.seq.map(|seq|seq as i64),annotation.ts,annotation.kind,annotation.note,annotation.event_id.map(|id|id.to_string()),target])?;
        Ok(())
    })?;
    // New calls can resolve formerly unknown events or make legacy targets
    // ambiguous. Re-link retained annotations as well as newly ingested ones.
    if calls_changed || annotations_changed {
        transaction.execute_batch("UPDATE annotations SET call_id=CASE WHEN event_id IS NOT NULL
            THEN (SELECT id FROM calls WHERE calls.event_id=annotations.event_id)
            ELSE (SELECT CASE WHEN COUNT(*)=1 THEN MIN(id) END FROM calls WHERE calls.session=annotations.session AND calls.seq=annotations.seq) END;")?;
    }
    let stats = Stats {
        calls: usize::try_from(transaction.query_row(
            "SELECT COUNT(*) FROM calls",
            [],
            |row| row.get::<_, i64>(0),
        )?)?,
        failures: usize::try_from(transaction.query_row(
            "SELECT COUNT(*) FROM calls WHERE outcome='error'",
            [],
            |row| row.get::<_, i64>(0),
        )?)?,
        annotations: usize::try_from(transaction.query_row(
            "SELECT COUNT(*) FROM annotations",
            [],
            |row| row.get::<_, i64>(0),
        )?)?,
        replayed_events,
    };
    plan.finish(&transaction, root, replayed_events)?;
    transaction.commit().context("committing index refresh")?;
    Ok(stats)
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
