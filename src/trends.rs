//! Readiness-score history for full-battery probe runs.
//!
//! One JSON line per run is appended to `<MCPEVAL_HOME>/store/probes/
//! history.jsonl`. The records stay inside the share-safe store boundary:
//! server label, verdict counts, score, and a timestamp — the same class of
//! metadata the journal already keeps.

use std::fmt::Write;
use std::io::Write as IoWrite;
use std::path::Path;

use anyhow::Context;
use serde::{Deserialize, Serialize};

use crate::probe::ProbeReport;
use crate::score;

#[derive(Debug, Serialize, Deserialize)]
struct TrendPoint {
    ts: String,
    server: String,
    passed: bool,
    cases_total: u64,
    cases_passed: u64,
    score: u64,
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

/// Grouped-by-server recent history, oldest first within each group, with a
/// score delta against the previous run of the same server.
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
        let previous_score = std::cell::Cell::new(None::<u64>);
        for point in runs.iter().rev().take(last).rev() {
            let delta = match previous_score.get() {
                Some(previous) => {
                    let difference = point.score as i64 - previous as i64;
                    format!(" {difference:+}")
                }
                None => String::new(),
            };
            previous_score.set(Some(point.score));
            writeln!(
                out,
                "  {} score={}/100 cases={}/{}{}{}",
                point.ts,
                point.score,
                point.cases_passed,
                point.cases_total,
                if point.passed { "" } else { " FAILING" },
                delta
            )?;
        }
    }
    Ok(out)
}
