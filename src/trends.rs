//! Readiness-score history for full-battery probe runs.
//!
//! One JSON line per run is appended to `<MCPEVAL_HOME>/store/probes/
//! history.jsonl`. The records stay inside the share-safe store boundary:
//! server label, verdict counts, score, manifest hash, and a timestamp — the
//! same class of metadata the journal already keeps.

use std::fmt::Write;
use std::io::Write as IoWrite;
use std::path::Path;

use anyhow::Context;
use serde::{Deserialize, Serialize};

use crate::probe::ProbeReport;
use crate::score;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrendPoint {
    pub ts: String,
    pub server: String,
    pub passed: bool,
    pub cases_total: u64,
    pub cases_passed: u64,
    pub score: u64,
    /// SHA-256 of the manifest the run parsed; absent in history recorded
    /// before runs carried it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manifest_sha256: Option<String>,
}

impl TrendPoint {
    /// `score=… cases=…`, the verdict, the score delta against `previous`
    /// when both runs used the same manifest (` manifest changed` when they
    /// did not), and the manifest hash prefix.
    pub fn summary(&self, previous: Option<&TrendPoint>) -> String {
        let mut out = format!(
            "score={}/100 cases={}/{}{}",
            self.score,
            self.cases_passed,
            self.cases_total,
            if self.passed { "" } else { " FAILING" }
        );
        if let Some(previous) = previous {
            if previous.manifest_sha256 == self.manifest_sha256 {
                let difference = self.score as i64 - previous.score as i64;
                out.push_str(&format!(" {difference:+}"));
            } else {
                out.push_str(" manifest changed");
            }
        }
        if let Some(manifest) = &self.manifest_sha256 {
            let prefix: String = manifest.chars().take(8).collect();
            out.push_str(&format!(" manifest={prefix}"));
        }
        out
    }
}

pub fn record(root: &Path, server: &str, report: &ProbeReport) -> anyhow::Result<()> {
    let path = history_path(root);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).context("creating trend directory")?;
    }
    let readiness = score::readiness(report);
    let point = TrendPoint {
        ts: chrono::Utc::now()
            .format("%Y-%m-%dT%H:%M:%S%.3fZ")
            .to_string(),
        server: server.to_owned(),
        passed: report.passed(),
        cases_total: report.cases.len() as u64,
        cases_passed: report.cases.iter().filter(|case| case.passed()).count() as u64,
        score: readiness.overall,
        manifest_sha256: report.manifest_sha256.clone(),
    };
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .context("opening trend history")?;
    writeln!(file, "{}", serde_json::to_string(&point)?).context("appending trend point")?;
    Ok(())
}

fn history_path(root: &Path) -> std::path::PathBuf {
    root.join("store").join("probes").join("history.jsonl")
}

/// Recent trend points for consumers that serve them programmatically
/// (`mcpeval serve`); oldest first, at most `last` per server.
pub fn load(root: &Path, last: usize) -> anyhow::Result<Vec<TrendPoint>> {
    let path = history_path(root);
    if !path.is_file() {
        return Ok(Vec::new());
    }
    let body = std::fs::read_to_string(&path).context("reading trend history")?;
    let mut points: Vec<TrendPoint> = body
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(serde_json::from_str)
        .collect::<Result<_, _>>()
        .context("trend history is corrupt")?;
    points.sort_by(|a, b| a.ts.cmp(&b.ts).then_with(|| a.server.cmp(&b.server)));
    let mut servers: Vec<String> = Vec::new();
    for point in &points {
        if !servers.contains(&point.server) {
            servers.push(point.server.clone());
        }
    }
    let mut selected = Vec::new();
    for server in servers {
        let runs: Vec<&TrendPoint> = points
            .iter()
            .filter(|point| point.server == server)
            .collect();
        selected.extend(
            runs.iter()
                .rev()
                .take(last)
                .rev()
                .map(|point| (*point).clone()),
        );
    }
    Ok(selected)
}

/// Grouped-by-server recent history, oldest first within each group, with a
/// score delta against the previous run of the same server and manifest.
pub fn render(root: &Path, last: usize) -> anyhow::Result<String> {
    let path = history_path(root);
    if !path.is_file() {
        return Ok("no trend history yet; run `mcpeval probe` first\n".into());
    }
    let body = std::fs::read_to_string(&path).context("reading trend history")?;
    let mut points: Vec<TrendPoint> = body
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(serde_json::from_str)
        .collect::<Result<_, _>>()
        .context("trend history is corrupt")?;
    points.sort_by(|a, b| a.ts.cmp(&b.ts).then_with(|| a.server.cmp(&b.server)));

    let mut servers: Vec<String> = Vec::new();
    for point in &points {
        if !servers.contains(&point.server) {
            servers.push(point.server.clone());
        }
    }

    let mut out = String::new();
    for server in servers {
        let runs: Vec<&TrendPoint> = points
            .iter()
            .filter(|point| point.server == server)
            .collect();
        out.push_str(&format!("{server}\n"));
        let mut previous = None;
        for point in runs.iter().rev().take(last).rev() {
            writeln!(out, "  {} {}", point.ts, point.summary(previous))?;
            previous = Some(*point);
        }
    }
    Ok(out)
}
