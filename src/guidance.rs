//! Discovery-only guidance review across fresh capability-dependent sessions.
//! Prose references are candidates, never inferred contracts. Explicit operator
//! expectations supply the oracle for a missing-tool verdict.

use std::collections::BTreeSet;
use std::fmt::Write;
use std::time::Duration;

use anyhow::{bail, Context};
use clap::ValueEnum;
use serde::Serialize;
use serde_json::{json, Value};

use crate::probe::{ClientTarget, ProbeClient};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum Profile {
    None,
    Roots,
    Sampling,
    Elicitation,
    All,
}

impl Profile {
    fn capabilities(self) -> Value {
        match self {
            Self::None => json!({}),
            Self::Roots => json!({"roots": {"listChanged": false}}),
            Self::Sampling => json!({"sampling": {}}),
            Self::Elicitation => json!({"elicitation": {}}),
            Self::All => {
                json!({"roots": {"listChanged": false}, "sampling": {}, "elicitation": {}})
            }
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Roots => "roots",
            Self::Sampling => "sampling",
            Self::Elicitation => "elicitation",
            Self::All => "all",
        }
    }
}

pub struct Options {
    pub server: String,
    pub profiles: Vec<Profile>,
    pub required_tools: Vec<String>,
    pub settle_ms: u64,
    pub command: Vec<String>,
    pub http_url: Option<String>,
    pub allow_remote_http: bool,
}

#[derive(Debug, Serialize)]
pub struct ProfileReport {
    pub profile: Profile,
    /// None when the catalog could not be completely evaluated.
    pub tools: Option<BTreeSet<String>>,
    pub instructions_present: Option<bool>,
    pub error: Option<&'static str>,
}

#[derive(Debug, Serialize)]
pub struct Finding {
    pub profile: Profile,
    pub tool: String,
    pub reason: &'static str,
    pub hint: &'static str,
    /// Profiles in which this tool was actually listed; not a causal claim.
    pub available_in: Vec<Profile>,
}

#[derive(Debug, Serialize)]
pub struct Report {
    pub schema: &'static str,
    pub generator: Value,
    pub server: String,
    pub protocol_version: &'static str,
    /// A limited reference extractor, not a semantic interpretation of prose.
    pub reference_detection: &'static str,
    pub settle_ms: u64,
    pub complete: bool,
    /// Gate for explicitly required tools only; None for an incomplete run.
    pub passed: Option<bool>,
    pub required_tools: BTreeSet<String>,
    pub profiles: Vec<ProfileReport>,
    pub candidates: Vec<Finding>,
    pub defects: Vec<Finding>,
}

impl Report {
    pub fn text(&self) -> String {
        let mut text = format!(
            "{} guidance complete={} profiles={} defects={} review_candidates={}\n",
            self.server,
            self.complete,
            self.profiles.len(),
            self.defects.len(),
            self.candidates.len()
        );
        for profile in &self.profiles {
            if let Some(error) = profile.error {
                writeln!(text, "  {} error={error}", profile.profile.label()).unwrap();
            } else {
                writeln!(
                    text,
                    "  {} tools={} instructions_present={}",
                    profile.profile.label(),
                    profile.tools.as_ref().map_or(0, BTreeSet::len),
                    profile.instructions_present.unwrap_or(false)
                )
                .unwrap();
            }
        }
        for (kind, findings) in [("defect", &self.defects), ("review", &self.candidates)] {
            for finding in findings {
                writeln!(
                    text,
                    "  {kind} profile={} tool={} reason={} available_in={}\n    hint: {}",
                    finding.profile.label(),
                    finding.tool,
                    finding.reason,
                    finding
                        .available_in
                        .iter()
                        .map(|profile| profile.label())
                        .collect::<Vec<_>>()
                        .join(","),
                    finding.hint
                )
                .unwrap();
            }
        }
        text.push_str(
            "  review candidates do not fail the required-tool gate; prose intent needs review\n",
        );
        text
    }
}

struct Snapshot {
    tools: BTreeSet<String>,
    instructions: Option<String>,
}

