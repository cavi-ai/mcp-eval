use clap::{Parser, Subcommand, ValueEnum};

#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum FindingsFormat {
    Agent,
    Md,
    Json,
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
    /// SARIF 2.1.0 for GitHub code-scanning annotations.
    Sarif,
}

#[derive(Clone, Copy, Debug, Default, ValueEnum)]
pub enum SchemaDocument {
    /// mcp-eval.manifest.json.
    #[default]
    Manifest,
    /// mcpeval.probe-report/v1 (`probe --format json`).
    Report,
    /// mcpeval.probe-diff/v1 (`diff --format json`).
    Diff,
}

#[derive(Clone, Copy, Debug, Default, ValueEnum)]
pub enum ReportFormat {
    /// Human-readable summary with readiness and hints.
    #[default]
    Text,
    /// Pull-request-ready markdown.
    Markdown,
    /// SARIF 2.1.0 for code-scanning uploads.
    Sarif,
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

#[derive(Clone, Copy, Debug, Default, ValueEnum)]
pub enum DiffFormat {
    /// Aligned per-case verdict movement.
    #[default]
    Text,
    /// Markdown movement table for issues and pull requests.
    Markdown,
    /// Deterministic JSON diff document (mcpeval.probe-diff/v1).
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
        probe: Option<mcpeval::manifest::ProbeKind>,
        /// Output format for the probe report.
        #[arg(long, value_enum, default_value_t = ProbeFormat::Text)]
        format: ProbeFormat,
        /// Suppress remediation hints in text output (for scripts).
        #[arg(long)]
        brief: bool,
        /// US dollars per million tokens, for interpreting the measured
        /// catalog cost in text and markdown reports. The JSON report
        /// stays price-free and deterministic.
        #[arg(long, value_name = "USD")]
        price_per_mtok: Option<f64>,
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
        /// Attest that every zero-required tool without a readOnlyHint
        /// annotation is read-only, so it is called and scaffolded too.
        /// Tools annotated destructiveHint or readOnlyHint=false are never
        /// called.
        #[arg(long)]
        confirm_read_only: bool,
        /// Scaffold only this tool; repeat for each tool.
        #[arg(long = "tool", value_name = "NAME")]
        tools: Vec<String>,
        /// Print each tool's decision without calling any tool or writing
        /// the manifest.
        #[arg(long)]
        dry_run: bool,
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
    /// Print a published JSON Schema: the manifest (for editor validation:
    /// add "$schema" pointing at docs/mcp-eval.manifest.schema.json), the
    /// probe report, or the diff document.
    Schema {
        /// Which document the schema describes.
        #[arg(value_enum, default_value_t = SchemaDocument::Manifest)]
        document: SchemaDocument,
    },
    /// Print the remediation guidance for a fixed failure reason.
    Explain {
        /// A fixed reason label, e.g. pagination-stalled-cursor. Pass no
        /// reason to list every label.
        #[arg(value_name = "REASON")]
        reason: Option<String>,
    },
    /// Run one manifest against several HTTP endpoints and diff the results.
    Compare {
        /// Shared server label for all endpoints in the report.
        #[arg(long)]
        server: String,
        /// Strict versioned probe and sandbox declaration.
        #[arg(long, default_value = "mcp-eval.manifest.json")]
        manifest: std::path::PathBuf,
        /// Endpoint as label=url; repeat for each HTTP endpoint.
        #[arg(long = "endpoint", value_name = "LABEL=URL")]
        endpoints: Vec<String>,
        /// Optional stdio command (after `--`) compared alongside the
        /// endpoints; its column is labeled `stdio`.
        #[arg(last = true)]
        command: Vec<String>,
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
    /// Compare a committed baseline report against a current report and
    /// classify every case as regressed, fixed, or unchanged. Both
    /// documents are mcpeval.probe-report/v1; pass `-` for stdin.
    Diff {
        /// Baseline report document (the committed gate).
        #[arg()]
        baseline: std::path::PathBuf,
        /// Current report document to gate.
        #[arg()]
        current: std::path::PathBuf,
        /// Exit non-zero when any case regressed.
        #[arg(long)]
        fail_on_regression: bool,
        /// Exit non-zero when any case fails for a different reason than in
        /// the baseline.
        #[arg(long)]
        fail_on_change: bool,
        /// Output format for the diff.
        #[arg(long, value_enum, default_value_t = DiffFormat::Text)]
        format: DiffFormat,
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
    /// Re-render a committed mcpeval.probe-report/v1 document (a baseline
    /// or CI artifact) into text, markdown, or SARIF without re-running
    /// any server. Reads the document from a file or stdin with `-`.
    Report {
        /// Path to the report document, or `-` for stdin.
        #[arg()]
        document: std::path::PathBuf,
        /// Output format for the re-rendered report.
        #[arg(long, value_enum, default_value_t = ReportFormat::Text)]
        format: ReportFormat,
        /// Manifest the report was produced from; SARIF results are located
        /// at its failing cases.
        #[arg(long, default_value = "mcp-eval.manifest.json")]
        manifest: std::path::PathBuf,
        /// Suppress remediation hints in text output.
        #[arg(long)]
        brief: bool,
        /// US dollars per million tokens for session-cost interpretation.
        #[arg(long, value_name = "USD")]
        price_per_mtok: Option<f64>,
    },
    /// Serve findings, trends, and the agent-loop tools over a loopback
    /// Streamable HTTP MCP endpoint (tools: list_findings, get_finding,
    /// get_readiness_trends, record_annotation, and with --allow-spawn
    /// run_probe and scaffold).
    Serve {
        /// Loopback socket address to accept MCP requests on.
        #[arg(long)]
        listen: String,
        /// Enable run_probe and scaffold, which launch the server process an
        /// agent names. Off by default.
        #[arg(long)]
        allow_spawn: bool,
        /// Print an MCP client config JSON snippet for this endpoint and
        /// exit without serving.
        #[arg(long)]
        print_config: bool,
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
    /// Package the share-safe envelope: the store subtree, minus trend
    /// history, with a SHARE.md manifest. Refuses to package a store the
    /// redaction sweep flags.
    Share {
        /// Directory that receives the envelope.
        #[arg(long)]
        dir: std::path::PathBuf,
        /// Include the readiness-trend history.
        #[arg(long)]
        include_probe_history: bool,
        /// Replace an existing populated directory.
        #[arg(long)]
        force: bool,
    },
}
