//! Finding verification shared by the CLI and native MCP surface.
//!
//! Validate the selected definition before launch, execute that same loaded
//! definition, and record only evaluated outcomes in the durable journal.

use anyhow::Context;

use crate::lifecycle::Status;
use crate::manifest::Manifest;
use crate::probe::{FailureReason, ProbeOptions, ProbeReport};
use crate::store::Store;

pub struct VerifyOptions {
    pub finding: String,
    pub case: String,
    pub manifest: Manifest,
    pub allow_mutation: bool,
    pub command: Vec<String>,
    pub http_url: Option<String>,
    pub allow_remote_http: bool,
}

pub struct Verification {
    pub server: String,
    pub report: ProbeReport,
    /// None when transport failure prevented evaluation; no credit was recorded.
    pub status: Option<Status>,
    pub reason: Option<FailureReason>,
}

pub fn run(options: VerifyOptions, store: &mut Store) -> anyhow::Result<Verification> {
    options.manifest.validate().map_err(crate::exit::usage)?;
    let selected = options
        .manifest
        .probes
        .iter()
        .find(|candidate| candidate.id() == options.case)
        .ok_or_else(|| {
            crate::exit::usage(anyhow::anyhow!(
                "probe case is not declared in the manifest"
            ))
        })?;
    let tool = selected.tool().ok_or_else(|| {
        crate::exit::usage(anyhow::anyhow!(
            "finding verification requires a tool probe"
        ))
    })?;
    let server = crate::lifecycle::prepare(store.root(), &options.finding, selected.id(), tool)?;
    let definition_id = crate::lifecycle::definition_id(
        store.root(),
        &options.manifest,
        selected,
        serde_json::json!({"command": options.command, "url": options.http_url,
            "allow_remote_http": options.allow_remote_http}),
    )?;
    let run_id = uuid::Uuid::new_v4().to_string();
    let report = crate::probe::run(
        ProbeOptions {
            server: server.clone(),
            manifest_path: std::path::PathBuf::new(),
            manifest_inline: Some(serde_json::to_string(&options.manifest)?),
            selected_probe: None,
            selected_case: Some(options.case.clone()),
            allow_mutation: options.allow_mutation,
            command: options.command,
            http_url: options.http_url,
            allow_remote_http: options.allow_remote_http,
            standard: false,
            confirm_read_only: false,
            skip_tools: Vec::new(),
        },
        store,
    )?;
    let reason = report
        .cases
        .first()
        .context("verification produced no case outcome")?
        .reason;
    let status = if reason.is_some_and(|reason| reason.is_transport()) {
        None
    } else {
        Some(crate::lifecycle::record(
            store.root(),
            &options.finding,
            &options.case,
            &definition_id,
            &run_id,
            reason.is_none(),
            chrono::Utc::now(),
        )?)
    };
    Ok(Verification {
        server,
        report,
        status,
        reason,
    })
}
