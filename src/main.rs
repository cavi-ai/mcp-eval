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
            println!("indexed {} calls, {} failures", stats.calls, stats.failures);
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
    }
}
