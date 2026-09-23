//! Score calibration against the observed distribution of real servers.
//!
//! A score without a referent is a number; against a population it has a
//! place. The corpus is a checked-in JSON document of observations gathered
//! by running one fixed battery over popular public servers (see
//! scripts/corpus). A report is placed on that battery's cases only, and
//! placement counts ties explicitly, so a perfect score among perfect
//! scores reads as tied rather than as a middling share. Calibration is
//! deterministic: same score against same corpus, same placement.

use std::path::Path;

use anyhow::Context;
use serde::Deserialize;

use crate::manifest::ProbeKind;

#[derive(Debug, Deserialize)]
pub struct Corpus {
    /// e.g. "mcpeval.readiness-corpus/v1".
    pub schema: String,
    /// Where the observations came from; informational.
    pub source: String,
    /// Probe kinds every observation was scored on.
    #[serde(default = "default_battery", deserialize_with = "probe_kinds")]
    pub battery: Vec<ProbeKind>,
    pub observations: Vec<Observation>,
}

#[derive(Debug, Deserialize)]
pub struct Observation {
    pub server: String,
    pub score: u64,
    /// Tools the server listed, when the collector recorded it.
    #[serde(default)]
    pub tool_count: Option<u64>,
    /// The server's token-cost catalog estimate, when recorded.
    #[serde(default)]
    pub catalog_tokens: Option<u64>,
}

/// Where a score sits among all observations: observed servers it scores
/// above, ties, and scores below.
#[derive(Debug, Eq, PartialEq)]
pub struct Placement {
    pub above: usize,
    pub tied: usize,
    pub below: usize,
}

/// Where a catalog's token estimate sits among the observations that
/// recorded one.
#[derive(Debug, Eq, PartialEq)]
pub struct CatalogPlacement {
    /// Observations with a strictly larger catalog.
    pub lighter_than: usize,
    /// Observations with a strictly smaller catalog.
    pub heavier_than: usize,
    /// Observations that recorded a catalog estimate.
    pub observed: usize,
    /// Median recorded estimate; the lower middle for an even count.
    pub median_tokens: u64,
}

/// The battery every v1 corpus without a `battery` field was collected with.
fn default_battery() -> Vec<ProbeKind> {
    vec![
        ProbeKind::DiscoveryCost,
        ProbeKind::TokenCost,
        ProbeKind::Pagination,
        ProbeKind::SurfaceListing,
    ]
}

fn probe_kinds<'de, D>(deserializer: D) -> Result<Vec<ProbeKind>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Vec::<String>::deserialize(deserializer)?
        .iter()
        .map(|label| {
            ProbeKind::from_report_label(label)
                .ok_or_else(|| serde::de::Error::custom(format!("unknown probe kind {label}")))
        })
        .collect()
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
        if self.battery.is_empty() {
            anyhow::bail!("corpus battery is empty");
        }
        if self.observations.is_empty() {
            anyhow::bail!("corpus has no observations");
        }
        Ok(())
    }

    /// The battery's probe kinds, comma-separated, for report lines.
    pub fn battery_label(&self) -> String {
        self.battery
            .iter()
            .map(ProbeKind::as_str)
            .collect::<Vec<_>>()
            .join(", ")
    }

    pub fn placement(&self, score: u64) -> Placement {
        let scores = self
            .observations
            .iter()
            .map(|observation| observation.score);
        Placement {
            above: scores.clone().filter(|&observed| observed < score).count(),
            tied: scores.clone().filter(|&observed| observed == score).count(),
            below: scores.filter(|&observed| observed > score).count(),
        }
    }

    /// `None` when no observation recorded a catalog estimate.
    pub fn catalog_placement(&self, tokens: u64) -> Option<CatalogPlacement> {
        let mut recorded: Vec<u64> = self
            .observations
            .iter()
            .filter_map(|observation| observation.catalog_tokens)
            .collect();
        if recorded.is_empty() {
            return None;
        }
        recorded.sort_unstable();
        let median_tokens = recorded[(recorded.len() - 1) / 2];
        Some(CatalogPlacement {
            lighter_than: recorded.iter().filter(|&&other| other > tokens).count(),
            heavier_than: recorded.iter().filter(|&&other| other < tokens).count(),
            observed: recorded.len(),
            median_tokens,
        })
    }
}

