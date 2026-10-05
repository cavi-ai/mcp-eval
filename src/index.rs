use std::collections::{HashMap, HashSet};
use std::fs::DirEntry;
use std::path::{Path, PathBuf};

use anyhow::Context;
use rusqlite::{params, Connection};

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

#[derive(Clone, Copy, Eq, PartialEq, Ord, PartialOrd, Hash)]
enum ContextKey<'a> {
    Capture(uuid::Uuid),
    Legacy(&'a str, &'a str),
}

fn context_key(record: &CallRecord) -> ContextKey<'_> {
    match &record.identity {
        Some(identity) => ContextKey::Capture(identity.capture_id),
        None => ContextKey::Legacy(&record.session, &record.server),
    }
}

/// Replays with a modern event ID count once. Conflicting IDs or capture
/// coordinates are corruption, never evidence to silently merge or renumber.
fn prepare_records(input: Vec<CallRecord>) -> anyhow::Result<(Vec<CallRecord>, usize)> {
    let mut records: Vec<CallRecord> = Vec::with_capacity(input.len());
    let mut events = HashMap::new();
    let mut captures = HashMap::new();
    let mut positions = HashSet::new();
    let mut replays = 0;
    for record in input {
        if let Some(identity) = &record.identity {
            identity.validate()?;
            if let Some(&index) = events.get(&identity.event_id) {
                if records[index] != record {
                    anyhow::bail!("conflicting records for one event ID");
                }
                replays += 1;
                continue;
            }
            if !positions.insert((identity.capture_id, record.seq)) {
                anyhow::bail!("multiple event IDs claim one capture sequence");
            }
            if let Some(&index) = captures.get(&identity.capture_id) {
                let first: &CallRecord = &records[index];
                if first.session != record.session || first.server != record.server {
                    anyhow::bail!("capture ID crosses session or server boundaries");
                }
            } else {
                captures.insert(identity.capture_id, records.len());
            }
            events.insert(identity.event_id, records.len());
        }
        records.push(record);
    }
    Ok((records, replays))
}

pub fn build(root: &Path) -> anyhow::Result<Stats> {
    let (mut records, replayed_events) = prepare_records(load_records(root)?)?;
    // Captures have independent sequence spaces. Legacy records can only
    // establish context within one session/server and without duplicate seqs.
    records.sort_by(|left, right| {
        context_key(left)
            .cmp(&context_key(right))
            .then(left.seq.cmp(&right.seq))
    });
    let mut legacy_positions = HashSet::new();
    let mut ambiguous_legacy = HashSet::new();
    for record in records.iter().filter(|record| record.identity.is_none()) {
        if !legacy_positions.insert((&record.session, &record.server, record.seq)) {
            ambiguous_legacy.insert(context_key(record));
        }
    }

    let failures = records
        .iter()
        .filter(|record| record.outcome == "error")
        .count();
    let mut db = Connection::open(root.join("index.db")).context("opening index.db")?;
    db.busy_timeout(std::time::Duration::from_secs(5))?;
    db.pragma_update(None, "foreign_keys", "ON")?;
    let transaction = db.transaction().context("starting index rebuild")?;
    transaction
        .execute_batch(DROP_SCHEMA)
        .context("dropping prior index schema")?;
    transaction
        .execute_batch(SCHEMA)
        .context("creating index schema")?;

    let mut ids = Vec::with_capacity(records.len());
    let mut events = HashMap::new();
    let mut coordinates: HashMap<(&str, u64), Option<i64>> = HashMap::new();
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
              err_code, err_template, err_template_id, err_retryable, args, kind, capture_id, event_id)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16)",
            params![
                record.ts,
                record.session,
                record.seq as i64,
                record.server,
                record.method,
                record.tool,
                record.latency_ms.map(|value| value as i64),
                record.outcome,
                error_code,
                error.and_then(|value| value.template.as_ref()),
                error.and_then(|value| value.template_id.as_ref()),
                error.and_then(|value| value.retryable).map(i64::from),
                args,
                record.kind,
                record.identity.as_ref().map(|identity| identity.capture_id.to_string()),
                record.identity.as_ref().map(|identity| identity.event_id.to_string()),
            ],
        )?;
        let id = transaction.last_insert_rowid();
        ids.push(id);
        if let Some(identity) = &record.identity {
            events.insert(identity.event_id, id);
        }
        coordinates
            .entry((&record.session, record.seq))
            .and_modify(|target| *target = None)
            .or_insert(Some(id));
    }

    for (failure_index, failure) in records.iter().enumerate() {
        if failure.outcome != "error" {
            continue;
        }
        if ambiguous_legacy.contains(&context_key(failure)) {
            continue;
        }
        for distance in 1..=5usize {
            let Some(neighbour_index) = failure_index.checked_sub(distance) else {
                break;
            };
            if context_key(&records[neighbour_index]) != context_key(failure) {
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
            if context_key(neighbour) != context_key(failure) {
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

    let annotations = load_annotations(root)?;
    for annotation in &annotations {
        annotation.validate_target()?;
        let target = match (&annotation.event_id, &annotation.session, annotation.seq) {
            (Some(event), _, _) => events.get(event).copied(),
            (None, Some(session), Some(seq)) => {
                coordinates.get(&(session.as_str(), seq)).copied().flatten()
            }
            _ => unreachable!("validated annotation target"),
        };
        transaction.execute(
            "INSERT INTO annotations (session, seq, ts, kind, note, event_id, call_id) VALUES (?1,?2,?3,?4,?5,?6,?7)",
            params![
                annotation.session,
                annotation.seq.map(|seq| seq as i64),
                annotation.ts,
                annotation.kind,
                annotation.note,
                annotation.event_id.map(|id| id.to_string()),
                target,
            ],
        )?;
    }

    transaction.commit().context("committing index rebuild")?;
    Ok(Stats {
        calls: records.len(),
        failures,
        annotations: annotations.len(),
        replayed_events,
    })
}

fn load_records(root: &Path) -> anyhow::Result<Vec<CallRecord>> {
    Ok(load_jsonl(root, "calls-")?
        .into_iter()
        .map(|record: CallRecord| record.sanitized())
        .collect())
}

fn load_annotations(root: &Path) -> anyhow::Result<Vec<AnnotationRecord>> {
    Ok(load_jsonl(root, "annotations-")?
        .into_iter()
        .map(|record: AnnotationRecord| record.sanitized())
        .collect())
}

/// Loads every `<prefix>*.jsonl` file in `<root>/store`, in deterministic
/// name order. Tolerates an unterminated trailing line from an active
/// writer, but a malformed *complete* line is a hard error — the caller
/// rolls back rather than indexing a partial, silently-wrong picture.
fn load_jsonl<T: serde::de::DeserializeOwned>(root: &Path, prefix: &str) -> anyhow::Result<Vec<T>> {
    let store_dir = root.join("store");
    let mut paths = std::fs::read_dir(&store_dir)
        .with_context(|| format!("reading {}", store_dir.display()))?
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
