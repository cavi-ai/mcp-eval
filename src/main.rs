mod cli;

use clap::Parser;

fn main() -> anyhow::Result<()> {
    let cli = cli::Cli::parse();
    match cli.command {
        cli::Command::Shim { .. } => anyhow::bail!("shim not implemented yet"),
        cli::Command::Index => anyhow::bail!("index not implemented yet"),
    }
}
