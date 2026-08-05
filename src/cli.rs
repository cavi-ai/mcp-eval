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
    /// Record an agent-authored observation about a call, identified by
    /// (session, seq): a documented path was blocked, a call reported
    /// success but changed nothing, and so on.
    Annotate {
        /// The session the annotated call belongs to.
        #[arg(long)]
        session: String,
        /// The seq of the call within that session.
        #[arg(long)]
        seq: u64,
        /// One of `record::ANNOTATION_KINDS`.
        #[arg(long)]
        kind: String,
        /// Free-text note: at most 240 characters, no control characters.
        #[arg(long)]
        note: String,
    },
}
