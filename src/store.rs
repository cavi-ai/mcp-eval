use std::fs::{create_dir_all, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::Context;

use crate::record::CallRecord;

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
        let day = rec.ts.get(..10).unwrap_or("unknown");
        let path = self.root.join("store").join(format!("calls-{day}.jsonl"));
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .with_context(|| format!("opening {}", path.display()))?;
        let line = serde_json::to_string(rec)?;
        file.write_all(line.as_bytes())?;
        file.write_all(b"\n")?;
        Ok(())
    }
}
