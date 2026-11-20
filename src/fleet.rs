//! Every server in an MCP client config scored together: the catalog tokens
//! the set costs each session, and tool names more than one server lists.
//! Reports server names, scores, counts, and tool names; never commands,
//! arguments, environment values, or URLs.

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::Context;
use serde::Serialize;
use serde_json::Value;

pub const SCHEMA: &str = "mcpeval.fleet/v1";

/// A launchable `mcpServers` entry.
#[derive(Debug, PartialEq)]
pub enum Entry {
    Stdio {
        command: Vec<String>,
        env: Vec<(String, String)>,
    },
    Http {
        url: String,
    },
}

#[derive(Debug, PartialEq, Serialize)]
pub struct Skipped {
    pub server: String,
    pub reason: &'static str,
}

#[derive(Debug, PartialEq, Serialize)]
pub struct ServerResult {
    pub server: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub readiness: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub readiness_error: Option<&'static str>,
    pub tools: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub catalog_tokens: Option<u64>,
}

#[derive(Debug, PartialEq, Serialize)]
pub struct Collision {
    pub tool: String,
    pub servers: Vec<String>,
}

#[derive(Debug, PartialEq, Serialize)]
pub struct Fleet {
    pub schema: &'static str,
    pub servers: Vec<ServerResult>,
    pub skipped: Vec<Skipped>,
    /// Tools over every scored server.
    pub tools: u64,
    /// Catalog tokens over every scored server: what loading the whole
    /// config costs one session before any call.
    pub catalog_tokens: u64,
    pub collisions: Vec<Collision>,
}

/// A client config's `mcpServers` entries by server name, and the entries
/// that cannot be launched with the reason.
#[derive(Debug, Default, PartialEq)]
pub struct Config {
    pub entries: Vec<(String, Entry)>,
    pub skipped: Vec<Skipped>,
}

pub fn load(path: &Path) -> anyhow::Result<Config> {
    let text =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let config: Value =
        serde_json::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
    let servers = config
        .get("mcpServers")
        .and_then(Value::as_object)
        .with_context(|| format!("{} has no mcpServers object", path.display()))?;
    let mut entries = Vec::new();
    let mut skipped = Vec::new();
    for (server, entry) in servers {
        let parsed = if crate::privacy::valid_server(server) {
            parse(entry)
        } else {
            Err("invalid-server-label")
        };
        match parsed {
            Ok(entry) => entries.push((server.clone(), entry)),
            Err(reason) => skipped.push(Skipped {
                server: server.clone(),
                reason,
            }),
        }
    }
    Ok(Config { entries, skipped })
}

fn parse(entry: &Value) -> Result<Entry, &'static str> {
    if entry.get("type").and_then(Value::as_str) == Some("sse") {
        return Err("sse-transport");
    }
    let strings = |value: Option<&Value>| -> Result<Vec<String>, &'static str> {
        match value {
            None => Ok(Vec::new()),
            Some(Value::Array(items)) => items
                .iter()
                .map(|item| item.as_str().ok_or("invalid-entry").and_then(expand))
                .collect(),
            Some(_) => Err("invalid-entry"),
        }
    };
    if let Some(program) = entry.get("command") {
        let mut command = vec![program.as_str().ok_or("invalid-entry").and_then(expand)?];
        command.extend(strings(entry.get("args"))?);
        let env = match entry.get("env") {
            None => Vec::new(),
            Some(Value::Object(variables)) => variables
                .iter()
                .map(|(name, value)| {
                    let value = value.as_str().ok_or("invalid-entry").and_then(expand)?;
                    Ok((name.clone(), value))
                })
                .collect::<Result<_, &'static str>>()?,
            Some(_) => return Err("invalid-entry"),
        };
        return Ok(Entry::Stdio { command, env });
    }
    match entry.get("url").map(Value::as_str) {
        Some(Some(url)) => Ok(Entry::Http { url: expand(url)? }),
        Some(None) => Err("invalid-entry"),
        None => Err("no-command-or-url"),
    }
}

/// `${NAME}` and `${NAME:-default}` replaced from the environment, as
/// client configs write them.
fn expand(text: &str) -> Result<String, &'static str> {
    let mut expanded = String::new();
    let mut rest = text;
    while let Some(start) = rest.find("${") {
        expanded.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let end = after.find('}').ok_or("invalid-entry")?;
        let (name, default) = match after[..end].split_once(":-") {
            Some((name, default)) => (name, Some(default)),
            None => (&after[..end], None),
        };
        match (std::env::var(name), default) {
            (Ok(value), _) => expanded.push_str(&value),
            (Err(_), Some(default)) => expanded.push_str(default),
            (Err(_), None) => return Err("unset-variable"),
        }
        rest = &after[end + 1..];
    }
    expanded.push_str(rest);
    Ok(expanded)
}

