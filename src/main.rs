mod cli;

use anyhow::Context;
use clap::Parser;

fn render_probe_text(
    server: &str,
    report: &mcpeval::probe::ProbeReport,
    brief: bool,
    corpus: Option<&mcpeval::corpus::Corpus>,
    price_per_mtok: Option<f64>,
) {
    for case in &report.cases {
        let probe = case.probe.as_str();
        let mut measurements = String::new();
        if let Some(tools) = case.tool_count {
            measurements.push_str(&format!(" tools={tools}"));
        }
        if let Some(bytes) = case.schema_bytes {
            measurements.push_str(&format!(" schema_bytes={bytes}"));
        }
        if let Some(usage) = &case.token_usage {
            measurements.push_str(&format!(" total_tokens={}", usage.total_tokens));
        }
        if let Some(latency_ms) = case.latency_ms {
            measurements.push_str(&format!(" latency_ms={latency_ms}"));
        }
        if let Some(pages) = case.pages {
            measurements.push_str(&format!(" pages={pages}"));
        }
        if case.passed() {
            println!(
                "{} {probe} pass attempts={}{measurements}",
                case.id, case.attempts
            );
        } else {
            let reason = case.reason.expect("failed case has a reason");
            println!(
                "{} {probe} fail attempts={} first_failure={} reason={}{measurements}",
                case.id,
                case.attempts,
                case.first_failure.expect("failed case has a failure index"),
                reason.as_str()
            );
            if !brief {
                println!("  hint: {}", mcpeval::remediation::hint(reason));
            }
        }
    }
    let readiness = mcpeval::score::readiness(report);
    let categories = readiness
        .categories
        .iter()
        .map(|category| format!("{}={}/{}", category.name, category.passed, category.total))
        .collect::<Vec<_>>()
        .join(" ");
    match corpus {
        Some(corpus) => println!(
            "{server} readiness {}/100 (beats {}% of observed servers; corpus median {}) {categories}",
            readiness.overall,
            corpus.percentile(readiness.overall),
            corpus.median()
        ),
        None => println!("{server} readiness {}/100 {categories}", readiness.overall),
    }
    if let (Some(price), Some(usage)) = (price_per_mtok, catalog_tokens(report)) {
        println!("  cost: {}", mcpeval::score::cost_context(usage, price));
    }
}

/// The catalog-wide token measurement from a token-cost case, when the
/// battery included one.
fn catalog_tokens(report: &mcpeval::probe::ProbeReport) -> Option<u64> {
    report
        .cases
        .iter()
        .filter(|case| case.passed())
        .filter_map(|case| case.token_usage.as_ref())
        .map(|usage| usage.total_tokens)
        .max()
}

