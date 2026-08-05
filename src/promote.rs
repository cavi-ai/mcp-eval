use anyhow::bail;
use chrono::{DateTime, SecondsFormat, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use std::path::Path;

const WILSON_Z: f64 = 1.959_963_984_540_054;

#[derive(Clone, Copy, Debug)]
pub struct ScoreInput {
    pub failures: u64,
    pub calls: u64,
    pub last_seen: DateTime<Utc>,
    pub now: DateTime<Utc>,
    pub cost: f64,
    pub blast: u64,
}

#[derive(Clone, Copy, Debug)]
pub struct ScoreParts {
    pub rate: f64,
    pub confidence: f64,
    pub recency: f64,
    pub cost: f64,
    pub blast: u64,
    pub score: f64,
}

pub fn wilson_lower_bound(failures: u64, calls: u64) -> anyhow::Result<f64> {
    if calls == 0 {
        bail!("calls must be greater than zero");
    }
    if failures > calls {
        bail!("failures cannot exceed calls");
    }
    let n = calls as f64;
    let rate = failures as f64 / n;
    let z2 = WILSON_Z * WILSON_Z;
    let centre = rate + z2 / (2.0 * n);
    let margin = WILSON_Z * ((rate * (1.0 - rate) + z2 / (4.0 * n)) / n).sqrt();
    Ok((centre - margin) / (1.0 + z2 / n))
}

pub fn score(input: ScoreInput) -> anyhow::Result<ScoreParts> {
    if !input.cost.is_finite() || input.cost < 0.0 {
        bail!("cost must be finite and non-negative");
    }
    if input.blast == 0 {
        bail!("blast radius must be greater than zero");
    }
    let rate = input.failures as f64 / input.calls as f64;
    let confidence = wilson_lower_bound(input.failures, input.calls)?;
    let age_millis = (input.now - input.last_seen).num_milliseconds().max(0) as f64;
    let age_days = age_millis / 86_400_000.0;
    let recency = 0.5_f64.powf(age_days / 14.0);
    let score =
        confidence * recency * (1.0 + input.cost).log2() * (1.0 + (input.blast as f64).log2());
    Ok(ScoreParts {
        rate,
        confidence,
        recency,
        cost: input.cost,
        blast: input.blast,
        score,
    })
}

#[derive(Deserialize)]
struct SeedRow {
    class: String,
    failures: u64,
    calls: u64,
    age_days: f64,
    cost: f64,
    blast: u64,
}

pub fn calibrate_seed() -> anyhow::Result<f64> {
    let rows: Vec<SeedRow> =
        serde_json::from_str(include_str!("../tests/fixtures/phase2-seed.json"))?;
    let now = DateTime::<Utc>::UNIX_EPOCH;
    let mut lowest_blocker = f64::INFINITY;
    let mut highest_annoyance = f64::NEG_INFINITY;
    for row in rows {
        if !row.age_days.is_finite() || row.age_days < 0.0 {
            bail!("seed age_days must be finite and non-negative");
        }
        let last_seen = now - chrono::Duration::milliseconds((row.age_days * 86_400_000.0) as i64);
        let value = score(ScoreInput {
            failures: row.failures,
            calls: row.calls,
            last_seen,
            now,
            cost: row.cost,
            blast: row.blast,
        })?
        .score;
        match row.class.as_str() {
            "blocker" => lowest_blocker = lowest_blocker.min(value),
            "annoyance" => highest_annoyance = highest_annoyance.max(value),
            other => bail!("unknown seed class {other:?}"),
        }
    }
    if !lowest_blocker.is_finite() || !highest_annoyance.is_finite() {
        bail!("seed corpus must contain blockers and annoyances");
    }
    if highest_annoyance >= lowest_blocker {
        bail!("seed blocker and annoyance score bands overlap");
    }
    Ok((highest_annoyance + lowest_blocker) / 2.0)
}

#[derive(Clone, Copy, Debug)]
pub struct PromotionConfig {
    pub threshold: f64,
    pub now: DateTime<Utc>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PromotionStats {
    pub issues: usize,
    pub findings: usize,
}

#[derive(Deserialize)]
struct FileConfig {
    promotion_threshold: Option<f64>,
}

pub fn resolve_threshold(root: &Path, explicit: Option<f64>) -> anyhow::Result<f64> {
    let threshold = if let Some(value) = explicit {
        value
    } else {
        let path = root.join("config.json");
        match std::fs::read(&path) {
            Ok(body) => serde_json::from_slice::<FileConfig>(&body)?
                .promotion_threshold
                .unwrap_or(calibrate_seed()?),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => calibrate_seed()?,
            Err(error) => return Err(error.into()),
        }
    };
    if !threshold.is_finite() || threshold < 0.0 {
        bail!("promotion threshold must be finite and non-negative");
    }
    Ok(threshold)
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct IssueKey {
    server: String,
    tool: Option<String>,
    err_code: Option<String>,
    err_template_id: Option<String>,
}

#[derive(Debug)]
struct Failure {
    id: i64,
    session: String,
    ts: String,
    args: Option<String>,
}

const DERIVED_SCHEMA: &str = "
DROP TABLE IF EXISTS findings;
DROP TABLE IF EXISTS issues;
CREATE TABLE issues (
  id INTEGER PRIMARY KEY,
  server TEXT NOT NULL, tool TEXT, err_code TEXT, err_template_id TEXT,
  failures INTEGER NOT NULL, calls INTEGER NOT NULL, sessions INTEGER NOT NULL,
  last_seen TEXT NOT NULL, cost REAL NOT NULL, blast INTEGER NOT NULL,
  rate REAL NOT NULL, confidence REAL NOT NULL, recency REAL NOT NULL,
  score REAL NOT NULL, threshold REAL NOT NULL, severity TEXT NOT NULL,
  args TEXT
);
CREATE INDEX issues_score ON issues(score DESC);
CREATE TABLE findings (issue_id INTEGER PRIMARY KEY REFERENCES issues(id));
";

pub fn promote(root: &Path, config: PromotionConfig) -> anyhow::Result<PromotionStats> {
    if !config.threshold.is_finite() || config.threshold < 0.0 {
        bail!("promotion threshold must be finite and non-negative");
    }
    let mut db = Connection::open(root.join("index.db"))?;
    db.pragma_update(None, "foreign_keys", "ON")?;
    ensure_index_schema(&db)?;

    let mut grouped: HashMap<IssueKey, Vec<Failure>> = HashMap::new();
    {
        let mut statement = db.prepare(
            "SELECT id, session, ts, server, tool, err_code, err_template_id, args
             FROM calls WHERE outcome='error' AND method='tools/call'",
        )?;
        let rows = statement.query_map([], |row| {
            Ok((
                IssueKey {
                    server: row.get(3)?,
                    tool: row.get(4)?,
                    err_code: row.get(5)?,
                    err_template_id: row.get(6)?,
                },
                Failure {
                    id: row.get(0)?,
                    session: row.get(1)?,
                    ts: row.get(2)?,
                    args: row.get(7)?,
                },
            ))
        })?;
        for row in rows {
            let (key, failure) = row?;
            grouped.entry(key).or_default().push(failure);
        }
    }

    let transaction = db.transaction()?;
    transaction.execute_batch(DERIVED_SCHEMA)?;
    let mut findings = 0usize;
    for (key, failures) in grouped {
        let calls: u64 = transaction.query_row(
            "SELECT COUNT(*) FROM calls
             WHERE method='tools/call' AND server=?1 AND tool IS ?2",
            params![key.server, key.tool],
            |row| row.get(0),
        )?;
        let sessions = failures
            .iter()
            .map(|failure| failure.session.as_str())
            .collect::<HashSet<_>>()
            .len() as u64;
        let parsed_timestamps = failures
            .iter()
            .map(|failure| {
                DateTime::parse_from_rfc3339(&failure.ts)
                    .map(|value| value.with_timezone(&Utc))
                    .map_err(|error| {
                        anyhow::anyhow!("invalid indexed timestamp {:?}: {error}", failure.ts)
                    })
            })
            .collect::<anyhow::Result<Vec<_>>>()?;
        let parsed_last_seen = *parsed_timestamps
            .iter()
            .max()
            .expect("an issue has at least one failure");
        let last_seen = parsed_last_seen.to_rfc3339_opts(SecondsFormat::AutoSi, true);

        let mut costs = Vec::with_capacity(failures.len());
        let mut tools = HashSet::new();
        if let Some(tool) = key.tool.as_ref() {
            tools.insert(tool.clone());
        }
        let mut uplift = false;
        for failure in &failures {
            let real_neighbours: u64 = transaction.query_row(
                "SELECT COUNT(DISTINCT c.id) FROM windows w
                 JOIN calls c ON c.id=w.neighbour_id
                 WHERE w.failure_id=?1 AND c.kind='real'",
                [failure.id],
                |row| row.get(0),
            )?;
            costs.push(1.0 + real_neighbours as f64);
            let mut tool_statement = transaction.prepare(
                "SELECT DISTINCT c.tool FROM windows w
                 JOIN calls c ON c.id=w.neighbour_id
                 WHERE w.failure_id=?1 AND c.tool IS NOT NULL",
            )?;
            for tool in tool_statement.query_map([failure.id], |row| row.get::<_, String>(0))? {
                tools.insert(tool?);
            }
            uplift |= transaction
                .query_row(
                    "SELECT 1 FROM calls c JOIN annotations a
                     ON a.session=c.session AND a.seq=c.seq
                     WHERE c.id=?1 AND a.kind IN ('false-success','blocked-optimal-path')
                     LIMIT 1",
                    [failure.id],
                    |_| Ok(true),
                )
                .optional()?
                .unwrap_or(false);
        }
        costs.sort_by(f64::total_cmp);
        let cost = if costs.len() % 2 == 1 {
            costs[costs.len() / 2]
        } else {
            (costs[costs.len() / 2 - 1] + costs[costs.len() / 2]) / 2.0
        };
        let blast = tools.len().max(1) as u64;
        let parts = score(ScoreInput {
            failures: failures.len() as u64,
            calls,
            last_seen: parsed_last_seen,
            now: config.now,
            cost,
            blast,
        })?;
        let mut severity_rank = if parts.score >= config.threshold * 4.0 {
            2
        } else if parts.score >= config.threshold * 2.0 {
            1
        } else {
            0
        };
        if uplift {
            severity_rank = (severity_rank + 1).min(2);
        }
        let severity = ["low", "medium", "high"][severity_rank];
        let args = failures.iter().find_map(|failure| failure.args.as_deref());
        transaction.execute(
            "INSERT INTO issues
             (server,tool,err_code,err_template_id,failures,calls,sessions,last_seen,
              cost,blast,rate,confidence,recency,score,threshold,severity,args)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17)",
            params![
                key.server,
                key.tool,
                key.err_code,
                key.err_template_id,
                failures.len() as u64,
                calls,
                sessions,
                last_seen,
                cost,
                blast,
                parts.rate,
                parts.confidence,
                parts.recency,
                parts.score,
                config.threshold,
                severity,
                args,
            ],
        )?;
        if sessions >= 2 && parts.score >= config.threshold {
            transaction.execute(
                "INSERT INTO findings(issue_id) VALUES (?1)",
                [transaction.last_insert_rowid()],
            )?;
            findings += 1;
        }
    }
    let issues: usize =
        transaction.query_row("SELECT COUNT(*) FROM issues", [], |row| row.get(0))?;
    transaction.commit()?;
    Ok(PromotionStats { issues, findings })
}

fn ensure_index_schema(db: &Connection) -> anyhow::Result<()> {
    let calls: Option<String> = db
        .query_row(
            "SELECT name FROM sqlite_master WHERE type='table' AND name='calls'",
            [],
            |row| row.get(0),
        )
        .optional()?;
    if calls.is_none() {
        bail!("index.db is missing the calls table; run `mcpeval index` first");
    }
    Ok(())
}