/// Score every launchable entry of the config at `path`.
pub fn run(path: &Path, allow_remote_http: bool) -> anyhow::Result<Fleet> {
    let Config {
        entries,
        mut skipped,
    } = load(path).map_err(crate::exit::usage)?;
    let mut servers = Vec::new();
    let mut owners: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (server, entry) in entries {
        let (command, env, http_url) = match entry {
            Entry::Stdio { command, env } => (command, env, None),
            Entry::Http { url } => (Vec::new(), Vec::new(), Some(url)),
        };
        let report = match crate::probe::score(crate::probe::ScoreOptions {
            server: server.clone(),
            command,
            env,
            http_url,
            allow_remote_http,
            confirm_read_only: false,
            skip_tools: Vec::new(),
            arguments: None,
            sandbox: false,
            sandbox_network: false,
            repeats: crate::standard::REPEATS,
        }) {
            Ok(report) => report,
            Err(_) => {
                skipped.push(Skipped {
                    server,
                    reason: "invalid-entry",
                });
                continue;
            }
        };
        let mut names: Vec<&str> = report
            .catalog
            .iter()
            .flat_map(|catalog| &catalog.tools)
            .map(|tool| tool.name.as_str())
            .collect();
        names.sort_unstable();
        names.dedup();
        for name in &names {
            owners
                .entry((*name).to_owned())
                .or_default()
                .push(server.clone());
        }
        servers.push(ServerResult {
            readiness: report.readiness.as_ref().map(|readiness| readiness.score),
            readiness_error: report.readiness_error.map(|reason| reason.as_str()),
            tools: crate::score::catalog_tool_count(&report).unwrap_or(names.len() as u64),
            catalog_tokens: crate::score::catalog_tokens(&report),
            server,
        });
    }
    skipped.sort_by(|left, right| left.server.cmp(&right.server));
    Ok(Fleet {
        schema: SCHEMA,
        tools: servers.iter().map(|server| server.tools).sum(),
        catalog_tokens: servers
            .iter()
            .filter_map(|server| server.catalog_tokens)
            .sum(),
        collisions: owners
            .into_iter()
            .filter(|(_, servers)| servers.len() > 1)
            .map(|(tool, servers)| Collision { tool, servers })
            .collect(),
        servers,
        skipped,
    })
}

pub fn render_json(fleet: &Fleet) -> anyhow::Result<String> {
    Ok(format!("{}\n", serde_json::to_string_pretty(fleet)?))
}

pub fn render_text(fleet: &Fleet) -> String {
    let mut text = format!(
        "fleet: {} servers scored, {} skipped; {} tools, {} catalog tokens per session\n",
        fleet.servers.len(),
        fleet.skipped.len(),
        fleet.tools,
        fleet.catalog_tokens
    );
    for server in &fleet.servers {
        match (server.readiness, server.readiness_error) {
            (Some(score), _) => text.push_str(&format!(
                "  {} readiness {score}/100 tools={} catalog_tokens={}\n",
                server.server,
                server.tools,
                server.catalog_tokens.unwrap_or(0)
            )),
            (None, reason) => text.push_str(&format!(
                "  {} readiness error reason={}\n",
                server.server,
                reason.unwrap_or("unmeasured")
            )),
        }
    }
    for skipped in &fleet.skipped {
        text.push_str(&format!(
            "  skipped {} reason={}\n",
            skipped.server, skipped.reason
        ));
    }
    for collision in &fleet.collisions {
        text.push_str(&format!(
            "  collision {}: {}\n",
            collision.tool,
            collision.servers.join(", ")
        ));
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn config_entries_expand_variables_and_unlaunchable_entries_say_why() {
        // PATH is set on every platform; HOME is not set on Windows.
        let path = std::env::var("PATH").unwrap();
        let entry = |value: Value| parse(&value);
        assert_eq!(
            entry(json!({"command": "node", "args": ["${PATH}/index.js"],
                "env": {"MODE": "${MCPEVAL_FLEET_TEST_UNSET:-read}"}})),
            Ok(Entry::Stdio {
                command: vec!["node".into(), format!("{path}/index.js")],
                env: vec![("MODE".into(), "read".into())],
            })
        );
        assert_eq!(
            entry(
                json!({"type": "http", "url": "${MCPEVAL_FLEET_TEST_UNSET:-http://127.0.0.1:1}/mcp"})
            ),
            Ok(Entry::Http {
                url: "http://127.0.0.1:1/mcp".into()
            })
        );
        assert_eq!(
            entry(json!({"type": "sse", "url": "http://127.0.0.1:1/sse"})),
            Err("sse-transport")
        );
        assert_eq!(
            entry(json!({"command": "${MCPEVAL_FLEET_TEST_UNSET}"})),
            Err("unset-variable")
        );
        assert_eq!(entry(json!({"args": []})), Err("no-command-or-url"));
        assert_eq!(entry(json!({"command": ["node"]})), Err("invalid-entry"));
    }
}
