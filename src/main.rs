mod cli;

use clap::Parser;

fn main() -> anyhow::Result<()> {
    let cli = cli::Cli::parse();
    match cli.command {
        cli::Command::Shim { server, cmd } => {
            let code = mcpeval::shim::run(server, cmd)?;
            std::process::exit(code);
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
