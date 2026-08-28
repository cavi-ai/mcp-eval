use anyhow::{bail, Context};
use rusqlite::{Connection, OptionalExtension};
use serde::Serialize;
use serde_json::Value;
use std::fmt::Write;
use std::path::Path;

#[derive(Clone, Copy, Debug)]
pub enum ReportFormat {
    Agent,
    Md,
    Json,
}

#[derive(Debug, Serialize)]
pub struct Finding {
    finding_id: String,
    state: String,
    probe_id: Option<String>,
    consecutive_passes: u64,
    server: String,
    tool: Option<String>,
    err_code: Option<Value>,
    err_template_id: Option<String>,
    failures: u64,
    calls: u64,
    sessions: u64,
    last_seen: String,
    cost: f64,
    blast: u64,
    rate: f64,
    confidence: f64,
    recency: f64,
    score: f64,
    threshold: f64,
    severity: String,
    repro: Option<Value>,
}

pub fn render(root: &Path, format: ReportFormat) -> anyhow::Result<String> {
    let findings = load_findings(root)?;
    match format {
        ReportFormat::Json => Ok(serde_json::to_string_pretty(&findings)? + "\n"),
        ReportFormat::Agent => render_agent(&findings),
        ReportFormat::Md => render_markdown(&findings),
    }
}

fn open_index(root: &Path) -> anyhow::Result<Connection> {
    let path = root.join("index.db");
    if !path.is_file() {
        bail!("findings are unavailable; run `mcpeval index` and `mcpeval promote` first");
    }
    let db = Connection::open(path)?;
    let findings_table: Option<String> = db
        .query_row(
            "SELECT name FROM sqlite_master WHERE type='table' AND name='findings'",
            [],
            |row| row.get(0),
        )
        .optional()?;
    if findings_table.is_none() {
        bail!("findings are unavailable; run `mcpeval promote` first");
    }
    Ok(db)
}

