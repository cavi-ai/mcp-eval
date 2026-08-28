//! Score calibration against the observed distribution of real servers.
//!
//! A score without a referent is a number; against a population it becomes
//! a percentile. The corpus is a checked-in JSON document of
//! (server label, readiness score) observations gathered by running the
//! battery over popular public servers (see scripts/corpus). Calibration is
//! deterministic: same score against same corpus, same percentile.

use std::path::Path;

use anyhow::Context;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct Corpus {
    /// e.g. "mcpeval.readiness-corpus/v1".
    pub schema: String,
    /// Where the observations came from; informational.
    pub source: String,
    pub observations: Vec<Observation>,
}

#[derive(Debug, Deserialize)]
pub struct Observation {
    pub server: String,
    pub score: u64,
}

impl Corpus {
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        let body = std::fs::read_to_string(path)
            .with_context(|| format!("reading corpus {}", path.display()))?;
        let corpus: Self = serde_json::from_str(&body).context("parsing readiness corpus")?;
        corpus.validate()?;
        Ok(corpus)
    }

    fn validate(&self) -> anyhow::Result<()> {
        if self.schema != "mcpeval.readiness-corpus/v1" {
            anyhow::bail!("unsupported corpus schema {}", self.schema);
        }
        if self.observations.is_empty() {
            anyhow::bail!("corpus has no observations");
        }
        Ok(())
    }

    /// Percentile of `score` within the corpus, 0-100: the share of
    /// observed servers this score matches or beats.
    pub fn percentile(&self, score: u64) -> u64 {
        let mut scores: Vec<u64> = self.observations.iter().map(|o| o.score).collect();
        scores.sort_unstable();
        let below = scores.iter().filter(|&&observed| observed < score).count();
        let at_or_below = scores.iter().filter(|&&observed| observed <= score).count();
        // Midpoint percentile: strictly-below share plus half the ties,
        // which keeps the median observation at 50 rather than 100.
        let share = (below as f64 + (at_or_below - below) as f64 / 2.0) / scores.len() as f64;
        ((share * 100.0).round() as i64).clamp(0, 100) as u64
    }

    /// Median observed score, for one-line context in reports.
    pub fn median(&self) -> u64 {
        let mut scores: Vec<u64> = self.observations.iter().map(|o| o.score).collect();
        scores.sort_unstable();
        let middle = scores.len() / 2;
        if scores.len().is_multiple_of(2) {
            (scores[middle - 1] + scores[middle]) / 2
        } else {
            scores[middle]
        }
    }
}

/// Resolve the corpus in priority order: explicit override, MCPEVAL_HOME,
/// repository default. Missing everywhere is a normal state — reports
/// simply omit percentile context.
pub fn resolve(explicit: Option<&Path>, home: &Path) -> Option<Corpus> {
    if let Some(path) = explicit {
        return Corpus::load(path).ok();
    }
    let home_corpus = home.join("corpus.json");
    if home_corpus.is_file() {
        return Corpus::load(&home_corpus).ok();
    }
    // Repository default lives beside the crate; useful for development
    // and for the canonical published corpus.
    let repository_default =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("data/readiness-corpus.json");
    if repository_default.is_file() {
        return Corpus::load(&repository_default).ok();
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn corpus(scores: &[u64]) -> Corpus {
        Corpus {
            schema: "mcpeval.readiness-corpus/v1".into(),
            source: "test".into(),
            observations: scores
                .iter()
                .enumerate()
                .map(|(index, &score)| Observation {
                    server: format!("server-{index}"),
                    score,
                })
                .collect(),
        }
    }

    #[test]
    fn percentile_places_the_median_observation_at_fifty() {
        let corpus = corpus(&[10, 20, 30, 40, 50]);
        assert_eq!(corpus.median(), 30);
        assert_eq!(corpus.percentile(30), 50);
        assert_eq!(corpus.percentile(10), 10);
        assert_eq!(corpus.percentile(50), 90);
        assert_eq!(corpus.percentile(100), 100);
        assert_eq!(corpus.percentile(0), 0);
    }

    #[test]
    fn percentile_handles_duplicates_and_extremes() {
        let corpus = corpus(&[50, 50, 50, 50, 50]);
        assert_eq!(corpus.percentile(50), 50);
        assert_eq!(corpus.percentile(49), 0);
        assert_eq!(corpus.percentile(51), 100);
    }

    #[test]
    fn corpus_rejects_unknown_schema_and_emptiness() {
        let loaded = corpus(&[1]);
        assert!(Corpus::load(Path::new("/nonexistent")).is_err());
        let mut broken = loaded;
        broken.schema = "other".into();
        assert!(broken.validate().is_err());
        let empty = corpus(&[]);
        assert!(empty.validate().is_err());
    }
}
