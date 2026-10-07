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
    /// The readiness standard behind `score`; absent in history recorded
    /// before the standard existed, when `score` was a manifest pass rate.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub standard: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub measurement_profile: Option<crate::measurement::MeasurementProfile>,
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
            if previous.standard != self.standard {
                out.push_str(" standard changed");
            } else if self.standard.is_some()
                && (self.measurement_profile.is_none() || previous.measurement_profile.is_none())
            {
                out.push_str(" measurement profile unavailable");
            } else if self.measurement_profile != previous.measurement_profile {
                out.push_str(" measurement profile changed");
            } else if self.standard.is_some() || previous.manifest_sha256 == self.manifest_sha256 {
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
    // A point is a readiness measurement: gate-only runs record none.
    let Some(readiness) = &report.readiness else {
        return Ok(());
    };
    let point = TrendPoint {
        ts: chrono::Utc::now()
            .format("%Y-%m-%dT%H:%M:%S%.3fZ")
            .to_string(),
        server: server.to_owned(),
        passed: report.passed(),
        cases_total: report.cases.len() as u64,
        cases_passed: report.cases.iter().filter(|case| case.passed()).count() as u64,
        score: readiness.score,
        manifest_sha256: report.manifest_sha256.clone(),
        standard: Some(readiness.standard.clone()),
        measurement_profile: readiness.measurement_profile.clone(),
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
    let mut groups: std::collections::BTreeMap<String, Vec<TrendPoint>> =
        std::collections::BTreeMap::new();
    let mut reader = std::io::BufReader::new(std::fs::File::open(&path)?);
    crate::jsonl::visit(&mut reader, |point: TrendPoint| {
        if let Some(profile) = &point.measurement_profile {
            profile.validate()?;
        }
        if last == 0 {
            return Ok(());
        }
        let recent = groups.entry(point.server.clone()).or_default();
        let index = recent.partition_point(|existing| existing.ts <= point.ts);
        recent.insert(index, point);
        if recent.len() > last {
            recent.remove(0);
        }
        Ok(())
    })
    .context("trend history is corrupt")?;
    Ok(groups.into_values().flatten().collect())
}

/// Recent history with deltas only under compatible measurement conditions.
pub fn render(root: &Path, last: usize) -> anyhow::Result<String> {
    if !history_path(root).is_file() {
        return Ok("no trend history yet; run `mcpeval probe` first\n".into());
    }
    let points = load(root, last)?;
    let mut out = String::new();
    let mut previous: Option<&TrendPoint> = None;
    for point in &points {
        if previous.is_none_or(|p| p.server != point.server) {
            writeln!(out, "{}", point.server)?;
            previous = None;
        }
        writeln!(out, "  {} {}", point.ts, point.summary(previous))?;
        previous = Some(point);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn point(score: u64, standard: Option<&str>, manifest: &str) -> TrendPoint {
        TrendPoint {
            ts: "2026-09-30T00:00:00.000Z".into(),
            server: "demo".into(),
            passed: true,
            cases_total: 1,
            cases_passed: 1,
            score,
            manifest_sha256: Some(manifest.into()),
            standard: standard.map(str::to_owned),
            measurement_profile: Some(crate::measurement::MeasurementProfile::current(false, &[])),
        }
    }

    #[test]
    fn deltas_compare_one_standard_only() {
        let legacy = point(100, None, "aaaaaaaa");
        let first = point(90, Some("mcpeval-standard/1"), "aaaaaaaa");
        let second = point(85, Some("mcpeval-standard/1"), "bbbbbbbb");
        assert!(first.summary(Some(&legacy)).contains(" standard changed"));
        // A new manifest does not change what the standard score means.
        assert!(second.summary(Some(&first)).contains(" -5"));
        assert!(!second.summary(Some(&first)).contains("manifest changed"));
    }

    #[test]
    fn recent_history_handles_out_of_order_records_and_an_active_writer_tail() {
        let root = std::env::temp_dir().join(format!("mcpeval-trends-{}", uuid::Uuid::new_v4()));
        let path = history_path(&root);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mut records = String::new();
        for (server, timestamp, score) in [
            ("b", "03", 30),
            ("a", "02", 20),
            ("b", "01", 10),
            ("a", "03", 30),
            ("a", "01", 10),
            ("b", "02", 20),
        ] {
            let mut next = point(score, Some("mcpeval-standard/2"), "aaaaaaaa");
            next.server = server.into();
            next.ts = timestamp.into();
            records.push_str(&serde_json::to_string(&next).unwrap());
            records.push('\n');
        }
        std::fs::write(&path, format!("{records}{{\"unfinished\":")).unwrap();
        let recent = load(&root, 2).unwrap();
        assert_eq!(
            recent
                .iter()
                .map(|p| (p.server.as_str(), p.score))
                .collect::<Vec<_>>(),
            vec![("a", 20), ("a", 30), ("b", 20), ("b", 30)]
        );
        assert!(load(&root, 0).unwrap().is_empty());
        std::fs::write(&path, format!("{records}{{invalid}}\n")).unwrap();
        assert!(load(&root, 2).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }
}
