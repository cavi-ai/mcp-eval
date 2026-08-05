use std::collections::{HashMap, HashSet};

use serde_json::Value;

use crate::privacy;
use crate::record::{error_info, CallRecord};
use crate::shape::{self, EnumIndex};

struct Pending {
    method: String,
    tool: Option<String>,
    args: Option<Value>,
    sent_ms: u64,
    shim_self_us: u64,
}

pub struct Correlator {
    server: String,
    session: String,
    seq: u64,
    enums: EnumIndex,
    declared_tools: HashSet<String>,
    pending: HashMap<String, Pending>,
}

impl Correlator {
    pub fn new(server: String, session: String) -> Self {
        Self {
            server,
            session: privacy::opaque_session(&session),
            seq: 0,
            enums: EnumIndex::new(),
            declared_tools: HashSet::new(),
            pending: HashMap::new(),
        }
    }

    pub fn on_outbound(&mut self, v: &Value, now_ms: u64) {
        self.on_outbound_with_overhead(v, now_ms, 0);
    }

    pub fn on_outbound_with_overhead(&mut self, v: &Value, now_ms: u64, base_us: u64) {
        let started = std::time::Instant::now();
        let Some(method) = v.get("method").and_then(Value::as_str) else {
            return;
        };
        let Some(id) = id_key(v) else { return };
        let params = v.get("params");
        let requested_tool = params.and_then(|p| p.get("name")).and_then(Value::as_str);
        let tool = requested_tool.map(|name| {
            if self.declared_tools.contains(name) {
                name.to_string()
            } else {
                "unlisted".into()
            }
        });
        let args = params.and_then(|p| p.get("arguments")).map(|a| {
            shape::of(
                a,
                requested_tool
                    .filter(|name| self.declared_tools.contains(*name))
                    .unwrap_or(""),
                &self.enums,
            )
        });
        self.pending.insert(
            id,
            Pending {
                method: method.to_string(),
                tool,
                args,
                sent_ms: now_ms,
                shim_self_us: base_us.saturating_add(started.elapsed().as_micros() as u64),
            },
        );
    }

    pub fn on_inbound(&mut self, v: &Value, now_ms: u64) -> Option<CallRecord> {
        let is_response = v.get("result").is_some() || v.get("error").is_some();
        if let Some(result) = v.get("result") {
            if let Some(id) = id_key(v) {
                if let Some(p) = self.pending.get(&id) {
                    if p.method == "tools/list" {
                        self.learn_tools(result);
                    }
                }
            }
        }
        match id_key(v) {
            Some(id) if is_response => {
                let p = self.pending.remove(&id)?;
                let is_error = v.get("error").is_some();
                self.seq += 1;
                Some(CallRecord {
                    ts: now_iso(),
                    session: self.session.clone(),
                    seq: self.seq,
                    server: self.server.clone(),
                    method: p.method,
                    tool: p.tool,
                    args: p.args,
                    latency_ms: Some(now_ms.saturating_sub(p.sent_ms)),
                    outcome: if is_error {
                        "error".into()
                    } else {
                        "ok".into()
                    },
                    error: if is_error { Some(error_info(v)) } else { None },
                    shim_self_us: p.shim_self_us,
                    kind: "real".into(),
                })
            }
            Some(_) => None,
            None => {
                let method = v.get("method").and_then(Value::as_str)?.to_string();
                self.seq += 1;
                Some(CallRecord {
                    ts: now_iso(),
                    session: self.session.clone(),
                    seq: self.seq,
                    server: self.server.clone(),
                    method,
                    tool: None,
                    args: None,
                    latency_ms: None,
                    outcome: "notification".into(),
                    error: None,
                    shim_self_us: 0,
                    kind: "real".into(),
                })
            }
        }
    }

    pub fn on_unparsed(&mut self, direction: &str, _now_ms: u64) -> CallRecord {
        self.seq += 1;
        CallRecord {
            ts: now_iso(),
            session: self.session.clone(),
            seq: self.seq,
            server: self.server.clone(),
            method: format!("unparsed/{direction}"),
            tool: None,
            args: None,
            latency_ms: None,
            outcome: "unparsed".into(),
            error: None,
            shim_self_us: 0,
            kind: "real".into(),
        }
    }

    fn learn_tools(&mut self, result: &Value) {
        let Some(tools) = result.get("tools").and_then(Value::as_array) else {
            return;
        };
        for tool in tools {
            let (Some(name), Some(schema)) = (
                tool.get("name").and_then(Value::as_str),
                tool.get("inputSchema"),
            ) else {
                continue;
            };
            if !privacy::valid_tool(name) {
                continue;
            }
            self.declared_tools.insert(name.to_string());
            self.enums.learn(name, schema);
        }
    }
}

fn id_key(v: &Value) -> Option<String> {
    match v.get("id") {
        Some(Value::String(s)) => Some(format!("s:{s}")),
        Some(Value::Number(n)) => Some(format!("n:{n}")),
        _ => None,
    }
}

fn now_iso() -> String {
    chrono::Utc::now()
        .format("%Y-%m-%dT%H:%M:%S%.3fZ")
        .to_string()
}
