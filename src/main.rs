mod cli;

use clap::Parser;

fn main() -> anyhow::Result<()> {
    let cli = cli::Cli::parse();
    match cli.command {
        cli::Command::Shim { server, cmd } => {
            let code = mcpeval::shim::run(server, cmd)?;
            std::process::exit(code);
        }
        cli::Command::Probe {
            server,
            manifest,
            probe,
            allow_mutation,
            cmd,
        } => {
            let selected_probe = probe.map(|probe| match probe {
                cli::ProbeSelection::Contention => mcpeval::manifest::ProbeKind::Contention,
                cli::ProbeSelection::ErrorHonesty => mcpeval::manifest::ProbeKind::ErrorHonesty,
                cli::ProbeSelection::StateRecovery => mcpeval::manifest::ProbeKind::StateRecovery,
                cli::ProbeSelection::DiscoveryCost => mcpeval::manifest::ProbeKind::DiscoveryCost,
                cli::ProbeSelection::SchemaGuessability => {
                    mcpeval::manifest::ProbeKind::SchemaGuessability
                }
                cli::ProbeSelection::DegradationOverN => {
                    mcpeval::manifest::ProbeKind::DegradationOverN
                }
                cli::ProbeSelection::InstructionFidelity => {
                    mcpeval::manifest::ProbeKind::InstructionFidelity
                }
            });
            let mut store = mcpeval::store::Store::open(None)?;
            let report = mcpeval::probe::run(
                mcpeval::probe::ProbeOptions {
                    server,
                    manifest_path: manifest,
                    selected_probe,
                    selected_case: None,
                    allow_mutation,
                    command: cmd,
                },
                &mut store,
            )?;
            for case in &report.cases {
                let probe = match case.probe {
                    mcpeval::manifest::ProbeKind::Contention => "contention",
                    mcpeval::manifest::ProbeKind::ErrorHonesty => "error-honesty",
                    mcpeval::manifest::ProbeKind::StateRecovery => "state-recovery",
                    mcpeval::manifest::ProbeKind::DiscoveryCost => "discovery-cost",
                    mcpeval::manifest::ProbeKind::SchemaGuessability => "schema-guessability",
                    mcpeval::manifest::ProbeKind::DegradationOverN => "degradation-over-n",
                    mcpeval::manifest::ProbeKind::InstructionFidelity => "instruction-fidelity",
                };
                if case.passed() {
                    if let (Some(tools), Some(bytes)) = (case.tool_count, case.schema_bytes) {
                        println!(
                            "{} {probe} pass attempts={} tools={tools} schema_bytes={bytes}",
                            case.id, case.attempts
                        );
                    } else {
                        println!("{} {probe} pass attempts={}", case.id, case.attempts);
                    }
                } else {
                    let reason = match case.reason.expect("failed case has a reason") {
                        mcpeval::probe::FailureReason::UnexpectedOutcome => "unexpected-outcome",
                        mcpeval::probe::FailureReason::MissingField => "missing-field",
                        mcpeval::probe::FailureReason::ValueMismatch => "value-mismatch",
                        mcpeval::probe::FailureReason::ErrorCodeMismatch => "error-code-mismatch",
                        mcpeval::probe::FailureReason::DiscoveryLimitExceeded => {
                            "discovery-limit-exceeded"
                        }
                        mcpeval::probe::FailureReason::InvalidSchema => "invalid-schema",
                        mcpeval::probe::FailureReason::MissingRequiredArgument => {
                            "missing-required-argument"
                        }
                        mcpeval::probe::FailureReason::ExpectedError => "expected-error",
                        mcpeval::probe::FailureReason::UnstableErrorCode => "unstable-error-code",
                        mcpeval::probe::FailureReason::RetryabilityMismatch => {
                            "retryability-mismatch"
                        }
                        mcpeval::probe::FailureReason::RetryDidNotRecover => {
                            "retry-did-not-recover"
                        }
                        mcpeval::probe::FailureReason::FailureNotObserved => "failure-not-observed",
                        mcpeval::probe::FailureReason::RecoveryFailed => "recovery-failed",
                        mcpeval::probe::FailureReason::ValidationFailed => "validation-failed",
                        mcpeval::probe::FailureReason::ContendedClientFailed => {
                            "contended-client-failed"
                        }
                    };
                    if let (Some(tools), Some(bytes)) = (case.tool_count, case.schema_bytes) {
                        println!(
                            "{} {probe} fail attempts={} first_failure={} reason={reason} tools={tools} schema_bytes={bytes}",
                            case.id,
                            case.attempts,
                            case.first_failure.expect("failed case has a failure index")
                        );
                    } else {
                        println!(
                            "{} {probe} fail attempts={} first_failure={} reason={reason}",
                            case.id,
                            case.attempts,
                            case.first_failure.expect("failed case has a failure index")
                        );
                    }
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
                    selected_probe: None,
                    selected_case: Some(case.clone()),
                    allow_mutation,
                    command: cmd,
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
