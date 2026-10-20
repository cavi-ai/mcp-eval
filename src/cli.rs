use clap::{Args, Parser, Subcommand, ValueEnum};

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
    /// Versioned, deterministic JSON document (mcpeval.probe-report/v2).
    Json,
    /// Pull-request-ready markdown with a readiness score and badge.
    Markdown,
    /// SARIF 2.1.0 for GitHub code-scanning annotations.
    Sarif,
}

#[derive(Clone, Copy, Debug, Default, ValueEnum)]
pub enum ScoreFormat {
    /// Readiness, area scores, and each lost point with its hint.
    #[default]
    Text,
    /// Versioned, deterministic JSON document (mcpeval.probe-report/v2).
    Json,
    /// Pull-request-ready markdown with the readiness badge.
    Markdown,
}

#[derive(Clone, Copy, Debug, Default, ValueEnum)]
pub enum GuidanceFormat {
    #[default]
    Text,
    Json,
}

#[derive(Clone, Copy, Debug, Default, ValueEnum)]
pub enum SchemaDocument {
    /// mcp-eval.manifest.json.
    #[default]
    Manifest,
    /// mcpeval.probe-report/v2 (`probe` or `score --format json`).
    Report,
    /// mcpeval.probe-diff/v2 (`diff --format json`).
    Diff,
    /// mcpeval.guidance-report/v1 (`guidance --format json`).
    Guidance,
    /// mcpeval.corpus-verification/v1 (corpus harness replay --json).
    CorpusVerification,
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
    /// Deterministic JSON diff document (mcpeval.probe-diff/v2).
    Json,
}

/// Readiness floors shared by `probe` and `score`.
#[derive(Args, Debug)]
pub struct FloorArgs {
    /// Exit non-zero when readiness is under this score (0-100).
    #[arg(long, value_name = "SCORE", value_parser = clap::value_parser!(u64).range(0..=100))]
    pub min_readiness: Option<u64>,
    /// Exit non-zero when one area scores under a floor, as AREA=SCORE.
    /// Repeat for each area.
    #[arg(long = "min-area", value_name = "AREA=SCORE", value_parser = mcpeval::readiness_gate::parse_area_floor)]
    pub min_areas: Vec<(mcpeval::score::Area, u64)>,
    /// Compare the floors with every latency check at full marks; latency
    /// depends on the machine a run is on.
    #[arg(long)]
    pub exclude_latency: bool,
}

impl FloorArgs {
    pub fn floors(self) -> mcpeval::readiness_gate::Floors {
        mcpeval::readiness_gate::Floors {
            min_score: self.min_readiness,
            min_areas: self.min_areas,
            exclude_latency: self.exclude_latency,
        }
    }
}

