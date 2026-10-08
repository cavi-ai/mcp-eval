//! Optional, atomic derived cache outside the shareable journal tree.
use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::path::Path;

use rusqlite::{params, Connection, OptionalExtension, Transaction};
use sha2::{Digest, Sha256};

use super::TrendPoint;

// Bump when parsing, profile validation, or ordering semantics change.
const VERSION: i64 = 1;
const SCHEMA: &str = "
CREATE TABLE trend_points(id INTEGER PRIMARY KEY, server TEXT NOT NULL, ts TEXT NOT NULL, point TEXT NOT NULL);
CREATE INDEX trend_recent ON trend_points(server,ts DESC,id DESC);
CREATE TABLE trend_checkpoint(singleton INTEGER PRIMARY KEY CHECK(singleton=1), version INTEGER NOT NULL, schema_digest BLOB NOT NULL, offset INTEGER NOT NULL, digest BLOB NOT NULL);
";

fn digest(file: &mut File, length: u64) -> anyhow::Result<Vec<u8>> {
    file.seek(SeekFrom::Start(0))?;
    let mut hash = Sha256::new();
    let mut remaining = length;
    let mut buffer = [0u8; 64 * 1024];
    while remaining > 0 {
        let limit = remaining.min(buffer.len() as u64) as usize;
        let count = file.read(&mut buffer[..limit])?;
        anyhow::ensure!(count != 0, "trend journal was truncated during refresh");
        hash.update(&buffer[..count]);
        remaining -= count as u64;
    }
    Ok(hash.finalize().to_vec())
}

fn schema_digest(db: &Transaction<'_>) -> anyhow::Result<Vec<u8>> {
    let mut hash = Sha256::new();
    let mut statement = db.prepare("SELECT type,name,sql FROM sqlite_master WHERE type IN ('table','index') AND tbl_name IN ('trend_points','trend_checkpoint') ORDER BY type,name")?;
    let mut rows = statement.query([])?;
    while let Some(row) = rows.next()? {
        for column in 0..3 {
            let value: Option<String> = row.get(column)?;
            hash.update(serde_json::to_vec(&value)?);
        }
    }
    Ok(hash.finalize().to_vec())
}

pub(super) fn load(
    root: &Path,
    path: &Path,
    file: &mut File,
    last: usize,
) -> anyhow::Result<Vec<TrendPoint>> {
    let mut db = Connection::open(root.join("trends.db"))?;
    db.busy_timeout(std::time::Duration::from_secs(5))?;
    db.pragma_update(None, "temp_store", "FILE")?;
    // Serialize refreshes before reading the previous checkpoint. The caller
    // holds a shared journal lock; writers never need the cache database.
    let transaction = db.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let integrity: String = transaction.query_row("PRAGMA quick_check", [], |r| r.get(0))?;
    anyhow::ensure!(integrity == "ok", "trend cache is corrupt");
    let checkpoint_exists: bool = transaction.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='trend_checkpoint')",
        [],
        |r| r.get(0),
    )?;
    let checkpoint = if checkpoint_exists {
        transaction.query_row("SELECT version,schema_digest,offset,digest FROM trend_checkpoint WHERE singleton=1", [], |r| Ok((r.get::<_, i64>(0)?,r.get::<_, Vec<u8>>(1)?,r.get::<_, i64>(2)?,r.get::<_, Vec<u8>>(3)?))).optional().ok().flatten()
    } else {
        None
    };
    let length = file.metadata()?.len();
    let snapshot = digest(file, length)?;
    let mut offset = 0;
    let mut incremental = false;
    if let Some((version, schema, previous, expected)) = checkpoint {
        if version == VERSION && schema == schema_digest(&transaction)? && previous >= 0 {
            let previous = previous as u64;
            incremental = previous <= length
                && if previous == length {
                    snapshot == expected
                } else {
                    digest(file, previous)? == expected
                };
            if incremental {
                offset = previous;
            }
        }
    }
    if !incremental {
        transaction.execute_batch(
            "DROP TABLE IF EXISTS trend_checkpoint; DROP TABLE IF EXISTS trend_points;",
        )?;
        transaction.execute_batch(SCHEMA)?;
    }
    file.seek(SeekFrom::Start(offset))?;
    let complete = crate::jsonl::visit_complete(
        &mut BufReader::new((&mut *file).take(length - offset)),
        |point: TrendPoint| {
            if let Some(profile) = &point.measurement_profile {
                profile.validate()?;
            }
            transaction
                .prepare_cached("INSERT INTO trend_points(server,ts,point) VALUES (?1,?2,?3)")?
                .execute(params![
                    point.server,
                    point.ts,
                    serde_json::to_string(&point)?
                ])?;
            Ok(())
        },
    )?;
    offset += complete;
    let prefix = if offset == length {
        snapshot.clone()
    } else {
        digest(file, offset)?
    };
    transaction.execute(
        "INSERT OR REPLACE INTO trend_checkpoint VALUES (1,?1,?2,?3,?4)",
        params![
            VERSION,
            schema_digest(&transaction)?,
            i64::try_from(offset)?,
            prefix
        ],
    )?;
    let points = {
        let mut statement = transaction.prepare("SELECT point FROM (SELECT server,ts,id,point,ROW_NUMBER() OVER (PARTITION BY server ORDER BY ts DESC,id DESC) AS position FROM trend_points) WHERE position<=?1 ORDER BY server,ts,id")?;
        let mut rows = statement.query([i64::try_from(last).unwrap_or(i64::MAX)])?;
        let mut points = Vec::new();
        while let Some(row) = rows.next()? {
            let point: TrendPoint = serde_json::from_str(&row.get::<_, String>(0)?)?;
            points.push(point);
        }
        points
    };
    // Normal writers remain blocked by the caller's shared lock. Reopen the
    // pathname to detect replacement by an external editor before committing.
    // Do not acquire a second file lock while holding the first one.
    let mut current = File::open(path)?;
    anyhow::ensure!(
        digest(&mut current, length)? == snapshot,
        "trend journal changed during refresh"
    );
    transaction.commit()?;
    Ok(points)
}