/// Sanitized finding rows for consumers that serve them programmatically
/// (`mcpeval serve`); identical content to `findings --format json`.
pub fn load_findings(root: &Path) -> anyhow::Result<Vec<Finding>> {
    let db = open_index(root)?;
    let mut statement = db.prepare(
        "SELECT i.finding_id,l.state,l.probe_id,l.consecutive_passes,
                i.server,i.tool,i.err_code,i.err_template_id,i.failures,i.calls,
                i.sessions,i.last_seen,i.cost,i.blast,i.rate,i.confidence,i.recency,
                i.score,i.threshold,i.severity,i.args
         FROM findings f JOIN issues i ON i.id=f.issue_id
         JOIN finding_lifecycle l ON l.finding_id=i.finding_id
         ORDER BY i.score DESC, i.server, COALESCE(i.tool,''),
                  COALESCE(i.err_code,''), COALESCE(i.err_template_id,'')",
    )?;
    let rows = statement.query_map([], |row| {
        let err_code: Option<String> = row.get(6)?;
        let args: Option<String> = row.get(20)?;
        Ok(Finding {
            finding_id: row.get(0)?,
            state: row.get(1)?,
            probe_id: row.get(2)?,
            consecutive_passes: row.get(3)?,
            server: row.get(4)?,
            tool: row.get(5)?,
            err_code: err_code.and_then(|value| serde_json::from_str(&value).ok()),
            err_template_id: row.get(7)?,
            failures: row.get(8)?,
            calls: row.get(9)?,
            sessions: row.get(10)?,
            last_seen: row.get(11)?,
            cost: row.get(12)?,
            blast: row.get(13)?,
            rate: row.get(14)?,
            confidence: row.get(15)?,
            recency: row.get(16)?,
            score: row.get(17)?,
            threshold: row.get(18)?,
            severity: row.get(19)?,
            repro: args.and_then(|value| serde_json::from_str(&value).ok()),
        })
    })?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

fn tool_name(finding: &Finding) -> &str {
    finding.tool.as_deref().unwrap_or("unlisted")
}

fn render_agent(findings: &[Finding]) -> anyhow::Result<String> {
    let mut output = String::new();
    for finding in findings {
        writeln!(
            output,
            "finding {} {}/{} state={} probe={} consecutive_passes={} severity={} score={:.6} threshold={:.6} rate={:.6} confidence={:.6} recency={:.6} failures={}/{} sessions={} cost={:.1} blast={}",
            finding.finding_id,
            finding.server,
            tool_name(finding),
            finding.state,
            finding.probe_id.as_deref().unwrap_or("none"),
            finding.consecutive_passes,
            finding.severity,
            finding.score,
            finding.threshold,
            finding.rate,
            finding.confidence,
            finding.recency,
            finding.failures,
            finding.calls,
            finding.sessions,
            finding.cost,
            finding.blast,
        )?;
        writeln!(
            output,
            "  cause code={} template_id={} last_seen={}",
            finding
                .err_code
                .as_ref()
                .map(Value::to_string)
                .unwrap_or_else(|| "null".into()),
            finding.err_template_id.as_deref().unwrap_or("none"),
            finding.last_seen,
        )?;
        if let Some(repro) = &finding.repro {
            writeln!(output, "  repro={repro}")?;
        }
    }
    Ok(output)
}

fn render_markdown(findings: &[Finding]) -> anyhow::Result<String> {
    let mut output = String::from("# MCP Evaluation Findings\n\n");
    for finding in findings {
        writeln!(output, "## {} / {}\n", finding.server, tool_name(finding))?;
        writeln!(
            output,
            "- Finding ID: {}\n- State: {}\n- Probe: {}\n- Consecutive passes: {}\n- Severity: {}\n- Score: {:.6} (threshold {:.6})\n- Rate: {:.6}\n- Confidence: {:.6}\n- Recency: {:.6}\n- Error code: {}\n- Template ID: {}\n- Evidence: {} failures / {} calls across {} sessions\n- Agent cost: median {:.1} turns; blast radius {} tools\n- Last seen: {}",
            finding.finding_id,
            finding.state,
            finding.probe_id.as_deref().unwrap_or("none"),
            finding.consecutive_passes,
            finding.severity,
            finding.score,
            finding.threshold,
            finding.rate,
            finding.confidence,
            finding.recency,
            finding
                .err_code
                .as_ref()
                .map(Value::to_string)
                .unwrap_or_else(|| "null".into()),
            finding.err_template_id.as_deref().unwrap_or("none"),
            finding.failures,
            finding.calls,
            finding.sessions,
            finding.cost,
            finding.blast,
            finding.last_seen,
        )?;
        if let Some(repro) = &finding.repro {
            writeln!(output, "- Shape-level repro: `{repro}`")?;
        }
        output.push('\n');
    }
    Ok(output)
}

/// Pull-request-ready rendering of a probe report: verdict table, category
/// breakdown, readiness score, and a static badge URL. Contains only the
/// share-safe fields already present in the JSON report.
pub fn render_probe_markdown(
    server: &str,
    report: &crate::probe::ProbeReport,
    corpus: Option<&crate::corpus::Corpus>,
) -> String {
    let readiness = crate::score::readiness(report);
    let mut out = String::new();
    out.push_str(&format!("## mcp-eval report — {server}\n\n"));
    let calibration = corpus.map(|corpus| {
        format!(
            " — beats {}% of observed servers (corpus median {})",
            corpus.percentile(readiness.overall),
            corpus.median()
        )
    });
    match calibration {
        Some(context) => out.push_str(&format!(
            "**Readiness: {}/100{context}** ![mcpeval]({})\n\n",
            readiness.overall,
            crate::score::badge_url(readiness.overall)
        )),
        None => out.push_str(&format!(
            "**Readiness: {}/100** ![mcpeval]({})\n\n",
            readiness.overall,
            crate::score::badge_url(readiness.overall)
        )),
    };
    if !readiness.categories.is_empty() {
        out.push_str("| Category | Passed |\n| --- | --- |\n");
        for category in &readiness.categories {
            writeln!(
                out,
                "| {} | {}/{} |",
                category.name, category.passed, category.total
            )
            .ok();
        }
        out.push('\n');
    }
    out.push_str("| Case | Probe | Result | Attempts | First failure | Reason |\n");
    out.push_str("| --- | --- | --- | --- | --- | --- |\n");
    for case in &report.cases {
        let (result, first_failure, reason) = match case.reason {
            None => ("pass".to_string(), "—".to_string(), "—".to_string()),
            Some(reason) => (
                "fail".to_string(),
                case.first_failure
                    .map(|value| value.to_string())
                    .unwrap_or_default(),
                reason.as_str().to_string(),
            ),
        };
        writeln!(
            out,
            "| {} | {} | {} | {} | {} | {} |",
            case.id,
            case.probe.as_str(),
            result,
            case.attempts,
            first_failure,
            reason
        )
        .ok();
    }
    let failures: Vec<(&str, crate::probe::FailureReason)> = report
        .cases
        .iter()
        .filter_map(|case| case.reason.map(|reason| (case.id.as_str(), reason)))
        .collect();
    if !failures.is_empty() {
        out.push_str("\n### Remediation\n\n");
        for (case_id, reason) in failures {
            writeln!(
                out,
                "- **`{}` (`{}`):** {}",
                case_id,
                reason.as_str(),
                crate::remediation::hint(reason)
            )
            .ok();
        }
        out.push('\n');
    }
    let mut measurements = String::new();
    for case in &report.cases {
        if let Some(usage) = &case.token_usage {
            writeln!(
                measurements,
                "\n- `{}` catalog cost estimate: {} tokens across {} tools",
                case.id,
                usage.total_tokens,
                usage.per_tool.len()
            )
            .ok();
        }
    }
    out.push_str(&measurements);
    out.push_str(
        "\n*Deterministic battery (`mcpeval probe`); arguments, responses, and error prose are never recorded.*\n",
    );
    out
}

/// Write one GitHub-issue markdown file per finding into `dir`.
/// Open findings only, unless `include_closed` is set. Files contain no
/// captured private content beyond what `findings --format md` already shows.
pub fn export_issues(
    root: &Path,
    dir: &Path,
    include_closed: bool,
    force: bool,
) -> anyhow::Result<usize> {
    let findings = load_findings(root)?;
    let selected: Vec<&Finding> = findings
        .iter()
        .filter(|finding| include_closed || finding.state == "open")
        .collect();
    if selected.is_empty() {
        bail!("no exportable findings; run `mcpeval index` and `mcpeval promote` first");
    }
    if dir.exists() {
        let occupied = std::fs::read_dir(dir)?
            .filter_map(Result::ok)
            .next()
            .is_some();
        if occupied && !force {
            bail!(
                "{} is not empty; pass --force to replace its files",
                dir.display()
            );
        }
    } else {
        std::fs::create_dir_all(dir).context("creating export directory")?;
    }
    for finding in &selected {
        let path = dir.join(format!("{}.md", finding.finding_id));
        std::fs::write(&path, issue_markdown(finding)).context("writing issue file")?;
    }
    Ok(selected.len())
}

fn issue_markdown(finding: &Finding) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "# mcpeval finding {}: recurring failures on {}/{}\n\n",
        finding.finding_id,
        finding.server,
        tool_name(finding)
    ));
    out.push_str(&format!(
        "**Suggested labels:** `mcpeval`, `{}`\n\n",
        finding.severity
    ));
    out.push_str(&format!(
        "- State: {}\n- Severity: {}\n- Probe coverage: {}\n- Evidence: {} failures / {} calls across {} sessions\n- Agent cost: median {:.1} turns; blast radius {} tools\n- Observed failure rate: {:.6} (Wilson 95% lower bound; threshold {:.6})\n- Last seen: {}\n",
        finding.state,
        finding.severity,
        finding.probe_id.as_deref().unwrap_or("none — run `mcpeval generate` to attach one"),
        finding.failures,
        finding.calls,
        finding.sessions,
        finding.cost,
        finding.blast,
        finding.rate,
        finding.threshold,
        finding.last_seen,
    ));
    if let Some(repro) = &finding.repro {
        out.push_str(&format!("- Shape-level repro: `{repro}`\n"));
    }
    out.push_str("\n## Suggested next steps\n\n1. Reproduce with the shape above against the failing tool.\n2. Attach a deterministic probe:\n   ```sh\n   mcpeval generate --finding ");
    out.push_str(&finding.finding_id);
    out.push_str(" --confirm-read-only --output generated.manifest.json\n   ```\n3. After fixing, verify until it closes (three consecutive green runs):\n   ```sh\n   mcpeval verify --finding ");
    out.push_str(&finding.finding_id);
    out.push_str(" --case <case-id> --manifest mcp-eval.manifest.json -- <server command>\n   ```\n\n*Generated by `mcpeval export-issues`; content-free metadata only.*\n");
    out
}