fn main() -> anyhow::Result<()> {
    let cli = cli::Cli::parse();
    match cli.command {
        cli::Command::Shim { server, cmd } => {
            let code = mcpeval::shim::run(server, cmd)?;
            std::process::exit(code);
        }
        cli::Command::ShimHttp {
            server,
            listen,
            upstream,
            allow_remote_http,
        } => mcpeval::http_proxy::run(server, listen, upstream, allow_remote_http),
        cli::Command::Probe {
            server,
            manifest,
            probe,
            format,
            brief,
            price_per_mtok,
            allow_mutation,
            url,
            allow_remote_http,
            cmd,
        } => {
            let selected_probe = probe.map(|probe| match probe {
                cli::ProbeSelection::Contention => mcpeval::manifest::ProbeKind::Contention,
                cli::ProbeSelection::ErrorHonesty => mcpeval::manifest::ProbeKind::ErrorHonesty,
                cli::ProbeSelection::StateRecovery => mcpeval::manifest::ProbeKind::StateRecovery,
                cli::ProbeSelection::DiscoveryCost => mcpeval::manifest::ProbeKind::DiscoveryCost,
                cli::ProbeSelection::TokenCost => mcpeval::manifest::ProbeKind::TokenCost,
                cli::ProbeSelection::SchemaGuessability => {
                    mcpeval::manifest::ProbeKind::SchemaGuessability
                }
                cli::ProbeSelection::DegradationOverN => {
                    mcpeval::manifest::ProbeKind::DegradationOverN
                }
                cli::ProbeSelection::InstructionFidelity => {
                    mcpeval::manifest::ProbeKind::InstructionFidelity
                }
                cli::ProbeSelection::LatencyBudget => mcpeval::manifest::ProbeKind::LatencyBudget,
                cli::ProbeSelection::Pagination => mcpeval::manifest::ProbeKind::Pagination,
                cli::ProbeSelection::PayloadBounds => mcpeval::manifest::ProbeKind::PayloadBounds,
                cli::ProbeSelection::SurfaceListing => mcpeval::manifest::ProbeKind::SurfaceListing,
                cli::ProbeSelection::OutputSchema => mcpeval::manifest::ProbeKind::OutputSchema,
                cli::ProbeSelection::Cancellation => mcpeval::manifest::ProbeKind::Cancellation,
            });
            let full_battery = selected_probe.is_none();
            let mut store = mcpeval::store::Store::open(None)?;
            let report = mcpeval::probe::run(
                mcpeval::probe::ProbeOptions {
                    server: server.clone(),
                    manifest_path: manifest,
                    manifest_inline: None,
                    selected_probe,
                    selected_case: None,
                    allow_mutation,
                    command: cmd,
                    http_url: url,
                    allow_remote_http,
                },
                &mut store,
            )?;
            if full_battery {
                if let Err(error) = mcpeval::trends::record(store.root(), &server, &report) {
                    eprintln!("trend recording failed: {error}");
                }
            }
            let corpus = mcpeval::corpus::resolve(None, store.root());
            match format {
                cli::ProbeFormat::Json => {
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&report.to_json(&server))?
                    );
                }
                cli::ProbeFormat::Markdown => {
                    print!(
                        "{}",
                        mcpeval::report::render_probe_markdown(
                            &server,
                            &report,
                            corpus.as_ref(),
                            price_per_mtok
                        )
                    );
                }
                cli::ProbeFormat::Sarif => {
                    println!("{}", mcpeval::sarif::render_sarif(&server, &report));
                }
                cli::ProbeFormat::Text => {
                    render_probe_text(&server, &report, brief, corpus.as_ref(), price_per_mtok)
                }
            }
            if !report.passed() {
                std::process::exit(1);
            }
            Ok(())
        }
        cli::Command::Init {
            server,
            output,
            force,
            confirm_read_only,
            url,
            allow_remote_http,
            cmd,
        } => {
            let summary = mcpeval::init::run(mcpeval::init::InitOptions {
                server,
                output,
                force,
                confirm_read_only,
                command: cmd,
                http_url: url,
                allow_remote_http,
            })?;
            println!(
                "wrote {} ({} tools, {} schema-guessability cases)",
                summary.path.display(),
                summary.tool_count,
                summary.schema_cases
            );
            println!(
                "next: review budgets, add tool-specific fidelity/error probes, then run mcpeval probe"
            );
            Ok(())
        }
        cli::Command::Schema => {
            println!("{}", include_str!("../docs/mcp-eval.manifest.schema.json"));
            Ok(())
        }
        cli::Command::Explain { reason } => {
            match reason {
                None => {
                    println!("fixed failure reasons (mcpeval explain <reason>):");
                    for candidate in mcpeval::probe::FailureReason::ALL {
                        println!("  {}", candidate.as_str());
                    }
                }
                Some(requested) => {
                    // Also accept a bare case-level shorthand: the reason as
                    // it appears in reports, with or without the prefix.
                    let normalized = requested.trim().to_ascii_lowercase();
                    let candidate = mcpeval::probe::FailureReason::ALL
                        .iter()
                        .find(|candidate| candidate.as_str() == normalized);
                    match candidate {
                        Some(candidate) => {
                            println!("{}", mcpeval::remediation::hint(*candidate));
                        }
                        None => {
                            eprintln!(
                                "unknown reason {requested}; run mcpeval explain for the list"
                            );
                            std::process::exit(2);
                        }
                    }
                }
            }
            Ok(())
        }
        cli::Command::Compare {
            server,
            manifest,
            endpoints,
            command,
            format,
            allow_mutation,
            allow_remote_http,
        } => {
            let parsed = endpoints
                .iter()
                .map(|entry| {
                    let (label, url) = entry
                        .split_once('=')
                        .ok_or_else(|| anyhow::anyhow!("endpoint must be LABEL=URL"))?;
                    if label.is_empty() || url.is_empty() {
                        anyhow::bail!("endpoint label and URL must be non-empty");
                    }
                    Ok((label.to_owned(), url.to_owned()))
                })
                .collect::<anyhow::Result<Vec<_>>>()?;
            let mut labels = std::collections::HashSet::new();
            for (label, _) in &parsed {
                if !labels.insert(label.clone()) {
                    anyhow::bail!("endpoint labels must be unique");
                }
            }
            let output = mcpeval::compare::run(
                mcpeval::compare::CompareOptions {
                    server,
                    manifest_path: manifest,
                    endpoints: parsed,
                    command,
                    allow_mutation,
                    allow_remote_http,
                },
                match format {
                    cli::CompareFormat::Text => mcpeval::compare::CompareFormat::Text,
                    cli::CompareFormat::Markdown => mcpeval::compare::CompareFormat::Markdown,
                    cli::CompareFormat::Json => mcpeval::compare::CompareFormat::Json,
                },
            )?;
            print!("{output}");
            Ok(())
        }
        cli::Command::ExportIssues {
            dir,
            include_closed,
            force,
        } => {
            let store = mcpeval::store::Store::open(None)?;
            let written =
                mcpeval::report::export_issues(store.root(), &dir, include_closed, force)?;
            println!("wrote {written} issue files to {}", dir.display());
            Ok(())
        }
        cli::Command::Trends { last } => {
            let store = mcpeval::store::Store::open(None)?;
            print!("{}", mcpeval::trends::render(store.root(), last)?);
            Ok(())
        }
        cli::Command::Serve {
            listen,
            print_config,
        } => {
            if print_config {
                println!(
                    "{}",
                    serde_json::json!({
                        "mcpServers": {
                            "mcpeval": {
                                "type": "http",
                                "url": format!("http://{listen}/mcp")
                            }
                        }
                    })
                );
                return Ok(());
            }
            mcpeval::serve::run(listen)
        }
        cli::Command::Report {
            document,
            format,
            brief,
            price_per_mtok,
        } => {
            let body = if document.as_os_str() == "-" {
                use std::io::Read;
                let mut buffer = String::new();
                std::io::stdin().read_to_string(&mut buffer)?;
                buffer
            } else {
                std::fs::read_to_string(&document)
                    .with_context(|| format!("reading {}", document.display()))?
            };
            let parsed: serde_json::Value =
                serde_json::from_str(&body).context("report document is not valid JSON")?;
            let report = mcpeval::probe::ProbeReport::from_json_document(&parsed)
                .context("not a usable probe report")?;
            let server = parsed
                .get("server")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("unknown")
                .to_owned();
            match format {
                cli::ReportFormat::Text => {
                    render_probe_text(&server, &report, brief, None, price_per_mtok)
                }
                cli::ReportFormat::Markdown => print!(
                    "{}",
                    mcpeval::report::render_probe_markdown(&server, &report, None, price_per_mtok)
                ),
                cli::ReportFormat::Sarif => {
                    println!("{}", mcpeval::sarif::render_sarif(&server, &report));
                }
            }
            if !report.passed() {
                std::process::exit(1);
            }
            Ok(())
        }
        cli::Command::Verify {
            finding,
            case,
            manifest,
            allow_mutation,
            url,
            allow_remote_http,
            cmd,
        } => {
            let declaration = mcpeval::manifest::Manifest::load(&manifest)?;
            let selected = declaration
                .probes
                .iter()
                .find(|candidate| candidate.id() == case)
                .ok_or_else(|| anyhow::anyhow!("probe case is not declared in the manifest"))?;
            let mut store = mcpeval::store::Store::open(None)?;
            let server = mcpeval::lifecycle::prepare(
                store.root(),
                &finding,
                selected.id(),
                selected
                    .tool()
                    .ok_or_else(|| anyhow::anyhow!("finding verification requires a tool probe"))?,
            )?;
            let report = mcpeval::probe::run(
                mcpeval::probe::ProbeOptions {
                    server,
                    manifest_path: manifest,
                    manifest_inline: None,
                    selected_probe: None,
                    selected_case: Some(case.clone()),
                    allow_mutation,
                    command: cmd,
                    http_url: url,
                    allow_remote_http,
                },
                &mut store,
            )?;
            let passed = report.cases[0].passed();
            let status = mcpeval::lifecycle::record(
                store.root(),
                &finding,
                &case,
                passed,
                chrono::Utc::now(),
            )?;
            println!(
                "{finding} state={} probe={} consecutive_passes={}",
                status.state.as_str(),
                case,
                status.consecutive_passes
            );
            if !passed {
                std::process::exit(1);
            }
            Ok(())
        }
        cli::Command::Index => {
            let store = mcpeval::store::Store::open(None)?;
            let stats = mcpeval::index::build(store.root())?;
            println!(
                "indexed {} calls, {} failures, {} annotations",
                stats.calls, stats.failures, stats.annotations
            );
            Ok(())
        }
        cli::Command::Promote { threshold } => {
            let store = mcpeval::store::Store::open(None)?;
            let threshold = mcpeval::promote::resolve_threshold(store.root(), threshold)?;
            let stats = mcpeval::promote::promote(
                store.root(),
                mcpeval::promote::PromotionConfig {
                    threshold,
                    now: chrono::Utc::now(),
                },
            )?;
            println!("promoted {} of {} issues", stats.findings, stats.issues);
            Ok(())
        }
        cli::Command::Generate {
            finding,
            output,
            force,
            confirm_read_only,
        } => {
            let root = mcpeval::store::Store::resolve_root(None);
            let probe_id =
                mcpeval::generate::run(&root, &finding, &output, force, confirm_read_only)?;
            println!("{probe_id}");
            Ok(())
        }
        cli::Command::Findings { format } => {
            let store = mcpeval::store::Store::open(None)?;
            let format = match format {
                cli::FindingsFormat::Agent => mcpeval::report::ReportFormat::Agent,
                cli::FindingsFormat::Md => mcpeval::report::ReportFormat::Md,
                cli::FindingsFormat::Json => mcpeval::report::ReportFormat::Json,
            };
            print!("{}", mcpeval::report::render(store.root(), format)?);
            Ok(())
        }
        cli::Command::Annotate {
            session,
            seq,
            kind,
            note,
        } => {
            let record = mcpeval::record::AnnotationRecord {
                ts: chrono::Utc::now()
                    .format("%Y-%m-%dT%H:%M:%S%.3fZ")
                    .to_string(),
                session,
                seq,
                kind,
                note,
            };
            record.validate()?;
            let mut store = mcpeval::store::Store::open(None)?;
            store.append_annotation(&record)?;
            Ok(())
        }
        // `--check-redaction` is the only check `doctor` runs today, so
        // naming it, or naming nothing, both mean "run every check": it
        // always runs. A mistyped or omitted flag must never read as a
        // silent pass. When a second check is added, gate each one on its
        // own flag being set OR no flag being named at all, so this
        // "run everything by default" behavior survives.
        cli::Command::Share {
            dir,
            include_probe_history,
            force,
        } => {
            let summary = mcpeval::share::run(mcpeval::share::ShareOptions {
                output: dir,
                force,
                include_probe_history,
            })?;
            println!(
                "share envelope: {} ({} record files)",
                summary.directory.display(),
                summary.files
            );
            if summary.notes_requiring_review > 0 {
                println!(
                    "review before sharing: {} annotation note(s) contain prose",
                    summary.notes_requiring_review
                );
            }
            Ok(())
        }
        cli::Command::Doctor { check_redaction: _ } => {
            let store = mcpeval::store::Store::open(None)?;
            let report = mcpeval::doctor::check_redaction(store.root())?;
            println!("scanned {} files", report.files);
            for finding in &report.findings {
                println!("{finding}");
            }
            if report.notes_requiring_review > 0 {
                println!(
                    "{} annotation notes contain agent prose; review before sharing",
                    report.notes_requiring_review
                );
            }
            println!(
                "do not share: {} (fingerprint salt)",
                report.salt_path.display()
            );
            if !report.findings.is_empty() {
                std::process::exit(1);
            }
            Ok(())
        }
    }
}
