//! Readiness floors: a run fails when its readiness, or one area of it, is
//! under a declared minimum. Latency is the one input that depends on the
//! machine a run is on, so floors can compare the scores with every latency
//! check at full marks instead.

use anyhow::{bail, Context};
use serde_json::{json, Value};

use crate::score::{Area, Readiness};

/// Declared minimums for one run.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Floors {
    pub min_score: Option<u64>,
    /// One floor per area, in declaration order.
    pub min_areas: Vec<(Area, u64)>,
    pub exclude_latency: bool,
}

/// A score under its floor.
#[derive(Clone, Debug, PartialEq)]
pub struct Shortfall {
    /// `score`, or an area label.
    pub subject: String,
    pub observed: u64,
    pub floor: u64,
}

/// Floors and their outcome.
#[derive(Clone, Debug, PartialEq)]
pub struct FloorGate {
    pub floors: Floors,
    /// None when the run measured no readiness to compare.
    pub shortfalls: Option<Vec<Shortfall>>,
}

/// Parse an `AREA=SCORE` floor.
pub fn parse_area_floor(value: &str) -> Result<(Area, u64), String> {
    let areas = || {
        Area::ALL
            .iter()
            .map(|area| area.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    };
    let (label, score) = value
        .split_once('=')
        .ok_or_else(|| format!("expected AREA=SCORE, where AREA is one of {}", areas()))?;
    let area = Area::from_label(label)
        .ok_or_else(|| format!("unknown area `{label}`; expected one of {}", areas()))?;
    let score = score
        .parse::<u64>()
        .ok()
        .filter(|score| *score <= 100)
        .ok_or_else(|| format!("`{score}` is not a score from 0 to 100"))?;
    Ok((area, score))
}

impl Floors {
    pub fn is_empty(&self) -> bool {
        self.min_score.is_none() && self.min_areas.is_empty()
    }

    /// Reject floors that cannot be evaluated as declared.
    pub fn validate(&self) -> anyhow::Result<()> {
        if self.exclude_latency && self.is_empty() {
            bail!(
                "--exclude-latency applies to a readiness floor; add --min-readiness or --min-area"
            );
        }
        for (index, (area, _)) in self.min_areas.iter().enumerate() {
            if self.min_areas[..index].iter().any(|(seen, _)| seen == area) {
                bail!("--min-area {} is declared twice", area.as_str());
            }
        }
        Ok(())
    }

    /// Compare `readiness` with every floor.
    pub fn evaluate(&self, readiness: Option<&Readiness>) -> FloorGate {
        FloorGate {
            floors: self.clone(),
            shortfalls: readiness.and_then(|readiness| self.shortfalls(readiness)),
        }
    }

    fn shortfalls(&self, readiness: &Readiness) -> Option<Vec<Shortfall>> {
        let overall = if self.exclude_latency {
            readiness.score_without_latency?
        } else {
            readiness.score
        };
        let mut shortfalls = Vec::new();
        if let Some(floor) = self.min_score.filter(|floor| overall < *floor) {
            shortfalls.push(Shortfall {
                subject: "score".into(),
                observed: overall,
                floor,
            });
        }
        for (area, floor) in &self.min_areas {
            let scored = readiness.areas.iter().find(|scored| scored.area == *area)?;
            let observed = if self.exclude_latency && *area == Area::Reliability {
                scored
                    .measurements
                    .get("score_without_latency")
                    .and_then(Value::as_u64)?
            } else {
                scored.score
            };
            if observed < *floor {
                shortfalls.push(Shortfall {
                    subject: area.as_str().into(),
                    observed,
                    floor: *floor,
                });
            }
        }
        Some(shortfalls)
    }
}

impl FloorGate {
    /// None when there was no readiness to compare.
    pub fn passed(&self) -> Option<bool> {
        self.shortfalls.as_ref().map(Vec::is_empty)
    }

    pub fn to_json(&self) -> Value {
        json!({
            "min_score": self.floors.min_score,
            "min_areas": self
                .floors
                .min_areas
                .iter()
                .map(|(area, floor)| (area.as_str().to_owned(), json!(floor)))
                .collect::<serde_json::Map<_, _>>(),
            "exclude_latency": self.floors.exclude_latency,
            "passed": self.passed(),
            "shortfalls": self.shortfalls.as_ref().map(|shortfalls| {
                shortfalls
                    .iter()
                    .map(|shortfall| json!({
                        "subject": shortfall.subject,
                        "observed": shortfall.observed,
                        "floor": shortfall.floor,
                    }))
                    .collect::<Vec<_>>()
            }),
        })
    }

    /// Parse the `readiness_gate` object of an `mcpeval.probe-report/v2`
    /// document.
    pub fn from_json(value: &Value) -> anyhow::Result<Self> {
        let score = |value: &Value, field: &str| -> anyhow::Result<u64> {
            value
                .get(field)
                .and_then(Value::as_u64)
                .filter(|score| *score <= 100)
                .with_context(|| format!("readiness_gate {field} must be 0-100"))
        };
        let min_score = match value.get("min_score") {
            None | Some(Value::Null) => None,
            Some(_) => Some(score(value, "min_score")?),
        };
        let min_areas = value
            .get("min_areas")
            .and_then(Value::as_object)
            .context("readiness_gate is missing min_areas")?
            .iter()
            .map(|(label, floor)| {
                let area = Area::from_label(label)
                    .with_context(|| format!("unknown readiness_gate area {label}"))?;
                let floor = floor
                    .as_u64()
                    .filter(|floor| *floor <= 100)
                    .with_context(|| format!("readiness_gate floor for {label} must be 0-100"))?;
                Ok((area, floor))
            })
            .collect::<anyhow::Result<Vec<_>>>()?;
        let shortfalls = match value.get("shortfalls") {
            None | Some(Value::Null) => None,
            Some(shortfalls) => Some(
                shortfalls
                    .as_array()
                    .context("readiness_gate shortfalls must be an array")?
                    .iter()
                    .map(|shortfall| {
                        let subject = shortfall
                            .get("subject")
                            .and_then(Value::as_str)
                            .context("shortfall is missing a subject")?;
                        if subject != "score" && Area::from_label(subject).is_none() {
                            bail!("unknown shortfall subject {subject}");
                        }
                        Ok(Shortfall {
                            subject: subject.to_owned(),
                            observed: score(shortfall, "observed")?,
                            floor: score(shortfall, "floor")?,
                        })
                    })
                    .collect::<anyhow::Result<Vec<_>>>()?,
            ),
        };
        Ok(Self {
            floors: Floors {
                min_score,
                min_areas,
                exclude_latency: value
                    .get("exclude_latency")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
            },
            shortfalls,
        })
    }

    /// One line: the outcome and every floor, or every shortfall.
    pub fn text(&self) -> String {
        let latency = if self.floors.exclude_latency {
            " (latency excluded)"
        } else {
            ""
        };
        match &self.shortfalls {
            None => "readiness floor not evaluated: readiness was not measured".into(),
            Some(shortfalls) if shortfalls.is_empty() => {
                let floors = self
                    .floors
                    .min_score
                    .map(|floor| format!("score >= {floor}"))
                    .into_iter()
                    .chain(
                        self.floors
                            .min_areas
                            .iter()
                            .map(|(area, floor)| format!("{} >= {floor}", area.as_str())),
                    )
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("readiness floor passed: {floors}{latency}")
            }
            Some(shortfalls) => {
                let failed = shortfalls
                    .iter()
                    .map(|shortfall| {
                        format!(
                            "{} {} < {}",
                            shortfall.subject, shortfall.observed, shortfall.floor
                        )
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("readiness floor failed: {failed}{latency}")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn area_floors_parse_only_known_areas_and_scores() {
        assert_eq!(
            parse_area_floor("error-honesty=90"),
            Ok((Area::ErrorHonesty, 90))
        );
        assert!(parse_area_floor("honesty=90")
            .unwrap_err()
            .contains("unknown area"));
        assert!(parse_area_floor("protocol").is_err());
        assert!(parse_area_floor("protocol=101").is_err());
        assert!(parse_area_floor("protocol=-1").is_err());
    }

    #[test]
    fn latency_exclusion_needs_a_floor_and_areas_are_declared_once() {
        let floors = Floors {
            exclude_latency: true,
            ..Floors::default()
        };
        assert!(floors.validate().is_err());
        let floors = Floors {
            min_areas: vec![(Area::Protocol, 90), (Area::Protocol, 80)],
            ..Floors::default()
        };
        assert!(floors.validate().is_err());
    }

    fn readiness(
        score: u64,
        without: u64,
        reliability: u64,
        reliability_without: u64,
    ) -> Readiness {
        Readiness {
            standard: crate::score::STANDARD.into(),
            score,
            score_without_latency: Some(without),
            attested_read_only: false,
            surface: crate::score::Surface::default(),
            areas: vec![crate::score::AreaScore {
                area: Area::Reliability,
                score: reliability,
                checks: Vec::new(),
                measurements: serde_json::from_value(json!({
                    "score_without_latency": reliability_without
                }))
                .unwrap(),
            }],
            measurement_profile: None,
        }
    }

    #[test]
    fn excluding_latency_compares_the_latency_free_scores() {
        let slow = readiness(80, 90, 60, 100);
        let floors = Floors {
            min_score: Some(85),
            min_areas: vec![(Area::Reliability, 70)],
            exclude_latency: false,
        };
        let gate = floors.evaluate(Some(&slow));
        assert_eq!(gate.passed(), Some(false));
        assert_eq!(
            gate.text(),
            "readiness floor failed: score 80 < 85, reliability 60 < 70"
        );
        let floors = Floors {
            exclude_latency: true,
            ..floors
        };
        let gate = floors.evaluate(Some(&slow));
        assert_eq!(gate.passed(), Some(true));
        assert_eq!(
            gate.text(),
            "readiness floor passed: score >= 85, reliability >= 70 (latency excluded)"
        );
        // A document without the latency-free score cannot be compared.
        let legacy = Readiness {
            score_without_latency: None,
            ..slow
        };
        assert_eq!(floors.evaluate(Some(&legacy)).passed(), None);
        assert_eq!(floors.evaluate(None).passed(), None);
    }

    #[test]
    fn a_gate_round_trips_through_json() {
        let gate = FloorGate {
            floors: Floors {
                min_score: Some(85),
                min_areas: vec![(Area::ErrorHonesty, 90)],
                exclude_latency: true,
            },
            shortfalls: Some(vec![Shortfall {
                subject: "score".into(),
                observed: 76,
                floor: 85,
            }]),
        };
        assert_eq!(FloorGate::from_json(&gate.to_json()).unwrap(), gate);
        assert_eq!(
            gate.text(),
            "readiness floor failed: score 76 < 85 (latency excluded)"
        );
        let unmeasured = FloorGate {
            shortfalls: None,
            ..gate
        };
        assert_eq!(unmeasured.passed(), None);
        assert_eq!(
            FloorGate::from_json(&unmeasured.to_json()).unwrap(),
            unmeasured
        );
    }
}