/// Resolve the corpus in priority order: explicit override, MCPEVAL_HOME,
/// repository default. Missing everywhere is a normal state — reports
/// simply omit calibration context.
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
            battery: default_battery(),
            observations: scores
                .iter()
                .enumerate()
                .map(|(index, &score)| Observation {
                    server: format!("server-{index}"),
                    score,
                    tool_count: None,
                    catalog_tokens: None,
                })
                .collect(),
        }
    }

    fn parse(document: &str) -> anyhow::Result<Corpus> {
        let corpus: Corpus = serde_json::from_str(document)?;
        corpus.validate()?;
        Ok(corpus)
    }

    #[test]
    fn placement_counts_ties_at_the_top() {
        let mut scores = vec![100; 33];
        scores.push(75);
        let corpus = corpus(&scores);
        assert_eq!(
            corpus.placement(100),
            Placement {
                above: 1,
                tied: 33,
                below: 0
            }
        );
        assert_eq!(
            corpus.placement(94),
            Placement {
                above: 1,
                tied: 0,
                below: 33
            }
        );
        assert_eq!(
            corpus.placement(75),
            Placement {
                above: 0,
                tied: 1,
                below: 33
            }
        );
    }

    #[test]
    fn catalog_placement_uses_only_observations_with_measurements() {
        let mut corpus = corpus(&[100, 100, 100, 100, 100]);
        assert_eq!(corpus.catalog_placement(500), None);
        for (observation, tokens) in corpus.observations.iter_mut().zip([4000, 300, 900, 1500]) {
            observation.catalog_tokens = Some(tokens);
        }
        assert_eq!(
            corpus.catalog_placement(900),
            Some(CatalogPlacement {
                lighter_than: 2,
                heavier_than: 1,
                observed: 4,
                median_tokens: 900,
            })
        );
        corpus.observations[4].catalog_tokens = Some(10);
        assert_eq!(
            corpus.catalog_placement(20_000),
            Some(CatalogPlacement {
                lighter_than: 0,
                heavier_than: 5,
                observed: 5,
                median_tokens: 900,
            })
        );
    }

    #[test]
    fn battery_defaults_to_the_v1_collection_battery_and_rejects_empty() {
        let legacy = parse(
            r#"{"schema":"mcpeval.readiness-corpus/v1","source":"s",
                "observations":[{"server":"a","score":100}]}"#,
        )
        .unwrap();
        assert_eq!(legacy.battery, default_battery());
        assert_eq!(
            legacy.battery_label(),
            "discovery-cost, token-cost, pagination, surface-listing"
        );
        assert_eq!(legacy.observations[0].tool_count, None);
        assert_eq!(legacy.observations[0].catalog_tokens, None);

        let explicit = parse(
            r#"{"schema":"mcpeval.readiness-corpus/v1","source":"s",
                "battery":["token-cost","latency-budget"],
                "observations":[{"server":"a","score":90,"tool_count":12,"catalog_tokens":566}]}"#,
        )
        .unwrap();
        assert_eq!(
            explicit.battery,
            [ProbeKind::TokenCost, ProbeKind::LatencyBudget]
        );
        assert_eq!(explicit.observations[0].tool_count, Some(12));
        assert_eq!(explicit.observations[0].catalog_tokens, Some(566));

        assert!(parse(
            r#"{"schema":"mcpeval.readiness-corpus/v1","source":"s","battery":[],
                "observations":[{"server":"a","score":100}]}"#,
        )
        .is_err());
        assert!(parse(
            r#"{"schema":"mcpeval.readiness-corpus/v1","source":"s","battery":["not-a-probe"],
                "observations":[{"server":"a","score":100}]}"#,
        )
        .is_err());
    }

    #[test]
    fn repository_corpus_loads_as_checked_in() {
        let corpus =
            Corpus::load(&Path::new(env!("CARGO_MANIFEST_DIR")).join("data/readiness-corpus.json"))
                .unwrap();
        assert!(!corpus.battery.is_empty());
        assert!(!corpus.observations.is_empty());
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
