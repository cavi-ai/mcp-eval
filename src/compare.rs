//! Run one manifest against several Streamable HTTP endpoints and render a
//! side-by-side diff. Comparison is read-oriented: it reports verdicts and
//! scores but never exits non-zero for probe failures — gating stays the
//! job of `mcpeval probe`.

use std::path::PathBuf;

use anyhow::bail;

use crate::probe::{self, ProbeOptions, ProbeReport};
use crate::store::Store;

pub struct CompareOptions {
    pub server: String,
    pub manifest_path: PathBuf,
    /// Ordered label=url pairs; labels were validated by the CLI layer.
    pub endpoints: Vec<(String, String)>,
    pub allow_mutation: bool,
    pub allow_remote_http: bool,
}

#[derive(Clone, Copy)]
pub enum CompareFormat {
    Text,
    Markdown,
    Json,
}

pub fn run(options: CompareOptions, format: CompareFormat) -> anyhow::Result<String> {
    if !crate::privacy::valid_server(&options.server) {
        bail!("server label is invalid");
    }
    if options.endpoints.len() < 2 {
        bail!("comparison needs at least two --endpoint entries");
    }
    let mut store = Store::open(None)?;
    let mut results = Vec::with_capacity(options.endpoints.len());
    for (label, url) in &options.endpoints {
        let report = probe::run(
            ProbeOptions {
                server: options.server.clone(),
                manifest_path: options.manifest_path.clone(),
                manifest_inline: None,
                selected_probe: None,
                selected_case: None,
                allow_mutation: options.allow_mutation,
                command: Vec::new(),
                http_url: Some(url.clone()),
                allow_remote_http: options.allow_remote_http,
            },
            &mut store,
        )
        .map_err(|error| anyhow::anyhow!("endpoint {label}: {error}"))?;
        results.push((label.clone(), report));
    }
    Ok(match format {
        CompareFormat::Text => render_text(&results),
        CompareFormat::Markdown => render_markdown(&options.server, &results),
        CompareFormat::Json => render_json(&options.server, &results),
    })
}

fn cell(report: &ProbeReport, index: usize) -> String {
    let case = &report.cases[index];
    match case.reason {
        None => "pass".into(),
        Some(reason) => format!("fail({})", reason.as_str()),
    }
}

fn render_text(results: &[(String, ProbeReport)]) -> String {
    let mut width = "readiness".len();
    for (label, _) in results {
        width = width.max(label.len());
    }
    let mut out = String::new();
    let header = format!(
        "{:<width$}  {}",
        "case",
        results
            .iter()
            .map(|(l, _)| l.as_str())
            .collect::<Vec<_>>()
            .join("  "),
        width = width
    );
    out.push_str(header.trim_end());
    out.push('\n');
    let case_count = results.first().map(|(_, r)| r.cases.len()).unwrap_or(0);
    for index in 0..case_count {
        let id = &results[0].1.cases[index].id;
        let row = results
            .iter()
            .map(|(_, report)| cell(report, index))
            .collect::<Vec<_>>()
            .join("  ");
        out.push_str(&format!("{:<width$}  {}\n", id, row, width = width));
    }
    let scores = results
        .iter()
        .map(|(_, report)| format!("{}/100", crate::score::readiness(report).overall))
        .collect::<Vec<_>>()
        .join("  ");
    out.push_str(&format!(
        "{:<width$}  {}\n",
        "readiness",
        scores,
        width = width
    ));
    out
}

fn render_markdown(server: &str, results: &[(String, ProbeReport)]) -> String {
    let mut out = String::new();
    out.push_str(&format!("# mcp-eval comparison — {server}\n\n"));
    let headers = results
        .iter()
        .map(|(label, _)| label.as_str())
        .collect::<Vec<_>>()
        .join(" | ");
    out.push_str(&format!("| Case | Probe | {headers} |\n"));
    let rules = std::iter::repeat_n("---", results.len() + 2)
        .collect::<Vec<_>>()
        .join(" | ");
    out.push_str(&format!("| {rules} |\n"));
    let case_count = results.first().map(|(_, r)| r.cases.len()).unwrap_or(0);
    for index in 0..case_count {
        let first = &results[0].1.cases[index];
        let cells = results
            .iter()
            .map(|(_, report)| match report.cases[index].reason {
                None => "pass".to_string(),
                Some(reason) => format!("fail (`{}`)", reason.as_str()),
            })
            .collect::<Vec<_>>()
            .join(" | ");
        out.push_str(&format!(
            "| {} | {} | {} |\n",
            first.id,
            first.probe.as_str(),
            cells
        ));
    }
    let score_cells = results
        .iter()
        .map(|(_, report)| format!("{}/100", crate::score::readiness(report).overall))
        .collect::<Vec<_>>()
        .join(" | ");
    out.push_str(&format!("| **Readiness** | | {score_cells} |\n\n"));
    out.push_str("*Deterministic battery; no payloads or error prose are included.*\n");
    out
}

fn render_json(server: &str, results: &[(String, ProbeReport)]) -> String {
    let documents: Vec<serde_json::Value> = results
        .iter()
        .map(|(label, report)| {
            serde_json::json!({
                "endpoint": label,
                "report": report.to_json(server),
            })
        })
        .collect();
    serde_json::to_string_pretty(&documents).unwrap_or_else(|_| "[]".into()) + "\n"
}