pub fn run(options: Options) -> anyhow::Result<Report> {
    if !crate::privacy::valid_server(&options.server)
        || options.settle_ms > 5000
        || options
            .required_tools
            .iter()
            .any(|tool| !crate::privacy::valid_tool(tool))
    {
        return Err(crate::exit::usage(anyhow::anyhow!(
            "invalid guidance options"
        )));
    }
    if let Some(endpoint) = &options.http_url {
        crate::http_client::validate_endpoint(endpoint, options.allow_remote_http)
            .map_err(|_| crate::exit::usage(anyhow::anyhow!("invalid guidance endpoint")))?;
    }
    let target = ClientTarget::new(options.command, options.http_url, options.allow_remote_http)
        .map_err(crate::exit::usage)?;
    let profiles: BTreeSet<_> = if options.profiles.is_empty() {
        [
            Profile::None,
            Profile::Roots,
            Profile::Sampling,
            Profile::Elicitation,
            Profile::All,
        ]
        .into_iter()
        .collect()
    } else {
        options.profiles.into_iter().collect()
    };
    let mut report = Report {
        schema: "mcpeval.guidance-report/v1",
        generator: json!({"name": "mcpeval", "version": env!("CARGO_PKG_VERSION")}),
        server: options.server,
        protocol_version: crate::http_client::PROTOCOL_VERSION,
        reference_detection: "inline-code-known-tools",
        settle_ms: options.settle_ms,
        complete: true,
        passed: Some(true),
        required_tools: options.required_tools.into_iter().collect(),
        profiles: vec![],
        candidates: vec![],
        defects: vec![],
    };
    let mut snapshots = Vec::new();
    for profile in profiles {
        match inspect(&target, profile, options.settle_ms) {
            Ok(snapshot) => {
                report.profiles.push(ProfileReport {
                    profile,
                    tools: Some(snapshot.tools.clone()),
                    instructions_present: Some(snapshot.instructions.is_some()),
                    error: None,
                });
                snapshots.push((profile, snapshot));
            }
            Err(_) => {
                report.complete = false;
                report.passed = None;
                report.profiles.push(ProfileReport {
                    profile,
                    tools: None,
                    instructions_present: None,
                    error: Some("profile-incomplete"),
                });
            }
        }
    }
    // Incomplete discovery cannot supply a complete reference vocabulary.
    if !report.complete {
        return Ok(report);
    }
    let known: BTreeSet<_> = snapshots
        .iter()
        .flat_map(|(_, s)| s.tools.iter().cloned())
        .collect();
    let available_in = |tool: &str| {
        snapshots
            .iter()
            .filter(|(_, snapshot)| snapshot.tools.contains(tool))
            .map(|(profile, _)| *profile)
            .collect::<Vec<_>>()
    };
    let availability: std::collections::BTreeMap<_, _> = known
        .iter()
        .map(|tool| (tool.clone(), available_in(tool)))
        .collect();
    for (profile, snapshot) in snapshots {
        for tool in report.required_tools.difference(&snapshot.tools) {
            report.defects.push(Finding {
                profile, tool: tool.clone(), reason: "required-tool-unavailable",
                hint: "make the required tool available in this client profile, or correct the explicit required-tool expectation",
                available_in: availability.get(tool).cloned().unwrap_or_default(),
            });
        }
        let references: BTreeSet<_> = snapshot
            .instructions
            .as_deref()
            .unwrap_or_default()
            .split('`')
            .enumerate()
            .filter(|(index, token)| index % 2 == 1 && known.contains(*token))
            .map(|(_, token)| token.to_owned())
            .collect();
        for tool in references.difference(&snapshot.tools) {
            report.candidates.push(Finding {
                profile, tool: tool.clone(), reason: "referenced-tool-unavailable",
                hint: "review this tool reference in the session instructions; state its required client capability and check the session catalog before recommending it",
                available_in: availability.get(tool).cloned().unwrap_or_default(),
            });
        }
    }
    report.passed = Some(report.defects.is_empty());
    Ok(report)
}

fn inspect(target: &ClientTarget, profile: Profile, settle_ms: u64) -> anyhow::Result<Snapshot> {
    let mut client = target.connect(None)?;
    let response = client.initialize_with_capabilities(&profile.capabilities())?;
    let result = response
        .get("result")
        .filter(|v| v.is_object())
        .context("invalid initialization")?;
    if result.get("protocolVersion").and_then(Value::as_str)
        != Some(crate::http_client::PROTOCOL_VERSION)
    {
        bail!("unsupported guidance protocol version");
    }
    let capabilities = result
        .get("capabilities")
        .filter(|v| v.is_object())
        .context("invalid capabilities")?;
    let instructions = match result.get("instructions") {
        None => None,
        Some(Value::String(text)) => Some(text.clone()),
        Some(_) => bail!("invalid instructions"),
    };
    if let Some(tools) = capabilities.get("tools") {
        if !tools.is_object() {
            bail!("invalid tools capability");
        }
        // Conditional registration may occur after notifications/initialized.
        // This bounded, operator-visible window is part of the observation.
        let wait = target.budget.timeout(Duration::from_millis(settle_ms))?;
        std::thread::sleep(wait);
        target.budget.check()?;
        Ok(Snapshot {
            tools: catalog(&mut client)?,
            instructions,
        })
    } else {
        Ok(Snapshot {
            tools: BTreeSet::new(),
            instructions,
        })
    }
}

/// Strict discovery: later-page errors, duplicates, malformed cursors, and
/// page exhaustion are incomplete evaluation, never an absent-tool verdict.
fn catalog(client: &mut ProbeClient) -> anyhow::Result<BTreeSet<String>> {
    let mut tools = BTreeSet::new();
    let mut cursors = BTreeSet::new();
    let mut params = json!({});
    for _ in 0..crate::mcp_client::MAX_TOOL_PAGES {
        let response = client.raw_request("tools/list", params)?;
        let result = response.get("result").context("catalog error")?;
        let entries = result
            .get("tools")
            .and_then(Value::as_array)
            .context("invalid catalog")?;
        for entry in entries {
            let tool = crate::mcp_client::tool_definition(entry)?;
            if !tools.insert(tool.name) || tools.len() > 10_000 {
                bail!("invalid catalog entries");
            }
        }
        match result.get("nextCursor") {
            None => return Ok(tools),
            Some(Value::String(cursor)) if !cursor.is_empty() && cursors.insert(cursor.clone()) => {
                params = json!({"cursor": cursor});
            }
            Some(_) => bail!("invalid catalog cursor"),
        }
    }
    bail!("catalog page limit exceeded")
}
