//! Session-path friction from indexed shim records: what an agent's
//! `tools/call` sequence cost before it got a result. Reports counts and
//! tool names only; no arguments, error text, or session labels.

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::bail;
use rusqlite::{Connection, OptionalExtension};
use serde::Serialize;

pub const SCHEMA: &str = "mcpeval.friction/v1";
/// Consecutive failed calls to one tool that make a retry storm.
pub const STORM_CALLS: u64 = 3;

#[derive(Debug, Default, PartialEq, Serialize)]
pub struct ServerFriction {
    pub server: String,
    pub sessions: u64,
    pub calls: u64,
    /// Failed calls before each session's first successful call, summed.
    pub failed_before_first_success: u64,
    /// Sessions in which no call succeeded.
    pub sessions_without_success: u64,
    /// Tools with a retry storm or a correction, most storms first.
    pub tools: Vec<ToolFriction>,
    /// A failed call to one tool directly followed by a successful call to
    /// another: the agent picked the wrong tool first.
    pub switches: Vec<Switch>,
    /// Sessions whose call order the index cannot establish (legacy records
    /// sharing a sequence number): their calls and successes count, but
    /// they add no storms, corrections, switches, or failures before the
    /// first success.
    #[serde(skip_serializing_if = "is_zero")]
    pub unordered_sessions: u64,
}

fn is_zero(value: &u64) -> bool {
    *value == 0
}

#[derive(Debug, Default, PartialEq, Serialize)]
pub struct ToolFriction {
    pub tool: String,
    /// Runs of at least [`STORM_CALLS`] consecutive failed calls.
    pub retry_storms: u64,
    /// Runs of failed calls ended by a successful call to the same tool.
    pub corrections: u64,
    /// Failed calls inside those corrected runs.
    pub correction_failures: u64,
}

#[derive(Debug, PartialEq, Serialize)]
pub struct Switch {
    pub from: String,
    pub to: String,
    pub count: u64,
}

#[derive(Default)]
struct Tally {
    sessions: u64,
    calls: u64,
    failed_before_first_success: u64,
    sessions_without_success: u64,
    unordered_sessions: u64,
    tools: BTreeMap<String, ToolFriction>,
    switches: BTreeMap<(String, String), u64>,
}

impl Tally {
    fn tool(&mut self, name: &str) -> &mut ToolFriction {
        self.tools
            .entry(name.to_owned())
            .or_insert_with(|| ToolFriction {
                tool: name.to_owned(),
                ..ToolFriction::default()
            })
    }

    /// One session's real `tools/call` outcomes, in order: `Some(ok)` for a
    /// call that succeeded or failed, `None` for one whose outcome is
    /// unknown, which separates the calls around it. A session that is not
    /// `ordered` counts only its calls and whether any succeeded.
    fn session(&mut self, calls: &[(String, Option<bool>)], ordered: bool) {
        let known = calls.iter().filter(|(_, ok)| ok.is_some()).count() as u64;
        if known == 0 {
            return;
        }
        self.sessions += 1;
        self.calls += known;
        let succeeded = calls.iter().any(|(_, ok)| *ok == Some(true));
        if !succeeded {
            self.sessions_without_success += 1;
        }
        if !ordered {
            self.unordered_sessions += 1;
            return;
        }
        if succeeded {
            self.failed_before_first_success += calls
                .iter()
                .take_while(|(_, ok)| *ok != Some(true))
                .filter(|(_, ok)| *ok == Some(false))
                .count() as u64;
        }
        // The tool of the current run of failed calls, and its length.
        let mut run: Option<(&str, u64)> = None;
        let mut previous: Option<(&str, bool)> = None;
        for (tool, ok) in calls {
            let tool = tool.as_str();
            let Some(ok) = *ok else {
                self.close(run.take());
                previous = None;
                continue;
            };
            if let Some((from, false)) = previous {
                if ok && from != tool {
                    *self
                        .switches
                        .entry((from.to_owned(), tool.to_owned()))
                        .or_default() += 1;
                }
            }
            let same = run.is_some_and(|(failing, _)| failing == tool);
            if ok {
                if let (true, Some((_, failures))) = (same, run) {
                    let entry = self.tool(tool);
                    entry.corrections += 1;
                    entry.correction_failures += failures;
                }
                self.close(run.take());
            } else if same {
                run = run.map(|(failing, failures)| (failing, failures + 1));
            } else {
                self.close(run.replace((tool, 1)));
            }
            previous = Some((tool, ok));
        }
        self.close(run);
    }

    fn close(&mut self, run: Option<(&str, u64)>) {
        if let Some((tool, failures)) = run {
            if failures >= STORM_CALLS {
                self.tool(tool).retry_storms += 1;
            }
        }
    }

    fn finish(self, server: String) -> ServerFriction {
        let mut tools: Vec<ToolFriction> = self.tools.into_values().collect();
        tools.sort_by(|a, b| {
            (b.retry_storms, b.corrections)
                .cmp(&(a.retry_storms, a.corrections))
                .then_with(|| a.tool.cmp(&b.tool))
        });
        let mut switches: Vec<Switch> = self
            .switches
            .into_iter()
            .map(|((from, to), count)| Switch { from, to, count })
            .collect();
        switches.sort_by_key(|switch| std::cmp::Reverse(switch.count));
        ServerFriction {
            server,
            sessions: self.sessions,
            calls: self.calls,
            failed_before_first_success: self.failed_before_first_success,
            sessions_without_success: self.sessions_without_success,
            tools,
            switches,
            unordered_sessions: self.unordered_sessions,
        }
    }
}

