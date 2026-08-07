use clap::{Parser, Subcommand, ValueEnum};

#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum FindingsFormat {
    Agent,
    Md,
    Json,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum ProbeSelection {
    DiscoveryCost,
    SchemaGuessability,
    DegradationOverN,
    InstructionFidelity,
}

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
    /// Run deterministic, privacy-safe probes against an MCP stdio server.
    Probe {
        /// Name this server is recorded under.
        #[arg(long)]
        server: String,
        /// Strict versioned probe and sandbox declaration.
        #[arg(long, default_value = "mcp-eval.manifest.json")]
        manifest: std::path::PathBuf,
        /// Run only one probe kind.
        #[arg(long, value_enum)]
        probe: Option<ProbeSelection>,
        /// Explicitly authorize manifest-declared sandbox mutations.
        #[arg(long)]
        allow_mutation: bool,
        /// The server command, after `--`.
        #[arg(last = true, required = true)]
        cmd: Vec<String>,
    },
    /// Verify one finding with one manifest case and advance its lifecycle.
    Verify {
        /// Stable ID emitted by `mcpeval findings`.
        #[arg(long)]
        finding: String,
        /// Probe case ID from the manifest.
        #[arg(long)]
        case: String,
        /// Strict versioned probe and sandbox declaration.
        #[arg(long, default_value = "mcp-eval.manifest.json")]
        manifest: std::path::PathBuf,
        /// Explicitly authorize a manifest-declared sandbox mutation.
        #[arg(long)]
        allow_mutation: bool,
        /// The server command, after `--`.
        #[arg(last = true, required = true)]
        cmd: Vec<String>,
    },
    /// Load JSONL records into the SQLite index and derive failure windows.
    Index,
    /// Aggregate indexed failures into issues and promote supported findings.
    Promote {
        /// Override config.json's promotion_threshold for this run.
        #[arg(long)]
        threshold: Option<f64>,
    },
    /// Render promoted findings without exposing captured private content.
    Findings {
        /// Output format for agents, people, or structured consumers.
        #[arg(long, value_enum, default_value_t = FindingsFormat::Agent)]
        format: FindingsFormat,
    },
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
    /// Run store-hygiene checks against the capture root.
    Doctor {
        /// Scan every `*.jsonl` under the store for text that looks
        /// unredacted and exit non-zero if any is found.
        #[arg(long)]
        check_redaction: bool,
    },
}
