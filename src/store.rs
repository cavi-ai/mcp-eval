use std::fs::{create_dir_all, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::Context;

use crate::record::{AnnotationRecord, CallRecord};

pub struct Store {
    root: PathBuf,
}

impl Store {
    pub fn open(root: Option<PathBuf>) -> anyhow::Result<Self> {
        let root = root
            .or_else(|| std::env::var_os("MCPEVAL_HOME").map(PathBuf::from))
            .unwrap_or_else(|| {
                let home = std::env::var_os("HOME")
                    .map(PathBuf::from)
                    .unwrap_or_default();
                home.join(".mcp-eval")
            });
        create_dir_all(root.join("store")).context("creating store directory")?;
        Ok(Self { root })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn append(&mut self, rec: &CallRecord) -> anyhow::Result<()> {
        let safe = rec.sanitized();
        let day = safe.ts.get(..10).unwrap_or("unknown");
        let path = self.root.join("store").join(format!("calls-{day}.jsonl"));
        let mut file = OpenOptions::new()
            .create(true)
            .read(true)
            .append(true)
            .open(&path)
            .with_context(|| format!("opening {}", path.display()))?;
        file.lock()
            .with_context(|| format!("locking {}", path.display()))?;
        let mut line = serde_json::to_string(&safe)?;
        line.push('\n');
        file.write_all(line.as_bytes())?;
        file.unlock()
            .with_context(|| format!("unlocking {}", path.display()))?;
        Ok(())
    }

    /// Mirrors `append`: same directory, same file locking, session hashed
    /// through `privacy::opaque_session`, one JSON line per call. `kind` is
    /// re-validated here too — the CLI path calls `AnnotationRecord::validate`
    /// first, but this is the same defense-in-depth layer `append` applies
    /// to `server`/`method`/`tool`, for callers that reach the store
    /// directly without validating first.
    pub fn append_annotation(&mut self, rec: &AnnotationRecord) -> anyhow::Result<()> {
        let day = rec.ts.get(..10).unwrap_or("unknown");
        let path = self
            .root
            .join("store")
            .join(format!("annotations-{day}.jsonl"));
        let mut file = OpenOptions::new()
            .create(true)
            .read(true)
            .append(true)
            .open(&path)
            .with_context(|| format!("opening {}", path.display()))?;
        file.lock()
            .with_context(|| format!("locking {}", path.display()))?;
        let mut safe = rec.clone();
        safe.session = crate::privacy::opaque_session(&safe.session);
        if !crate::record::ANNOTATION_KINDS.contains(&safe.kind.as_str()) {
            safe.kind = "invalid".into();
        }
        let mut line = serde_json::to_string(&safe)?;
        line.push('\n');
        file.write_all(line.as_bytes())?;
        file.unlock()
            .with_context(|| format!("unlocking {}", path.display()))?;
        Ok(())
    }
}
