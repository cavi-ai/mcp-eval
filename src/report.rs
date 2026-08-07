use anyhow::bail;
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
struct Finding {
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
    let findings = rows.collect::<Result<Vec<_>, _>>()?;
    match format {
        ReportFormat::Json => Ok(serde_json::to_string_pretty(&findings)? + "\n"),
        ReportFormat::Agent => render_agent(&findings),
        ReportFormat::Md => render_markdown(&findings),
    }
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
