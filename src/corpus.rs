//! Score calibration against the observed distribution of real servers.
//!
//! A score without a referent is a number; against a population it has a
//! place. The corpus is a checked-in JSON document of readiness scores
//! gathered by running mcpeval's standard battery over popular public
//! servers (see scripts/corpus). A report is placed only against a corpus
//! scored under its own standard, and placement counts ties explicitly, so
//! a perfect score among perfect scores reads as tied rather than as a
//! middling share. Calibration is deterministic: same score against same
//! corpus, same placement.

use std::collections::{BTreeMap, HashSet};
use std::path::Path;

use anyhow::Context;
use serde::Deserialize;

/// Historical corpus format retained for compatibility.
pub const SCHEMA: &str = "mcpeval.readiness-corpus/v2";
pub const PROVENANCE_SCHEMA: &str = "mcpeval.readiness-corpus/v3";

#[derive(Debug, Deserialize)]
pub struct Corpus {
    /// Historical v2 or provenance-bearing v3.
    pub schema: String,
    /// Where the observations came from; informational.
    pub source: String,
    /// The readiness standard every observation was scored under.
    pub standard: String,
    pub observations: Vec<Observation>,
}

#[derive(Debug, Deserialize)]
pub struct Observation {
    pub server: String,
    pub score: u64,
    /// Area scores by area name.
    #[serde(default)]
    pub areas: BTreeMap<String, u64>,
    /// Tools the server listed, when the collector recorded it.
    #[serde(default)]
    pub tool_count: Option<u64>,
    /// The server's catalog token estimate, when recorded.
    #[serde(default)]
    pub catalog_tokens: Option<u64>,
    /// Historical rows remain readable without inventing their conditions.
    #[serde(default)]
    pub measurement_profile: Option<crate::measurement::MeasurementProfile>,
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

impl Corpus {
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        let body = std::fs::read_to_string(path)
            .with_context(|| format!("reading corpus {}", path.display()))?;
        let document: serde_json::Value =
            serde_json::from_str(&body).context("parsing readiness corpus")?;
        let corpus: Self =
            serde_json::from_value(document.clone()).context("parsing readiness corpus")?;
        corpus.validate()?;
        if corpus.schema == PROVENANCE_SCHEMA {
            validate_provenance(&document)?;
        }
        Ok(corpus)
    }

