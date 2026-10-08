//! Content-validated checkpoints for the derived history index.
use std::collections::BTreeMap;
use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use anyhow::Context;
use rusqlite::{params, Transaction};
use sha2::{Digest, Sha256};

pub(super) const SCHEMA: &str = "
CREATE TABLE index_sources(path TEXT PRIMARY KEY, offset INTEGER NOT NULL, digest TEXT NOT NULL);
CREATE TABLE index_events(event TEXT PRIMARY KEY, digest TEXT NOT NULL);
CREATE TABLE index_captures(capture TEXT PRIMARY KEY, session TEXT NOT NULL, server TEXT NOT NULL);
CREATE TABLE index_checkpoint(singleton INTEGER PRIMARY KEY CHECK(singleton=1), version INTEGER NOT NULL, schema_digest TEXT NOT NULL, replayed_events INTEGER NOT NULL);
";
pub(super) const DROP_SCHEMA: &str = "
DROP TABLE IF EXISTS index_checkpoint;
DROP TABLE IF EXISTS index_sources;
DROP TABLE IF EXISTS index_events;
DROP TABLE IF EXISTS index_captures;
";
// Invalidate caches whenever parsing, sanitization, identity, or correlation
// semantics change, even when the SQL schema does not.
const VERSION: i64 = 1;

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

pub(super) fn record_digest(record: &crate::record::CallRecord) -> anyhow::Result<String> {
    Ok(hex(&Sha256::digest(serde_json::to_vec(record)?)))
}

pub(super) struct Source {
    name: String,
    path: PathBuf,
    length: u64,
    snapshot: String,
    offset: u64,
}

pub(super) struct Plan {
    pub files: Vec<Source>,
    pub incremental: bool,
    pub replayed_events: usize,
}

fn paths(root: &Path) -> anyhow::Result<Vec<PathBuf>> {
    let mut paths = Vec::new();
    for entry in std::fs::read_dir(root.join("store"))? {
        let entry = entry?;
        if super::is_prefixed_jsonl_file(&entry, "calls-")?
            || super::is_prefixed_jsonl_file(&entry, "annotations-")?
        {
            paths.push(entry.path());
        }
    }
    paths.sort();
    Ok(paths)
}

fn locked_file(path: &Path) -> anyhow::Result<File> {
    let file = File::open(path).with_context(|| format!("opening {}", path.display()))?;
    file.lock_shared().context("locking journal for indexing")?;
    Ok(file)
}

fn digest(file: &mut File, length: u64) -> anyhow::Result<String> {
    file.seek(SeekFrom::Start(0))?;
    let mut hash = Sha256::new();
    let mut remaining = length;
    let mut buffer = [0u8; 64 * 1024];
    while remaining > 0 {
        let limit = remaining.min(buffer.len() as u64) as usize;
        let count = file.read(&mut buffer[..limit])?;
        anyhow::ensure!(count != 0, "journal was truncated during indexing");
        hash.update(&buffer[..count]);
        remaining -= count as u64;
    }
    Ok(hex(&hash.finalize()))
}

fn schema_digest(db: &Transaction<'_>) -> anyhow::Result<String> {
    let mut hash = Sha256::new();
    let mut statement = db.prepare("SELECT type,name,sql FROM sqlite_master WHERE type IN ('table','index') AND tbl_name IN ('calls','windows','annotations','index_sources','index_events','index_captures','index_checkpoint') ORDER BY type,name")?;
    let mut rows = statement.query([])?;
    while let Some(row) = rows.next()? {
        for column in 0..3 {
            let value: Option<String> = row.get(column)?;
            hash.update(serde_json::to_vec(&value)?);
        }
    }
    Ok(hex(&hash.finalize()))
}