/// Friction per server, by name, from the index under `root`.
pub fn load(root: &Path) -> anyhow::Result<Vec<ServerFriction>> {
    let path = root.join("index.db");
    if !path.is_file() {
        bail!("friction is unavailable; run `mcpeval index` first");
    }
    let db = Connection::open(path)?;
    let indexed: Option<String> = db
        .query_row(
            "SELECT name FROM sqlite_master WHERE type='table' AND name='calls'",
            [],
            |row| row.get(0),
        )
        .optional()?;
    if indexed.is_none() {
        bail!("friction is unavailable; run `mcpeval index` first");
    }
    // Legacy records (no capture) sharing a session, server, and sequence
    // number have no recorded order, as in the index's failure windows.
    let mut statement = db.prepare(
        "WITH ambiguous AS (
           SELECT session, server FROM calls WHERE capture_id IS NULL
           GROUP BY session, server, seq HAVING COUNT(*) > 1)
         SELECT c.server, COALESCE(c.capture_id, ''), c.session, c.tool, c.outcome,
           c.capture_id IS NULL AND EXISTS (
             SELECT 1 FROM ambiguous a WHERE a.session=c.session AND a.server=c.server)
         FROM calls c
         WHERE c.method='tools/call' AND c.kind='real' AND c.tool IS NOT NULL
         ORDER BY c.server, COALESCE(c.capture_id, ''), c.session, c.seq, c.id",
    )?;
    let rows = statement.query_map([], |row| {
        let outcome = row.get::<_, String>(4)?;
        Ok((
            row.get::<_, String>(0)?,
            (row.get::<_, String>(1)?, row.get::<_, String>(2)?),
            row.get::<_, String>(3)?,
            match outcome.as_str() {
                "ok" => Some(true),
                "error" => Some(false),
                _ => None,
            },
            row.get::<_, bool>(5)?,
        ))
    })?;
    let mut servers: BTreeMap<String, Tally> = BTreeMap::new();
    let mut current: Option<(String, (String, String))> = None;
    let mut calls: Vec<(String, Option<bool>)> = Vec::new();
    let mut ordered = true;
    for row in rows {
        let (server, session, tool, ok, ambiguous) = row?;
        let key = (server, session);
        if current.as_ref() != Some(&key) {
            if let Some((server, _)) = current.take() {
                servers.entry(server).or_default().session(&calls, ordered);
            }
            calls.clear();
            ordered = true;
            current = Some(key);
        }
        ordered &= !ambiguous;
        calls.push((tool, ok));
    }
    if let Some((server, _)) = current {
        servers.entry(server).or_default().session(&calls, ordered);
    }
    Ok(servers
        .into_iter()
        .map(|(server, tally)| tally.finish(server))
        .collect())
}

pub fn render_json(servers: &[ServerFriction]) -> anyhow::Result<String> {
    Ok(serde_json::to_string_pretty(&serde_json::json!({
        "schema": SCHEMA,
        "storm_calls": STORM_CALLS,
        "servers": servers,
    }))? + "\n")
}

pub fn render_text(servers: &[ServerFriction]) -> String {
    let mut out = String::new();
    for server in servers {
        out.push_str(&format!(
            "{}: {} sessions, {} calls, {} failed before the first success, {} sessions without one\n",
            server.server,
            server.sessions,
            server.calls,
            server.failed_before_first_success,
            server.sessions_without_success
        ));
        if server.unordered_sessions > 0 {
            out.push_str(&format!(
                "  {} sessions without a recorded call order; only their calls and successes count\n",
                server.unordered_sessions
            ));
        }
        for tool in &server.tools {
            out.push_str(&format!(
                "  {}: {} retry storms, {} corrections after {} failed calls\n",
                tool.tool, tool.retry_storms, tool.corrections, tool.correction_failures
            ));
        }
        for switch in &server.switches {
            out.push_str(&format!(
                "  {} failed, then {} succeeded: {} times\n",
                switch.from, switch.to, switch.count
            ));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tally(sessions: &[&[(&str, bool)]]) -> ServerFriction {
        let mut tally = Tally::default();
        for calls in sessions {
            let calls: Vec<(String, Option<bool>)> = calls
                .iter()
                .map(|(tool, ok)| ((*tool).to_owned(), Some(*ok)))
                .collect();
            tally.session(&calls, true);
        }
        tally.finish("demo".into())
    }

    #[test]
    fn storms_corrections_and_switches_are_counted_per_session_path() {
        let friction = tally(&[
            // A storm the agent corrected on the fourth call.
            &[
                ("click", false),
                ("click", false),
                ("click", false),
                ("click", true),
            ],
            // The wrong tool, then the right one; a two-call storm is none.
            &[("press", false), ("press", false), ("click", true)],
            // Nothing ever worked.
            &[("press", false)],
        ]);
        assert_eq!(
            friction,
            ServerFriction {
                server: "demo".into(),
                sessions: 3,
                calls: 8,
                failed_before_first_success: 5,
                sessions_without_success: 1,
                tools: vec![ToolFriction {
                    tool: "click".into(),
                    retry_storms: 1,
                    corrections: 1,
                    correction_failures: 3,
                }],
                switches: vec![Switch {
                    from: "press".into(),
                    to: "click".into(),
                    count: 1,
                }],
                unordered_sessions: 0,
            }
        );
    }
}
