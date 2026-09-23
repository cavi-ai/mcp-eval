mod cli;

use anyhow::Context;
use clap::Parser;

fn render_sarif(
    server: &str,
    report: &mcpeval::probe::ProbeReport,
    manifest: &std::path::Path,
) -> String {
    let cwd = std::env::current_dir().ok();
    let uri = mcpeval::sarif::manifest_uri(manifest, cwd.as_deref());
    let text = std::fs::read_to_string(manifest).ok();
    mcpeval::sarif::render_sarif(
        server,
        report,
        &mcpeval::sarif::ManifestSource {
            uri: &uri,
            text: text.as_deref(),
        },
    )
}

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
        } else if case.errored() {
            let reason = case.reason.expect("errored case has a reason");
            println!("{} {probe} error reason={}", case.id, reason.as_str());
            if !brief {
                println!("  hint: {}", mcpeval::remediation::hint(reason));
            }
        } else {
            let reason = case.reason.expect("failed case has a reason");
            if let Some(detail) = case.detail {
                measurements.push_str(&format!(
                    " bound={} limit={} observed={}",
                    detail.bound, detail.limit, detail.observed
                ));
            }
            println!(
                "{} {probe} fail attempts={} first_failure={} reason={}{measurements}",
                case.id,
                case.attempts,
                case.first_failure.expect("failed case has a failure index"),
                reason.as_str()
            );
            if !brief {
                println!("  hint: {}", mcpeval::remediation::hint(reason));
                if let (mcpeval::probe::FailureReason::TokenBudgetExceeded, Some(usage)) =
                    (reason, &case.token_usage)
                {
                    println!("  heaviest: {}", usage.heaviest());
                }
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
    println!("{server} readiness {}/100 {categories}", readiness.overall);
    if let Some(corpus) = corpus {
        if let Some(battery) = mcpeval::score::readiness_over(report, &corpus.battery) {
            let placement = corpus.placement(battery.overall);
            println!(
                "  corpus battery ({}): {}/100, above {}, tied with {}, below {} of {} observed servers",
                corpus.battery_label(),
                battery.overall,
                placement.above,
                placement.tied,
                placement.below,
                corpus.observations.len()
            );
        }
        if let (Some(tokens), Some(tools)) = (
            mcpeval::score::catalog_tokens(report),
            mcpeval::score::catalog_tool_count(report),
        ) {
            if let Some(catalog) = corpus.catalog_placement(tokens) {
                println!(
                    "  catalog: {tokens} tokens over {tools} tools, lighter than {} of {} observed servers (median {} tokens)",
                    catalog.lighter_than, catalog.observed, catalog.median_tokens
                );
            }
        }
    }
    if let (Some(price), Some(usage)) = (price_per_mtok, mcpeval::score::catalog_tokens(report)) {
        println!("  cost: {}", mcpeval::score::cost_context(usage, price));
    }
}

fn main() {
    if let Err(error) = run() {
        eprintln!("Error: {error:?}");
        std::process::exit(mcpeval::exit::code(&error));
    }
}

/// Exit for a report that ran: incomplete runs outrank red verdicts.
fn exit_for_report(report: &mcpeval::probe::ProbeReport) {
    if report.errored() {
        std::process::exit(mcpeval::exit::INFRASTRUCTURE);
    }
    if !report.passed() {
        std::process::exit(mcpeval::exit::VERDICT);
    }
}

fn run() -> anyhow::Result<()> {
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
            probe: selected_probe,
            format,
            brief,
            price_per_mtok,
            allow_mutation,
            url,
            allow_remote_http,
            cmd,
        } => {
            let full_battery = selected_probe.is_none();
            let mut store = mcpeval::store::Store::open(None)?;
            let report = mcpeval::probe::run(
                mcpeval::probe::ProbeOptions {
                    server: server.clone(),
                    manifest_path: manifest.clone(),
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
            // A trend point is a complete battery: an errored run would
            // record the transport, not the server.
            if full_battery && !report.errored() {
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
                    println!("{}", render_sarif(&server, &report, &manifest));
                }
                cli::ProbeFormat::Text => {
                    render_probe_text(&server, &report, brief, corpus.as_ref(), price_per_mtok)
                }
            }
            exit_for_report(&report);
            Ok(())
        }
        cli::Command::Init {
            server,
            output,
            force,
            confirm_read_only,
            tools,
            dry_run,
            url,
            allow_remote_http,
            cmd,
        } => {
            let options = mcpeval::init::InitOptions {
                server,
                output,
                force,
                confirm_read_only,
                tools,
                command: cmd,
                http_url: url,
                allow_remote_http,
            };
            if dry_run {
                for (tool, decision) in mcpeval::init::dry_run(&options)? {
                    println!("{tool}  {}", decision.as_str());
                }
                println!("dry run: no tool was called and nothing was written");
                return Ok(());
            }
            let summary = mcpeval::init::run(options)?;
            let kinds: Vec<String> = summary
                .kind_counts
                .iter()
                .map(|(kind, count)| format!("{} {count}", kind.as_str()))
                .collect();
            let skipped = match summary.skipped_by_annotations {
                0 => String::new(),
                1 => " (1 tool skipped by annotations)".to_owned(),
                count => format!(" ({count} tools skipped by annotations)"),
            };
            println!(
                "wrote {} ({} tools, {} cases: {}){skipped}",
                summary.path.display(),
                summary.tool_count,
                summary.case_count,
                kinds.join(", ")
            );
            println!(
                "next: review budgets; add error-honesty, state-recovery, and instruction-fidelity \
                 cases (they need expected inputs) and cancellation, sampling, elicitation, \
                 resource-subscription, and completion cases where the server supports them, then \
                 run mcpeval probe"
            );
            Ok(())
        }
        cli::Command::Schema { document } => {
            print!(
                "{}",
                match document {
                    cli::SchemaDocument::Manifest => {
                        include_str!("../docs/mcp-eval.manifest.schema.json")
                    }
                    cli::SchemaDocument::Report => {
                        include_str!("../docs/mcp-eval.probe-report.schema.json")
                    }
                    cli::SchemaDocument::Diff =>
                        include_str!("../docs/mcp-eval.probe-diff.schema.json"),
                }
            );
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
                            let family = normalized.split('-').next().unwrap_or_default();
                            let similar = mcpeval::probe::FailureReason::ALL
                                .iter()
                                .map(|candidate| candidate.as_str())
                                .filter(|label| {
                                    !normalized.is_empty()
                                        && (label.contains(normalized.as_str())
                                            || label.split('-').next() == Some(family))
                                })
                                .collect::<Vec<_>>();
                            if similar.is_empty() {
                                eprintln!(
                                    "unknown reason {requested}; run mcpeval explain for the list"
                                );
                            } else {
                                eprintln!(
                                    "unknown reason {requested}; did you mean: {}",
                                    similar.join(", ")
                                );
                            }
                            std::process::exit(mcpeval::exit::USAGE);
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
                .collect::<anyhow::Result<Vec<_>>>()
                .map_err(mcpeval::exit::usage)?;
            let mut labels = std::collections::HashSet::new();
            for (label, _) in &parsed {
                if !labels.insert(label.clone()) {
                    return Err(mcpeval::exit::usage(anyhow::anyhow!(
                        "endpoint labels must be unique"
                    )));
                }
            }
            let comparison = mcpeval::compare::run(
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
            print!("{}", comparison.output);
            if comparison.errored {
                std::process::exit(mcpeval::exit::INFRASTRUCTURE);
            }
            Ok(())
        }
        cli::Command::Diff {
            baseline,
            current,
            fail_on_regression,
            fail_on_change,
            format,
        } => {
            let baseline_document = mcpeval::diff::load_document(&baseline)
                .with_context(|| format!("loading baseline {}", baseline.display()))
                .map_err(mcpeval::exit::usage)?;
            let current_document = mcpeval::diff::load_document(&current)
                .with_context(|| format!("loading current {}", current.display()))
                .map_err(mcpeval::exit::usage)?;
            mcpeval::diff::ensure_same_server(&baseline_document, &current_document)
                .map_err(mcpeval::exit::usage)?;
            let outcome = mcpeval::diff::diff(&baseline_document.report, &current_document.report);
            match format {
                cli::DiffFormat::Json => {
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&mcpeval::diff::to_json(&outcome))?
                    );
                }
                cli::DiffFormat::Markdown => {
                    print!("{}", mcpeval::diff::render_markdown(&outcome));
                }
                cli::DiffFormat::Text => {
                    print!("{}", mcpeval::diff::render(&outcome));
                }
            }
            if (fail_on_regression && outcome.gated()) || (fail_on_change && outcome.changed() > 0)
            {
                std::process::exit(mcpeval::exit::VERDICT);
            }
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
            allow_spawn,
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
            mcpeval::serve::run(listen, allow_spawn)
        }
        cli::Command::Report {
            document,
            format,
            manifest,
            brief,
            price_per_mtok,
        } => {
            let loaded = mcpeval::diff::load_document(&document).map_err(mcpeval::exit::usage)?;
            let report = loaded.report;
            let server = loaded.server.unwrap_or_else(|| "unknown".to_owned());
            match format {
                cli::ReportFormat::Text => {
                    render_probe_text(&server, &report, brief, None, price_per_mtok)
                }
                cli::ReportFormat::Markdown => print!(
                    "{}",
                    mcpeval::report::render_probe_markdown(&server, &report, None, price_per_mtok)
                ),
                cli::ReportFormat::Sarif => {
                    println!("{}", render_sarif(&server, &report, &manifest));
                }
            }
            exit_for_report(&report);
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
            let declaration =
                mcpeval::manifest::Manifest::load(&manifest).map_err(mcpeval::exit::usage)?;
            let selected = declaration
                .probes
                .iter()
                .find(|candidate| candidate.id() == case)
                .ok_or_else(|| {
                    mcpeval::exit::usage(anyhow::anyhow!(
                        "probe case is not declared in the manifest"
                    ))
                })?;
            let mut store = mcpeval::store::Store::open(None)?;
            let server = mcpeval::lifecycle::prepare(
                store.root(),
                &finding,
                selected.id(),
                selected.tool().ok_or_else(|| {
                    mcpeval::exit::usage(anyhow::anyhow!(
                        "finding verification requires a tool probe"
                    ))
                })?,
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
            // A case that could not be evaluated is no evidence either way:
            // the finding's lifecycle is left untouched.
            if let Some(reason) = report.cases[0]
                .reason
                .filter(|reason| reason.is_transport())
            {
                println!(
                    "{finding} not verified: probe={case} reason={}",
                    reason.as_str()
                );
                std::process::exit(mcpeval::exit::INFRASTRUCTURE);
            }
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
                std::process::exit(mcpeval::exit::VERDICT);
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