impl Plan {
    pub fn prepare(db: &Transaction<'_>, root: &Path, force: bool) -> anyhow::Result<Self> {
        let exists: bool = db.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='index_checkpoint')", [], |row| row.get(0))?;
        let checkpoint = if exists && !force {
            db.query_row("SELECT version,schema_digest,replayed_events FROM index_checkpoint WHERE singleton=1", [], |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?, row.get::<_, i64>(2)?))).ok()
        } else {
            None
        };
        let compatible = match &checkpoint {
            Some((version, expected, replays)) => {
                *version == VERSION && *replays >= 0 && *expected == schema_digest(db)?
            }
            None => false,
        };
        let mut prior = BTreeMap::<String, (u64, String)>::new();
        if compatible {
            let mut query =
                db.prepare("SELECT path,offset,digest FROM index_sources ORDER BY path")?;
            for row in query.query_map([], |row| {
                Ok((row.get(0)?, (row.get::<_, i64>(1)?, row.get(2)?)))
            })? {
                let (name, checkpoint) = row?;
                prior.insert(name, (u64::try_from(checkpoint.0)?, checkpoint.1));
            }
        }
        let mut incremental = compatible;
        let mut files = Vec::new();
        for path in paths(root)? {
            let name = path.file_name().unwrap().to_str().unwrap().to_owned();
            let mut file = locked_file(&path)?;
            let length = file.metadata()?.len();
            let snapshot = digest(&mut file, length)?;
            let mut offset = 0;
            if let Some((previous, expected)) = prior.get(&name) {
                let matches = if *previous == length {
                    snapshot == *expected
                } else {
                    *previous < length && digest(&mut file, *previous)? == *expected
                };
                if matches {
                    offset = *previous;
                } else {
                    incremental = false;
                }
            }
            files.push(Source {
                name,
                path,
                length,
                snapshot,
                offset,
            });
        }
        // Full rebuilds insert calls and annotations in sorted file order.
        // Only a suffix can extend that order without changing existing IDs.
        for prefix in ["calls-", "annotations-"] {
            let previous: Vec<_> = prior
                .keys()
                .filter(|name| name.starts_with(prefix))
                .collect();
            let current: Vec<_> = files
                .iter()
                .filter(|file| file.name.starts_with(prefix))
                .collect();
            if previous.len() > current.len()
                || previous
                    .iter()
                    .zip(&current)
                    .any(|(old, new)| **old != new.name)
            {
                incremental = false;
            }
            if current
                .iter()
                .take(previous.len().saturating_sub(1))
                .any(|file| file.length != file.offset)
            {
                incremental = false;
            }
        }
        if !incremental {
            for file in &mut files {
                file.offset = 0;
            }
        }
        Ok(Self {
            files,
            incremental,
            replayed_events: if incremental {
                usize::try_from(checkpoint.unwrap().2)?
            } else {
                0
            },
        })
    }

    pub fn visit<T: serde::de::DeserializeOwned>(
        &mut self,
        db: &Transaction<'_>,
        prefix: &str,
        mut accept: impl FnMut(T) -> anyhow::Result<()>,
    ) -> anyhow::Result<bool> {
        let mut changed = false;
        for source in self
            .files
            .iter_mut()
            .filter(|source| source.name.starts_with(prefix))
        {
            if source.offset == source.length {
                continue;
            }
            let mut file = locked_file(&source.path)?;
            anyhow::ensure!(
                digest(&mut file, source.length)? == source.snapshot,
                "journal changed during indexing; retry"
            );
            file.seek(SeekFrom::Start(source.offset))?;
            let remaining = source.length - source.offset;
            let complete = crate::jsonl::visit_complete(
                &mut BufReader::new((&mut file).take(remaining)),
                |value| {
                    changed = true;
                    accept(value)
                },
            )
            .with_context(|| format!("reading {}", source.path.display()))?;
            source.offset += complete;
            let prefix_digest = if source.offset == source.length {
                source.snapshot.clone()
            } else {
                digest(&mut file, source.offset)?
            };
            db.prepare_cached(
                "INSERT OR REPLACE INTO index_sources(path,offset,digest) VALUES (?1,?2,?3)",
            )?
            .execute(params![
                source.name,
                i64::try_from(source.offset)?,
                prefix_digest
            ])?;
        }
        Ok(changed)
    }

    pub fn finish(&self, db: &Transaction<'_>, root: &Path, replays: usize) -> anyhow::Result<()> {
        anyhow::ensure!(
            paths(root)?
                == self
                    .files
                    .iter()
                    .map(|source| source.path.clone())
                    .collect::<Vec<_>>(),
            "journal inventory changed during indexing; retry"
        );
        for source in &self.files {
            let mut file = locked_file(&source.path)?;
            anyhow::ensure!(
                digest(&mut file, source.length)? == source.snapshot,
                "journal changed during indexing; retry"
            );
        }
        db.execute(
            "INSERT OR REPLACE INTO index_checkpoint VALUES (1,?1,?2,?3)",
            params![VERSION, schema_digest(db)?, i64::try_from(replays)?],
        )?;
        Ok(())
    }
}