#[derive(Parser, Debug)]
#[command(
    name = "mcpeval",
    version,
    about = "Evaluate MCP servers: a deterministic probe battery for CI and privacy-safe friction capture from real sessions",
    after_help = "Start here:\n  mcpeval init --server NAME --confirm-read-only -- your-mcp-server   # scaffold a manifest\n  mcpeval probe --server NAME -- your-mcp-server                     # run the battery\n  mcpeval shim --server NAME -- your-mcp-server                      # capture real sessions"
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Compare server instructions with capability-dependent tool catalogs; calls no tools.
    #[command(display_order = 3)]
    Guidance {
        #[arg(long)]
        server: String,
        /// Profiles to inspect (repeatable); defaults to none, roots, sampling, elicitation, all.
        #[arg(long, value_enum)]
        profile: Vec<mcpeval::guidance::Profile>,
        /// Tool expected in every selected profile (repeatable). Missing tools fail the gate.
        #[arg(long = "require-tool", value_name = "NAME")]
        required_tools: Vec<String>,
        /// Allow delayed post-initialization registration before reading the catalog (0..=5000).
        #[arg(long, default_value_t = 1500)]
        settle_ms: u64,
        #[arg(long, value_enum, default_value_t = GuidanceFormat::Text)]
        format: GuidanceFormat,
        #[arg(long)]
        url: Option<String>,
        #[arg(long, requires = "url")]
        allow_remote_http: bool,
        #[arg(last = true)]
        cmd: Vec<String>,
    },
    /// Proxy an MCP server on stdio, recording every framed message.
    #[command(display_order = 10)]
    Shim {
        /// Name this server is recorded under.
        #[arg(long)]
        server: String,
        /// The server command, after `--`.
        #[arg(last = true, required = true)]
        cmd: Vec<String>,
    },
    /// Proxy a Streamable HTTP MCP endpoint and record sanitized call metadata.
    #[command(display_order = 11)]
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
    #[command(display_order = 2)]
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
        /// Skip the standard battery and report the manifest gate only.
        #[arg(long)]
        gate_only: bool,
        /// Attest that tools annotated with neither readOnlyHint nor
        /// destructiveHint are read-only, so the standard battery calls them.
        #[arg(long)]
        confirm_read_only: bool,
        /// Never call this tool in the standard battery; it scores as
        /// unexercised. Repeat for each tool.
        #[arg(long = "skip-tool", value_name = "NAME")]
        skip_tools: Vec<String>,
        /// JSON object mapping tool names to the arguments the standard
        /// battery calls them with, for tools whose working inputs (a path,
        /// an ID) the schema does not imply. Reports carry only its digest.
        #[arg(long, value_name = "FILE")]
        arguments: Option<std::path::PathBuf>,
        #[command(flatten)]
        floors: FloorArgs,
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
    /// Score a server against mcpeval's standard battery; no manifest.
    #[command(display_order = 2)]
    Score {
        /// Name this server is recorded under.
        #[arg(long)]
        server: String,
        /// Output format.
        #[arg(long, value_enum, default_value_t = ScoreFormat::Text)]
        format: ScoreFormat,
        /// Suppress remediation hints in text output (for scripts).
        #[arg(long)]
        brief: bool,
        /// Attest that tools annotated with neither readOnlyHint nor
        /// destructiveHint are read-only, so the standard battery calls them.
        #[arg(long)]
        confirm_read_only: bool,
        /// Never call this tool in the standard battery; it scores as
        /// unexercised. Repeat for each tool.
        #[arg(long = "skip-tool", value_name = "NAME")]
        skip_tools: Vec<String>,
        /// JSON object mapping tool names to the arguments the standard
        /// battery calls them with, for tools whose working inputs (a path,
        /// an ID) the schema does not imply. Reports carry only its digest.
        #[arg(long, value_name = "FILE")]
        arguments: Option<std::path::PathBuf>,
        #[command(flatten)]
        floors: FloorArgs,
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
    #[command(display_order = 1)]
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
        /// JSON object mapping tool names to the arguments init calls them
        /// with and writes into their cases. Tools without an entry are
        /// called with `{}`, or with arguments built from their schema when
        /// they declare required parameters.
        #[arg(long, value_name = "FILE")]
        arguments: Option<std::path::PathBuf>,
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
    /// probe report, diff, guidance, or corpus verification document.
    #[command(display_order = 7)]
    Schema {
        /// Which document the schema describes.
        #[arg(value_enum, default_value_t = SchemaDocument::Manifest)]
        document: SchemaDocument,
    },
    /// Print the remediation guidance for a fixed failure reason.
    #[command(display_order = 6)]
    Explain {
        /// A fixed reason label, e.g. pagination-stalled-cursor. Pass no
        /// reason to list every label.
        #[arg(value_name = "REASON")]
        reason: Option<String>,
    },
    /// Run one manifest against several HTTP endpoints and diff the results.
    #[command(display_order = 5)]
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
        /// Skip the standard battery and compare the manifest gate only.
        #[arg(long)]
        gate_only: bool,
        /// Allow explicitly selected remote HTTPS endpoints.
        #[arg(long)]
        allow_remote_http: bool,
    },
    /// Compare a committed baseline report against a current report and
    /// classify every case as regressed, fixed, or unchanged. Both
    /// documents are mcpeval.probe-report/v1 or /v2; pass `-` for stdin.
    #[command(display_order = 4)]
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
        /// Exit non-zero when the catalog contract changed in a way that
        /// breaks callers: a tool removed, a parameter newly required or
        /// removed, a read-only tool become a writer or unannotated, or an
        /// output schema removed. Both documents must carry the catalog.
        #[arg(long)]
        fail_on_breaking: bool,
        /// Exit non-zero when readiness dropped by more than this many
        /// points since the baseline. Both documents must carry readiness
        /// under the same standard and measurement profile.
        #[arg(long, value_name = "POINTS", value_parser = clap::value_parser!(u64).range(0..=100))]
        max_readiness_drop: Option<u64>,
        /// Compare readiness with every latency check at full marks;
        /// latency depends on the machine a run is on.
        #[arg(long, requires = "max_readiness_drop")]
        exclude_latency: bool,
        /// Output format for the diff.
        #[arg(long, value_enum, default_value_t = DiffFormat::Text)]
        format: DiffFormat,
    },
    /// Write one GitHub-issue markdown file per open finding into a directory.
    #[command(display_order = 17)]
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
    #[command(display_order = 8)]
    Trends {
        /// Show at most this many runs per server.
        #[arg(long, default_value_t = 10)]
        last: usize,
    },
    /// Re-render a committed mcpeval.probe-report/v1 or /v2 document (a baseline
    /// or CI artifact) into text, markdown, or SARIF without re-running
    /// any server. Reads the document from a file or stdin with `-`.
    #[command(display_order = 3)]
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
    /// run_probe, scaffold, score, and verify_finding).
    #[command(display_order = 9)]
    Serve {
        /// Loopback socket address to accept MCP requests on.
        #[arg(long)]
        listen: String,
        /// Enable evaluation tools, which launch the server process an
        /// agent names. Off by default.
        #[arg(long)]
        allow_spawn: bool,
        /// Print an MCP client config JSON snippet for this endpoint and
        /// exit without serving.
        #[arg(long)]
        print_config: bool,
    },
    /// Verify one finding with one manifest case and advance its lifecycle.
    #[command(display_order = 16)]
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
    /// Refresh the SQLite index from JSONL records and derive failure windows.
    #[command(display_order = 12)]
    Index {
        /// Ignore cached checkpoints and reconstruct every derived row.
        #[arg(long)]
        rebuild: bool,
    },
    /// Aggregate indexed failures into issues and promote supported findings.
    #[command(display_order = 13)]
    Promote {
        /// Override config.json's promotion_threshold for this run.
        #[arg(long)]
        threshold: Option<f64>,
    },
    /// Generate a read-only manifest from an eligible promoted finding.
    #[command(display_order = 15)]
    Generate {
        /// Stable ID emitted by `mcpeval findings`.
        #[arg(long)]
        finding: String,
        /// Path for the generated manifest.
        #[arg(long)]
        output: std::path::PathBuf,
        /// Reviewed expectation JSON; required when metadata cannot supply a semantic oracle.
        #[arg(long)]
        expect: Option<std::path::PathBuf>,
        /// Replace an existing output file.
        #[arg(long)]
        force: bool,
        /// Attest that the selected tool is read-only; this does not authorize mutation.
        #[arg(long, required = true)]
        confirm_read_only: bool,
    },
    /// Render promoted findings without exposing captured private content.
    #[command(display_order = 14)]
    Findings {
        /// Output format for agents, people, or structured consumers.
        #[arg(long, value_enum, default_value_t = FindingsFormat::Agent)]
        format: FindingsFormat,
    },
    /// Record an agent-authored observation about a call, identified by
    /// event ID or legacy (session, seq): a documented path was blocked, a call reported
    /// success but changed nothing, and so on.
    #[command(display_order = 18)]
    Annotate {
        /// The recorder-generated event UUID from the call journal.
        #[arg(long, conflicts_with_all = ["session", "seq"])]
        event_id: Option<uuid::Uuid>,
        /// The session the annotated call belongs to.
        #[arg(long, required_unless_present = "event_id", requires = "seq")]
        session: Option<String>,
        /// The seq of the call within that session.
        #[arg(long, required_unless_present = "event_id", requires = "session")]
        seq: Option<u64>,
        /// One of `record::ANNOTATION_KINDS`.
        #[arg(long)]
        kind: String,
        /// Free-text note: at most 240 characters, no control characters.
        #[arg(long)]
        note: String,
    },
    /// Run store-hygiene checks against the capture root.
    #[command(display_order = 19)]
    Doctor {
        /// Scan every `*.jsonl` under the store for text that looks
        /// unredacted and exit non-zero if any is found.
        #[arg(long)]
        check_redaction: bool,
    },
    /// Package the share-safe envelope: the store subtree, minus trend
    /// history, with a SHARE.md manifest. Refuses to package a store the
    /// redaction sweep flags.
    #[command(display_order = 20)]
    Share {
        /// Directory that receives the envelope.
        #[arg(long)]
        dir: std::path::PathBuf,
        /// Include the readiness-trend history.
        #[arg(long)]
        include_probe_history: bool,
        /// Include free-form annotation notes after manual review.
        #[arg(long)]
        include_annotation_notes: bool,
        /// Replace an existing populated directory.
        #[arg(long)]
        force: bool,
    },
}
