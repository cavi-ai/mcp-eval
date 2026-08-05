use clap::{Parser, Subcommand};

#[derive(Parser, Debug)]
#[command(
    name = "mcpeval",
    version,
    about = "MCP friction capture and evaluation"
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Proxy an MCP server on stdio, recording every framed message.
    Shim {
        /// Name this server is recorded under.
        #[arg(long)]
        server: String,
        /// The server command, after `--`.
        #[arg(last = true, required = true)]
        cmd: Vec<String>,
    },
    /// Load JSONL records into the SQLite index and derive failure windows.
    Index,
}
