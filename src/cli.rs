use clap::{Parser, Subcommand, ValueEnum};

#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum FindingsFormat {
    Agent,
    Md,
    Json,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum ProbeSelection {
    Contention,
    ErrorHonesty,
    StateRecovery,
    DiscoveryCost,
    TokenCost,
    SchemaGuessability,
    DegradationOverN,
    InstructionFidelity,
    LatencyBudget,
    Pagination,
}

#[derive(Clone, Copy, Debug, Default, ValueEnum)]
pub enum ProbeFormat {
    /// Human-readable one-line-per-case summary.
    #[default]
    Text,
    /// Versioned, deterministic JSON document (mcpeval.probe-report/v1).
    Json,
    /// Pull-request-ready markdown with a readiness score and badge.
    Markdown,
}

#[derive(Clone, Copy, Debug, Default, ValueEnum)]
pub enum CompareFormat {
    /// Aligned pass/fail grid.
    #[default]
    Text,
    /// Markdown comparison table for issues and pull requests.
    Markdown,
    /// Deterministic JSON array of per-endpoint probe reports.
    Json,
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
    /// Proxy a Streamable HTTP MCP endpoint and record sanitized call metadata.
    ShimHttp {
        /// Name this server is recorded under.
        #[arg(long)]
        server: String,
        /// Loopback socket address to accept MCP requests on.
        #[arg(long)]
        listen: String,
        /// Streamable HTTP endpoint to forward requests to.
        #[arg(long)]
        upstream: String,
        /// Allow an explicitly selected remote HTTPS upstream.
        #[arg(long)]
        allow_remote_http: bool,
    },
    /// Run deterministic, privacy-safe probes against an MCP server.
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
        /// Output format for the probe report.
        #[arg(long, value_enum, default_value_t = ProbeFormat::Text)]
        format: ProbeFormat,
        /// Explicitly authorize manifest-declared sandbox mutations.
        #[arg(long)]
        allow_mutation: bool,
        /// Streamable HTTP endpoint instead of a stdio command.
        #[arg(long)]
        url: Option<String>,
        /// Allow an explicitly selected remote HTTPS endpoint.
        #[arg(long, requires = "url")]
        allow_remote_http: bool,
        /// The server command, after `--`.
        #[arg(last = true)]
        cmd: Vec<String>,
    },
    /// Scaffold a starter manifest from a live server's tool catalog.
    Init {
        /// Name this server is recorded under.
        #[arg(long)]
        server: String,
        /// Path for the generated manifest.
        #[arg(long, default_value = "mcp-eval.manifest.json")]
        output: std::path::PathBuf,
        /// Replace an existing manifest.
        #[arg(long)]
        force: bool,
        /// Attest that every empty-argument schema check is read-only.
        #[arg(long)]
        confirm_read_only: bool,
        /// Streamable HTTP endpoint instead of a stdio command.
        #[arg(long)]
        url: Option<String>,
        /// Allow an explicitly selected remote HTTPS endpoint.
        #[arg(long, requires = "url")]
        allow_remote_http: bool,
        /// The server command, after `--`.
        #[arg(last = true)]
        cmd: Vec<String>,
    },
    /// Print the JSON Schema for mcp-eval.manifest.json (for editor
    /// validation: add "$schema" pointing at docs/mcp-eval.manifest.schema.json).
    Schema,
    /// Run one manifest against several HTTP endpoints and diff the results.
    Compare {
        /// Shared server label for all endpoints in the report.
        #[arg(long)]
        server: String,
        /// Strict versioned probe and sandbox declaration.
        #[arg(long, default_value = "mcp-eval.manifest.json")]
        manifest: std::path::PathBuf,
        /// Endpoint as label=url; repeat to compare more than two.
        #[arg(long = "endpoint", value_name = "LABEL=URL", required = true)]
        endpoints: Vec<String>,
        /// Output format for the comparison table.
        #[arg(long, value_enum, default_value_t = CompareFormat::Text)]
        format: CompareFormat,
        /// Explicitly authorize manifest-declared sandbox mutations.
        #[arg(long)]
        allow_mutation: bool,
        /// Allow explicitly selected remote HTTPS endpoints.
        #[arg(long)]
        allow_remote_http: bool,
    },
    /// Write one GitHub-issue markdown file per open finding into a directory.
    ExportIssues {
        /// Directory that receives <finding-id>.md files.
        #[arg(long)]
        dir: std::path::PathBuf,
        /// Include fix-claimed, verifying, and closed findings.
        #[arg(long)]
        include_closed: bool,
        /// Replace existing files in the directory.
        #[arg(long)]
        force: bool,
    },
    /// Show readiness-score history recorded by previous probe runs.
    Trends {
        /// Show at most this many runs per server.
        #[arg(long, default_value_t = 10)]
        last: usize,
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
        /// Streamable HTTP endpoint instead of a stdio command.
        #[arg(long)]
        url: Option<String>,
        /// Allow an explicitly selected remote HTTPS endpoint.
        #[arg(long, requires = "url")]
        allow_remote_http: bool,
        /// The server command, after `--`.
        #[arg(last = true)]
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
    /// Generate a read-only manifest from an eligible promoted finding.
    Generate {
        /// Stable ID emitted by `mcpeval findings`.
        #[arg(long)]
        finding: String,
        /// Path for the generated manifest.
        #[arg(long)]
        output: std::path::PathBuf,
        /// Replace an existing output file.
        #[arg(long)]
        force: bool,
        /// Attest that the selected tool is read-only; this does not authorize mutation.
        #[arg(long, required = true)]
        confirm_read_only: bool,
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