    fn validate(&self) -> anyhow::Result<()> {
        if self.schema != SCHEMA && self.schema != PROVENANCE_SCHEMA {
            anyhow::bail!("unsupported corpus schema {}", self.schema);
        }
        if self.standard.is_empty() {
            anyhow::bail!("corpus names no standard");
        }
        if self.observations.is_empty() {
            anyhow::bail!("corpus has no observations");
        }
        let mut servers = HashSet::new();
        for observation in &self.observations {
            if let Some(profile) = &observation.measurement_profile {
                profile.validate()?;
            }
            if !crate::privacy::valid_server(&observation.server)
                || !servers.insert(&observation.server)
                || observation.score > 100
                || observation.areas.iter().any(|(name, score)| {
                    crate::score::Area::from_label(name).is_none() || *score > 100
                })
            {
                anyhow::bail!("invalid or duplicate corpus observation");
            }
        }
        Ok(())
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

    /// Empirical readiness comparisons require known, identical conditions.
    pub fn placement_for(
        &self,
        score: u64,
        profile: Option<&crate::measurement::MeasurementProfile>,
    ) -> Option<Placement> {
        let profile = profile?;
        let scores: Vec<_> = self
            .observations
            .iter()
            .filter(|row| row.measurement_profile.as_ref() == Some(profile))
            .map(|row| row.score)
            .collect();
        if scores.is_empty() {
            return None;
        }
        Some(Placement {
            above: scores.iter().filter(|&&value| value < score).count(),
            tied: scores.iter().filter(|&&value| value == score).count(),
            below: scores.iter().filter(|&&value| value > score).count(),
        })
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

/// Validate v3 metadata before reducing it to the calibration view. This
/// establishes structure and consistency, not artifact authenticity; the
/// collector's verifier checks the original reports and executable hashes.
fn validate_provenance(document: &serde_json::Value) -> anyhow::Result<()> {
    let schema: serde_json::Value =
        serde_json::from_str(include_str!("../docs/mcp-eval.corpus.schema.json"))?;
    if !crate::schema::conforms(&schema, document) {
        anyhow::bail!("invalid corpus provenance");
    }
    let mut population = HashSet::new();
    let mut observed = HashSet::new();
    for entry in document["population"]
        .as_array()
        .expect("validated population")
    {
        let server = entry["server"].as_str().expect("validated server");
        if !population.insert(server) {
            anyhow::bail!("duplicate corpus population target");
        }
        if entry["status"] == "observed" {
            observed.insert(server);
        }
    }
    let observations = document["observations"]
        .as_array()
        .expect("validated observations");
    for observation in observations {
        if let Some(profile) = observation.get("measurement_profile") {
            anyhow::ensure!(
                profile["evaluator_version"] == document["evaluator"]["version"],
                "corpus measurement evaluator mismatch"
            );
        }
    }
    if observed.len() != observations.len() {
        anyhow::bail!("corpus population and observations disagree");
    }
    for observation in observations {
        if !observed.contains(observation["server"].as_str().expect("validated server"))
            || ![
                "successful_calls",
                "tool_errors",
                "rpc_errors",
                "rejected_calls",
                "transport_errors",
            ]
            .iter()
            .any(|key| {
                observation["calls"][key]
                    .as_u64()
                    .is_some_and(|value| value > 0)
            })
        {
            anyhow::bail!("corpus observation lacks observed tool-call evidence");
        }
        let source = &observation["provenance"];
        let declared: HashSet<_> = source["prerequisites"]
            .as_array()
            .expect("validated prerequisites")
            .iter()
            .filter_map(|value| value.as_str().and_then(|name| name.strip_prefix("state:")))
            .collect();
        let mut states = HashSet::new();
        if let Some(checks) = source["state_checks"].as_array() {
            for check in checks {
                if !states.insert(check["name"].as_str().expect("validated state name")) {
                    anyhow::bail!("duplicate corpus state check");
                }
            }
        }
        if states != declared {
            anyhow::bail!("corpus state checks and prerequisites disagree");
        }
    }
    Ok(())
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
            schema: SCHEMA.into(),
            source: "test".into(),
            standard: "mcpeval-standard/1".into(),
            observations: scores
                .iter()
                .enumerate()
                .map(|(index, &score)| Observation {
                    server: format!("server-{index}"),
                    score,
                    areas: BTreeMap::new(),
                    tool_count: None,
                    catalog_tokens: None,
                    measurement_profile: None,
                })
                .collect(),
        }
    }

    #[test]
    fn readiness_placement_excludes_unknown_and_different_profiles() {
        let mut corpus = corpus(&[10, 50, 90]);
        let profile = crate::measurement::MeasurementProfile::current(false, &[]);
        corpus.observations[0].measurement_profile = Some(profile.clone());
        corpus.observations[1].measurement_profile =
            Some(crate::measurement::MeasurementProfile::current(true, &[]));
        assert_eq!(corpus.placement_for(20, None), None);
        assert_eq!(
            corpus.placement_for(20, Some(&profile)),
            Some(Placement {
                above: 1,
                tied: 0,
                below: 0
            })
        );
        let mut changed = profile;
        changed.skip_tools.push("read_other".into());
        assert_eq!(corpus.placement_for(20, Some(&changed)), None);
    }

    fn parse(document: &str) -> anyhow::Result<Corpus> {
        let corpus: Corpus = serde_json::from_str(document)?;
        corpus.validate()?;
        Ok(corpus)
    }

    #[test]
    fn corpus_metadata_matches_shared_contract_cases_and_calibration_requirements() {
        let fixture: serde_json::Value =
            serde_json::from_str(include_str!("../tests/fixtures/corpus-v3.json")).unwrap();
        let scenarios: serde_json::Value = serde_json::from_str(include_str!(
            "../tests/fixtures/corpus-v3-metadata-cases.json"
        ))
        .unwrap();
        let schema: serde_json::Value =
            serde_json::from_str(include_str!("../docs/mcp-eval.corpus.schema.json")).unwrap();
        let home =
            std::env::temp_dir().join(format!("mcpeval-corpus-contract-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&home).unwrap();
        let file = home.join("corpus.json");
        for scenario in scenarios.as_array().unwrap() {
            let mut document = fixture.clone();
            let fields = document.as_object_mut().unwrap();
            fields.extend(scenario["set"].as_object().unwrap().clone());
            for key in scenario["remove"].as_array().unwrap() {
                fields.remove(key.as_str().unwrap());
            }
            assert_eq!(
                crate::schema::conforms(&schema, &document),
                scenario["valid"].as_bool().unwrap(),
                "published schema: {}",
                scenario["name"]
            );
            std::fs::write(&file, document.to_string()).unwrap();
            assert_eq!(
                Corpus::load(&file).is_ok(),
                scenario["calibratable"].as_bool().unwrap(),
                "calibration loader: {}",
                scenario["name"]
            );
        }
        std::fs::remove_dir_all(home).unwrap();
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
    fn a_v2_corpus_names_its_standard_and_carries_area_scores() {
        let corpus = parse(
            r#"{"schema":"mcpeval.readiness-corpus/v2","source":"s","standard":"mcpeval-standard/1",
                "observations":[{"server":"a","score":71,"areas":{"protocol":100,"coverage":40},
                                 "tool_count":9,"catalog_tokens":2688}]}"#,
        )
        .unwrap();
        assert_eq!(corpus.standard, "mcpeval-standard/1");
        let observation = &corpus.observations[0];
        assert_eq!(observation.areas["protocol"], 100);
        assert_eq!(observation.areas["coverage"], 40);
        assert_eq!(observation.tool_count, Some(9));
        assert_eq!(observation.catalog_tokens, Some(2688));

        // A v1 corpus holds manifest pass rates, not readiness: refused.
        assert!(parse(
            r#"{"schema":"mcpeval.readiness-corpus/v1","source":"s",
                "observations":[{"server":"a","score":100}]}"#,
        )
        .is_err());
        // The standard is required and named.
        assert!(parse(
            r#"{"schema":"mcpeval.readiness-corpus/v2","source":"s",
                "observations":[{"server":"a","score":100}]}"#,
        )
        .is_err());
        assert!(parse(
            r#"{"schema":"mcpeval.readiness-corpus/v2","source":"s","standard":"",
                "observations":[{"server":"a","score":100}]}"#,
        )
        .is_err());
    }

    #[test]
    fn repository_corpus_loads_as_checked_in() {
        let corpus =
            Corpus::load(&Path::new(env!("CARGO_MANIFEST_DIR")).join("data/readiness-corpus.json"))
                .unwrap();
        // Historical observations must retain the standard that measured them.
        assert_eq!(corpus.standard, "mcpeval-standard/1");
        assert_ne!(corpus.standard, crate::score::STANDARD);
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
