# mcp-eval Phase 1 (shim + store) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Capture every MCP call an agent makes through a redacting stdio passthrough, and land it in a queryable store — with proof that no payload content reaches disk and that the shim adds under 2ms at p95.

**Architecture:** `mcpeval shim -- <server cmd>` spawns the real MCP server as a child process and pumps newline-delimited JSON-RPC between the agent's stdin/stdout and the child, unmodified. Every framed message is cloned to a redactor that extracts argument *shape* (never values), templates error messages, and appends a flat record to a daily JSONL file. A separate `mcpeval index` pass loads those records into SQLite and derives the failure windows. The shim never blocks the proxied stream on store I/O.

**Tech Stack:** Rust (edition 2021), clap, serde/serde_json, rusqlite (bundled SQLite), uuid, chrono, regex, anyhow. Tests are `cargo test`.

## Global Constraints

- Redaction happens in the shim before anything is written. There is no verbose mode and no raw frame buffer anywhere in the codebase.
- Never recorded: free text, contents of text fields, filesystem paths, URL path/query/fragment, header values, any unbounded string not declared as an enum by the server's own schema.
- Recorded values are limited to: argument keys, value types, string length buckets, container sizes, schema-declared enum values, numeric and boolean scalars, registrable domain of URLs, and path-root/depth booleans.
- Error objects record only the keys `code`, `layer`, `retryable`, `kind`; the human message is stored only as a template with variable substrings replaced by placeholders.
- Shim overhead budget: p95 under 2ms added per proxied message. Each record carries `shim_self_us` so overhead can be subtracted from measurements.
- The proxied stream is byte-transparent: whatever the agent sends reaches the server unchanged, and vice versa. A message the shim cannot parse is forwarded untouched and recorded as `unparsed`.
- Store root is `$MCPEVAL_HOME` when set, else `~/.mcp-eval`.
- Deviation from the design, deliberate: the design shows `window` inside the `call`
  record. It is derived by `mcpeval index` instead (Task 9). Holding a record open in the
  shim until three later calls arrive would put mutable state on the forwarding path for
  no gain; the derived form carries identical information.
- Plans and specs live under `docs/plan/` and `docs/design/` in this repo; `plans/` and `specs/` are ignored by the user's global gitignore and must not be used as directory names.

---

### Task 1: Repo scaffolding and CLI skeleton

**Files:**
- Create: `Cargo.toml`
- Create: `src/main.rs`
- Create: `src/cli.rs`
- Create: `rust-toolchain.toml`
- Create: `.gitignore`
- Test: `tests/cli.rs`

**Interfaces:**
- Consumes: nothing.
- Produces: binary `mcpeval`; `cli::Cli` (clap parser) with subcommands `Shim { server: String, cmd: Vec<String> }` and `Index`.

- [ ] **Step 1: Write the failing test**

`tests/cli.rs`:

```rust
use std::process::Command;

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_mcpeval")
}

#[test]
fn prints_version() {
    let out = Command::new(bin()).arg("--version").output().unwrap();
    assert!(out.status.success());
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(text.contains("mcpeval"), "unexpected version output: {text}");
}

#[test]
fn shim_requires_a_server_name_and_command() {
    let out = Command::new(bin()).arg("shim").output().unwrap();
    assert!(!out.status.success(), "shim with no args must fail");
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --test cli`
Expected: FAIL — no `Cargo.toml`, nothing to build.

- [ ] **Step 3: Write minimal implementation**

`Cargo.toml`:

```toml
[package]
name = "mcpeval"
version = "0.1.0"
edition = "2021"

[dependencies]
anyhow = "1"
chrono = { version = "0.4", default-features = false, features = ["clock", "std"] }
clap = { version = "4", features = ["derive"] }
regex = "1"
rusqlite = { version = "0.32", features = ["bundled"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
uuid = { version = "1", features = ["v4"] }
```

`rust-toolchain.toml`:

```toml
[toolchain]
channel = "stable"
```

`.gitignore`:

```
/target
```

`src/cli.rs`:

```rust
use clap::{Parser, Subcommand};

#[derive(Parser, Debug)]
#[command(name = "mcpeval", version, about = "MCP friction capture and evaluation")]
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
```

`src/main.rs`:

```rust
mod cli;

use clap::Parser;

fn main() -> anyhow::Result<()> {
    let cli = cli::Cli::parse();
    match cli.command {
        cli::Command::Shim { .. } => anyhow::bail!("shim not implemented yet"),
        cli::Command::Index => anyhow::bail!("index not implemented yet"),
    }
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test --test cli`
Expected: PASS (2 tests).

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml Cargo.lock rust-toolchain.toml .gitignore src tests
git commit -m "Add mcpeval CLI skeleton"
```

---

### Task 2: Newline-delimited JSON-RPC codec

**Files:**
- Create: `src/frame.rs`
- Modify: `src/main.rs` (add `mod frame;`)
- Test: `tests/frame.rs`

**Interfaces:**
- Consumes: nothing.
- Produces:
  - `frame::Frame { pub raw: Vec<u8>, pub value: Option<serde_json::Value> }`
  - `frame::read_frame<R: BufRead>(r: &mut R) -> std::io::Result<Option<Frame>>` — returns `Ok(None)` at EOF. `value` is `None` when the line is not valid JSON.

MCP's stdio transport is newline-delimited JSON: one message per line, no embedded raw newlines. The codec preserves the exact bytes of each line, including its terminator, so forwarding is byte-transparent.

- [ ] **Step 1: Write the failing test**

`tests/frame.rs`:

```rust
use std::io::BufReader;
use mcpeval::frame::{read_frame, Frame};

#[test]
fn reads_two_messages_and_preserves_bytes() {
    let input = b"{\"jsonrpc\":\"2.0\",\"id\":1}\n{\"jsonrpc\":\"2.0\",\"id\":2}\n";
    let mut r = BufReader::new(&input[..]);

    let first: Frame = read_frame(&mut r).unwrap().unwrap();
    assert_eq!(first.raw, b"{\"jsonrpc\":\"2.0\",\"id\":1}\n");
    assert_eq!(first.value.unwrap()["id"], 1);

    let second = read_frame(&mut r).unwrap().unwrap();
    assert_eq!(second.value.unwrap()["id"], 2);

    assert!(read_frame(&mut r).unwrap().is_none(), "third read must be EOF");
}

#[test]
fn unparsable_line_is_still_a_frame() {
    let input = b"this is not json\n";
    let mut r = BufReader::new(&input[..]);
    let f = read_frame(&mut r).unwrap().unwrap();
    assert_eq!(f.raw, b"this is not json\n");
    assert!(f.value.is_none());
}

#[test]
fn final_line_without_newline_is_returned() {
    let input = b"{\"id\":9}";
    let mut r = BufReader::new(&input[..]);
    let f = read_frame(&mut r).unwrap().unwrap();
    assert_eq!(f.raw, b"{\"id\":9}");
    assert_eq!(f.value.unwrap()["id"], 9);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --test frame`
Expected: FAIL — `mcpeval::frame` does not exist, and there is no library target.

- [ ] **Step 3: Write minimal implementation**

Add a library target so integration tests can import modules. `Cargo.toml`, append:

```toml
[lib]
name = "mcpeval"
path = "src/lib.rs"

[[bin]]
name = "mcpeval"
path = "src/main.rs"
```

`src/lib.rs`:

```rust
pub mod frame;
```

`src/frame.rs`:

```rust
use std::io::BufRead;

#[derive(Debug, Clone)]
pub struct Frame {
    /// Exact bytes read, including the trailing newline when present.
    pub raw: Vec<u8>,
    /// Parsed JSON, or None when the line was not valid JSON.
    pub value: Option<serde_json::Value>,
}

/// Reads one newline-delimited message. Returns Ok(None) at end of input.
pub fn read_frame<R: BufRead>(r: &mut R) -> std::io::Result<Option<Frame>> {
    let mut raw = Vec::new();
    let n = r.read_until(b'\n', &mut raw)?;
    if n == 0 {
        return Ok(None);
    }
    let value = serde_json::from_slice(&raw).ok();
    Ok(Some(Frame { raw, value }))
}
```

`src/main.rs` is unchanged in this task — the binary does not use `frame` yet; the
library target is what the integration test imports.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test --test frame`
Expected: PASS (3 tests).

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml src/lib.rs src/frame.rs src/main.rs tests/frame.rs
git commit -m "Add newline-delimited JSON-RPC codec"
```

---

### Task 3: Shape extraction

**Files:**
- Create: `src/shape.rs`
- Modify: `src/lib.rs` (add `pub mod shape;`)
- Test: `tests/shape.rs`

**Interfaces:**
- Consumes: nothing.
- Produces:
  - `shape::EnumIndex` — `EnumIndex::new() -> EnumIndex`, `EnumIndex::learn(&mut self, tool: &str, schema: &serde_json::Value)`, `EnumIndex::is_enum(&self, tool: &str, path: &str, value: &str) -> bool`
  - `shape::of(value: &serde_json::Value, tool: &str, enums: &EnumIndex) -> serde_json::Value` — returns the shape document for an arguments object.

Shape rules, exhaustive:

| Input | Recorded as |
| --- | --- |
| `null` | `"null"` |
| bool | `"bool:true"` / `"bool:false"` |
| number | `"num:<value>"` |
| string matching a schema enum at that path | `"enum:<value>"` |
| string parsing as a UUID | `"uuid"` |
| string parsing as an http(s) URL | `"url:<registrable domain>"` |
| any other string | `"str<<bucket>"` where bucket is the smallest of 8, 32, 128, 512, 4096, or `"str>4096"` |
| array | `{"array": <len>, "items": <shape of first element or "empty">}` |
| object | `{ key: <shape>, ... }` recursed |

Registrable domain here means host with a leading `www.` stripped; no attempt at a public-suffix list, because the value is only used to group findings.

- [ ] **Step 1: Write the failing test**

`tests/shape.rs`:

```rust
use mcpeval::shape::{self, EnumIndex};
use serde_json::json;

fn enums_with_wait_until() -> EnumIndex {
    let mut idx = EnumIndex::new();
    idx.learn(
        "navigate",
        &json!({
            "type": "object",
            "properties": {
                "waitUntil": { "type": "string", "enum": ["commit", "networkIdle"] },
                "url": { "type": "string" }
            }
        }),
    );
    idx
}

#[test]
fn scalars_and_strings_are_shaped_not_stored() {
    let idx = EnumIndex::new();
    let out = shape::of(
        &json!({ "note": "call me at 555-0100", "count": 3, "ok": true, "nothing": null }),
        "anything",
        &idx,
    );
    assert_eq!(out["note"], "str<32");
    assert_eq!(out["count"], "num:3");
    assert_eq!(out["ok"], "bool:true");
    assert_eq!(out["nothing"], "null");
    let text = out.to_string();
    assert!(!text.contains("555"), "payload leaked into shape: {text}");
}

#[test]
fn schema_declared_enums_keep_their_value() {
    let idx = enums_with_wait_until();
    let out = shape::of(&json!({ "waitUntil": "networkIdle" }), "navigate", &idx);
    assert_eq!(out["waitUntil"], "enum:networkIdle");
}

#[test]
fn a_string_that_matches_an_enum_of_a_different_tool_is_not_kept() {
    let idx = enums_with_wait_until();
    let out = shape::of(&json!({ "waitUntil": "networkIdle" }), "click", &idx);
    assert_eq!(out["waitUntil"], "str<32");
}

#[test]
fn urls_keep_only_the_domain() {
    let idx = EnumIndex::new();
    let out = shape::of(
        &json!({ "url": "https://www.example.com/a/secret/path?token=abc" }),
        "navigate",
        &idx,
    );
    assert_eq!(out["url"], "url:example.com");
    let text = out.to_string();
    assert!(!text.contains("secret") && !text.contains("token"));
}

#[test]
fn uuids_are_labelled_not_stored() {
    let idx = EnumIndex::new();
    let out = shape::of(
        &json!({ "sessionId": "0be9b59c-af70-47b0-9169-d9de92330600" }),
        "click",
        &idx,
    );
    assert_eq!(out["sessionId"], "uuid");
}

#[test]
fn nested_objects_and_arrays_recurse() {
    let idx = EnumIndex::new();
    let out = shape::of(
        &json!({ "target": { "role": "button", "ordinal": 1 }, "paths": ["/a/b.pdf", "/c.pdf"] }),
        "click",
        &idx,
    );
    assert_eq!(out["target"]["role"], "str<8");
    assert_eq!(out["target"]["ordinal"], "num:1");
    assert_eq!(out["paths"]["array"], 2);
    assert_eq!(out["paths"]["items"], "str<8");
}

#[test]
fn long_strings_bucket_upward() {
    let idx = EnumIndex::new();
    let long = "x".repeat(200);
    let out = shape::of(&json!({ "essay": long }), "t", &idx);
    assert_eq!(out["essay"], "str<512");
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --test shape`
Expected: FAIL — `mcpeval::shape` does not exist.

- [ ] **Step 3: Write minimal implementation**

`src/shape.rs`:

```rust
use std::collections::{HashMap, HashSet};

use serde_json::{json, Map, Value};

/// Enum values declared by each tool's own input schema, keyed by tool then
/// dotted property path. Only values learned here are ever stored verbatim.
#[derive(Debug, Default)]
pub struct EnumIndex {
    by_tool: HashMap<String, HashMap<String, HashSet<String>>>,
}

impl EnumIndex {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn learn(&mut self, tool: &str, schema: &Value) {
        let mut found: HashMap<String, HashSet<String>> = HashMap::new();
        walk_schema(schema, String::new(), &mut found);
        self.by_tool.insert(tool.to_string(), found);
    }

    pub fn is_enum(&self, tool: &str, path: &str, value: &str) -> bool {
        self.by_tool
            .get(tool)
            .and_then(|paths| paths.get(path))
            .is_some_and(|vals| vals.contains(value))
    }
}

fn walk_schema(schema: &Value, path: String, out: &mut HashMap<String, HashSet<String>>) {
    if let Some(values) = schema.get("enum").and_then(Value::as_array) {
        let set: HashSet<String> = values
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect();
        if !set.is_empty() {
            out.insert(path.clone(), set);
        }
    }
    for key in ["oneOf", "anyOf", "allOf"] {
        if let Some(list) = schema.get(key).and_then(Value::as_array) {
            for branch in list {
                walk_schema(branch, path.clone(), out);
            }
        }
    }
    if let Some(props) = schema.get("properties").and_then(Value::as_object) {
        for (name, sub) in props {
            let child = if path.is_empty() {
                name.clone()
            } else {
                format!("{path}.{name}")
            };
            walk_schema(sub, child, out);
        }
    }
    if let Some(items) = schema.get("items") {
        walk_schema(items, format!("{path}[]"), out);
    }
}

pub fn of(value: &Value, tool: &str, enums: &EnumIndex) -> Value {
    shape_at(value, tool, "", enums)
}

fn shape_at(value: &Value, tool: &str, path: &str, enums: &EnumIndex) -> Value {
    match value {
        Value::Null => json!("null"),
        Value::Bool(b) => json!(format!("bool:{b}")),
        Value::Number(n) => json!(format!("num:{n}")),
        Value::String(s) => json!(string_shape(s, tool, path, enums)),
        Value::Array(items) => {
            let first = items
                .first()
                .map(|v| shape_at(v, tool, &format!("{path}[]"), enums))
                .unwrap_or_else(|| json!("empty"));
            json!({ "array": items.len(), "items": first })
        }
        Value::Object(map) => {
            let mut out = Map::new();
            for (key, sub) in map {
                let child = if path.is_empty() {
                    key.clone()
                } else {
                    format!("{path}.{key}")
                };
                out.insert(key.clone(), shape_at(sub, tool, &child, enums));
            }
            Value::Object(out)
        }
    }
}

fn string_shape(s: &str, tool: &str, path: &str, enums: &EnumIndex) -> String {
    if enums.is_enum(tool, path, s) {
        return format!("enum:{s}");
    }
    if is_uuid(s) {
        return "uuid".to_string();
    }
    if let Some(domain) = registrable_domain(s) {
        return format!("url:{domain}");
    }
    for bucket in [8usize, 32, 128, 512, 4096] {
        if s.len() <= bucket {
            return format!("str<{bucket}");
        }
    }
    "str>4096".to_string()
}

fn is_uuid(s: &str) -> bool {
    uuid::Uuid::parse_str(s).is_ok()
}

fn registrable_domain(s: &str) -> Option<String> {
    let rest = s
        .strip_prefix("https://")
        .or_else(|| s.strip_prefix("http://"))?;
    let host = rest
        .split(['/', '?', '#'])
        .next()
        .unwrap_or("")
        .split('@')
        .next_back()
        .unwrap_or("")
        .split(':')
        .next()
        .unwrap_or("");
    if host.is_empty() {
        return None;
    }
    Some(host.strip_prefix("www.").unwrap_or(host).to_string())
}
```

`src/lib.rs`:

```rust
pub mod frame;
pub mod shape;
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test --test shape`
Expected: PASS (7 tests).

- [ ] **Step 5: Commit**

```bash
git add src/shape.rs src/lib.rs tests/shape.rs
git commit -m "Add shape extraction with schema-declared enum allowlist"
```

---

### Task 4: Error templating

**Files:**
- Create: `src/errtemplate.rs`
- Modify: `src/lib.rs` (add `pub mod errtemplate;`)
- Test: `tests/errtemplate.rs`

**Interfaces:**
- Consumes: nothing.
- Produces: `errtemplate::normalize(message: &str) -> String`.

Substitution order matters: UUIDs, then quoted strings, then absolute paths, then URLs, then bare numbers. Each becomes a named placeholder so two messages differing only in a value collapse to one template — which is what makes counting an issue across sessions possible.

- [ ] **Step 1: Write the failing test**

`tests/errtemplate.rs`:

```rust
use mcpeval::errtemplate::normalize;

#[test]
fn uuids_become_placeholders() {
    let a = normalize("session 0be9b59c-af70-47b0-9169-d9de92330600 not found");
    let b = normalize("session f5a8fb32-922f-4f72-b09a-474045fd0094 not found");
    assert_eq!(a, "session {uuid} not found");
    assert_eq!(a, b);
}

#[test]
fn numbers_become_placeholders() {
    assert_eq!(normalize("journal line 6 is corrupt"), "journal line {n} is corrupt");
    assert_eq!(
        normalize("journal line 958 is corrupt"),
        "journal line {n} is corrupt"
    );
}

#[test]
fn quoted_strings_become_placeholders() {
    assert_eq!(
        normalize("Cannot upload \"/Users/someone/private.pdf\": not shared"),
        "Cannot upload {str}: not shared"
    );
}

#[test]
fn absolute_paths_become_placeholders() {
    assert_eq!(
        normalize("descriptor at /Users/someone/Library/x.json missing"),
        "descriptor at {path} missing"
    );
}

#[test]
fn urls_become_placeholders() {
    assert_eq!(
        normalize("ws://127.0.0.1:9222/session unreachable"),
        "{url} unreachable"
    );
}

#[test]
fn a_message_with_no_variables_is_unchanged() {
    let m = "extension observation failed: the content action failed";
    assert_eq!(normalize(m), m);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --test errtemplate`
Expected: FAIL — module does not exist.

- [ ] **Step 3: Write minimal implementation**

`src/errtemplate.rs`:

```rust
use std::sync::OnceLock;

use regex::Regex;

struct Patterns {
    uuid: Regex,
    quoted: Regex,
    url: Regex,
    path: Regex,
    number: Regex,
}

fn patterns() -> &'static Patterns {
    static P: OnceLock<Patterns> = OnceLock::new();
    P.get_or_init(|| Patterns {
        uuid: Regex::new(
            r"(?i)\b[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}\b",
        )
        .expect("uuid pattern"),
        quoted: Regex::new(r#""[^"]*"|'[^']*'"#).expect("quoted pattern"),
        url: Regex::new(r"\b[a-z][a-z0-9+.-]*://[^\s]+").expect("url pattern"),
        path: Regex::new(r"(?:^|\s)(/[^\s:]+)").expect("path pattern"),
        number: Regex::new(r"\b\d+\b").expect("number pattern"),
    })
}

/// Collapses a human error message to a stable template. Values are replaced
/// with named placeholders, so the same defect aggregates across sessions.
pub fn normalize(message: &str) -> String {
    let p = patterns();
    let out = p.uuid.replace_all(message, "{uuid}").into_owned();
    let out = p.quoted.replace_all(&out, "{str}").into_owned();
    let out = p.url.replace_all(&out, "{url}").into_owned();
    let out = p
        .path
        .replace_all(&out, |caps: &regex::Captures| {
            let lead = if caps[0].starts_with(' ') { " " } else { "" };
            format!("{lead}{{path}}")
        })
        .into_owned();
    p.number.replace_all(&out, "{n}").into_owned()
}
```

`src/lib.rs`:

```rust
pub mod errtemplate;
pub mod frame;
pub mod shape;
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test --test errtemplate`
Expected: PASS (6 tests).

- [ ] **Step 5: Commit**

```bash
git add src/errtemplate.rs src/lib.rs tests/errtemplate.rs
git commit -m "Add error message templating"
```

---

### Task 5: Record model and JSONL writer

**Files:**
- Create: `src/record.rs`
- Create: `src/store.rs`
- Modify: `src/lib.rs`
- Test: `tests/store.rs`

**Interfaces:**
- Consumes: `shape`, `errtemplate`.
- Produces:
  - `record::CallRecord` with fields `ts: String, session: String, seq: u64, server: String, method: String, tool: Option<String>, args: Option<serde_json::Value>, latency_ms: Option<u64>, outcome: String, error: Option<record::ErrorInfo>, shim_self_us: u64, kind: String`
  - `record::ErrorInfo { code: Option<serde_json::Value>, layer: Option<String>, retryable: Option<bool>, kind: Option<String>, template: Option<String> }`
  - `record::error_info(payload: &serde_json::Value) -> ErrorInfo`
  - `store::Store::open(root: Option<PathBuf>) -> anyhow::Result<Store>`, `Store::append(&mut self, rec: &CallRecord) -> anyhow::Result<()>`, `Store::root(&self) -> &Path`

`outcome` is one of `ok`, `error`, `unparsed`, `notification`. `kind` is `real` or `synthetic`. Only the four named keys are lifted out of an error object; every other key is dropped.

- [ ] **Step 1: Write the failing test**

`tests/store.rs`:

```rust
use mcpeval::record::{error_info, CallRecord};
use mcpeval::store::Store;
use serde_json::json;

fn sample(seq: u64) -> CallRecord {
    CallRecord {
        ts: "2026-08-04T12:00:00Z".into(),
        session: "11111111-1111-4111-8111-111111111111".into(),
        seq,
        server: "demo".into(),
        method: "tools/call".into(),
        tool: Some("click".into()),
        args: Some(json!({ "sessionId": "uuid" })),
        latency_ms: Some(12),
        outcome: "ok".into(),
        error: None,
        shim_self_us: 40,
        kind: "real".into(),
    }
}

#[test]
fn appends_one_json_line_per_record() {
    let dir = tempdir();
    let mut store = Store::open(Some(dir.clone())).unwrap();
    store.append(&sample(1)).unwrap();
    store.append(&sample(2)).unwrap();

    let files: Vec<_> = std::fs::read_dir(dir.join("store")).unwrap().collect();
    assert_eq!(files.len(), 1, "one daily file expected");

    let path = files.into_iter().next().unwrap().unwrap().path();
    let body = std::fs::read_to_string(path).unwrap();
    let lines: Vec<&str> = body.lines().collect();
    assert_eq!(lines.len(), 2);
    let first: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
    assert_eq!(first["seq"], 1);
    assert_eq!(first["tool"], "click");
}

#[test]
fn error_info_keeps_only_the_four_allowed_keys_and_a_template() {
    let payload = json!({
        "code": "browserCommandFailed",
        "layer": "driver",
        "retryable": false,
        "message": "session 0be9b59c-af70-47b0-9169-d9de92330600 died",
        "correlationId": "9c27f579-94a7-4df3-a59b-576aa4a12317",
        "stack": "secret internals"
    });
    let info = error_info(&payload);
    assert_eq!(info.code.unwrap(), json!("browserCommandFailed"));
    assert_eq!(info.layer.unwrap(), "driver");
    assert_eq!(info.retryable.unwrap(), false);
    assert_eq!(info.template.unwrap(), "session {uuid} died");

    let text = serde_json::to_string(&error_info(&payload)).unwrap();
    assert!(!text.contains("secret internals"));
    assert!(!text.contains("correlationId"));
}

fn tempdir() -> std::path::PathBuf {
    let base = std::env::temp_dir().join(format!("mcpeval-test-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&base).unwrap();
    base
}
```

Add `uuid` and `tempfile`-free helper usage: the test uses `uuid`, already a dependency, and needs it available to tests — add `uuid` to `[dev-dependencies]` as well in Step 3.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --test store`
Expected: FAIL — `mcpeval::record` and `mcpeval::store` do not exist.

- [ ] **Step 3: Write minimal implementation**

`Cargo.toml`, add:

```toml
[dev-dependencies]
uuid = { version = "1", features = ["v4"] }
```

`src/record.rs`:

```rust
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::errtemplate;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CallRecord {
    pub ts: String,
    pub session: String,
    pub seq: u64,
    pub server: String,
    pub method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub args: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latency_ms: Option<u64>,
    pub outcome: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<ErrorInfo>,
    pub shim_self_us: u64,
    pub kind: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ErrorInfo {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub layer: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retryable: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub template: Option<String>,
}

/// Lifts the four allowed keys out of an error payload and templates its
/// message. Every other key is dropped, at any depth.
pub fn error_info(payload: &Value) -> ErrorInfo {
    let inner = payload.get("error").unwrap_or(payload);
    ErrorInfo {
        code: inner.get("code").cloned(),
        layer: inner.get("layer").and_then(Value::as_str).map(str::to_string),
        retryable: inner.get("retryable").and_then(Value::as_bool),
        kind: inner.get("kind").and_then(Value::as_str).map(str::to_string),
        template: inner
            .get("message")
            .and_then(Value::as_str)
            .map(errtemplate::normalize),
    }
}
```

`src/store.rs`:

```rust
use std::fs::{create_dir_all, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::Context;

use crate::record::CallRecord;

pub struct Store {
    root: PathBuf,
}

impl Store {
    pub fn open(root: Option<PathBuf>) -> anyhow::Result<Self> {
        let root = root
            .or_else(|| std::env::var_os("MCPEVAL_HOME").map(PathBuf::from))
            .unwrap_or_else(|| {
                let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
                home.join(".mcp-eval")
            });
        create_dir_all(root.join("store")).context("creating store directory")?;
        Ok(Self { root })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn append(&mut self, rec: &CallRecord) -> anyhow::Result<()> {
        let day = rec.ts.get(..10).unwrap_or("unknown");
        let path = self.root.join("store").join(format!("calls-{day}.jsonl"));
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .with_context(|| format!("opening {}", path.display()))?;
        let line = serde_json::to_string(rec)?;
        file.write_all(line.as_bytes())?;
        file.write_all(b"\n")?;
        Ok(())
    }
}
```

`src/lib.rs`:

```rust
pub mod errtemplate;
pub mod frame;
pub mod record;
pub mod shape;
pub mod store;
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test --test store`
Expected: PASS (2 tests).

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml Cargo.lock src/record.rs src/store.rs src/lib.rs tests/store.rs
git commit -m "Add call record model and JSONL store"
```

---

### Task 6: Call correlation

**Files:**
- Create: `src/correlate.rs`
- Modify: `src/lib.rs`
- Test: `tests/correlate.rs`

**Interfaces:**
- Consumes: `shape::EnumIndex`, `record::CallRecord`, `record::error_info`.
- Produces: `correlate::Correlator` with
  - `Correlator::new(server: String, session: String) -> Correlator`
  - `Correlator::on_outbound(&mut self, v: &serde_json::Value, now_ms: u64)` — agent → server
  - `Correlator::on_inbound(&mut self, v: &serde_json::Value, now_ms: u64) -> Option<CallRecord>` — server → agent; emits a record when a response matches a pending request
  - `Correlator::on_unparsed(&mut self, direction: &str, now_ms: u64) -> CallRecord`

Requests are matched to responses by JSON-RPC `id`. A `tools/list` response teaches the `EnumIndex`, which is why enum values are only recognised after discovery — a deliberate ordering, since a value is only safe to store once the server's own schema has declared it. Notifications (no `id`) emit immediately with `outcome: "notification"`.

- [ ] **Step 1: Write the failing test**

`tests/correlate.rs`:

```rust
use mcpeval::correlate::Correlator;
use serde_json::json;

#[test]
fn matches_a_response_to_its_request_and_measures_latency() {
    let mut c = Correlator::new("demo".into(), "sess".into());
    c.on_outbound(
        &json!({ "jsonrpc": "2.0", "id": 7, "method": "tools/call",
                 "params": { "name": "click", "arguments": { "note": "hello" } } }),
        1_000,
    );
    let rec = c
        .on_inbound(&json!({ "jsonrpc": "2.0", "id": 7, "result": { "ok": true } }), 1_250)
        .expect("a matched response emits a record");

    assert_eq!(rec.method, "tools/call");
    assert_eq!(rec.tool.as_deref(), Some("click"));
    assert_eq!(rec.latency_ms, Some(250));
    assert_eq!(rec.outcome, "ok");
    assert_eq!(rec.args.unwrap()["note"], "str<8");
}

#[test]
fn an_error_response_records_code_and_template_only() {
    let mut c = Correlator::new("demo".into(), "sess".into());
    c.on_outbound(
        &json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call",
                 "params": { "name": "navigate", "arguments": {} } }),
        0,
    );
    let rec = c
        .on_inbound(
            &json!({ "jsonrpc": "2.0", "id": 1,
                     "error": { "code": -32000, "message": "session 0be9b59c-af70-47b0-9169-d9de92330600 gone" } }),
            10,
        )
        .unwrap();

    assert_eq!(rec.outcome, "error");
    let err = rec.error.unwrap();
    assert_eq!(err.code.unwrap(), json!(-32000));
    assert_eq!(err.template.unwrap(), "session {uuid} gone");
}

#[test]
fn tools_list_teaches_enums_used_by_later_calls() {
    let mut c = Correlator::new("demo".into(), "sess".into());
    c.on_outbound(&json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list" }), 0);
    c.on_inbound(
        &json!({ "jsonrpc": "2.0", "id": 1, "result": { "tools": [
            { "name": "navigate", "inputSchema": { "type": "object", "properties": {
                "waitUntil": { "type": "string", "enum": ["commit", "networkIdle"] } } } }
        ] } }),
        5,
    );

    c.on_outbound(
        &json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/call",
                 "params": { "name": "navigate", "arguments": { "waitUntil": "networkIdle" } } }),
        6,
    );
    let rec = c
        .on_inbound(&json!({ "jsonrpc": "2.0", "id": 2, "result": {} }), 7)
        .unwrap();
    assert_eq!(rec.args.unwrap()["waitUntil"], "enum:networkIdle");
}

#[test]
fn notifications_emit_immediately() {
    let mut c = Correlator::new("demo".into(), "sess".into());
    let rec = c
        .on_inbound(&json!({ "jsonrpc": "2.0", "method": "notifications/message" }), 3)
        .expect("a notification emits its own record");
    assert_eq!(rec.outcome, "notification");
    assert_eq!(rec.method, "notifications/message");
}

#[test]
fn sequence_numbers_increase_per_record() {
    let mut c = Correlator::new("demo".into(), "sess".into());
    c.on_outbound(&json!({ "jsonrpc": "2.0", "id": 1, "method": "ping" }), 0);
    let a = c.on_inbound(&json!({ "jsonrpc": "2.0", "id": 1, "result": {} }), 1).unwrap();
    c.on_outbound(&json!({ "jsonrpc": "2.0", "id": 2, "method": "ping" }), 2);
    let b = c.on_inbound(&json!({ "jsonrpc": "2.0", "id": 2, "result": {} }), 3).unwrap();
    assert!(b.seq > a.seq);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --test correlate`
Expected: FAIL — `mcpeval::correlate` does not exist.

- [ ] **Step 3: Write minimal implementation**

`src/correlate.rs`:

```rust
use std::collections::HashMap;

use serde_json::Value;

use crate::record::{error_info, CallRecord};
use crate::shape::{self, EnumIndex};

struct Pending {
    method: String,
    tool: Option<String>,
    args: Option<Value>,
    sent_ms: u64,
}

pub struct Correlator {
    server: String,
    session: String,
    seq: u64,
    enums: EnumIndex,
    pending: HashMap<String, Pending>,
}

impl Correlator {
    pub fn new(server: String, session: String) -> Self {
        Self {
            server,
            session,
            seq: 0,
            enums: EnumIndex::new(),
            pending: HashMap::new(),
        }
    }

    pub fn on_outbound(&mut self, v: &Value, now_ms: u64) {
        let Some(id) = id_key(v) else { return };
        let method = v.get("method").and_then(Value::as_str).unwrap_or("").to_string();
        let params = v.get("params");
        let tool = params
            .and_then(|p| p.get("name"))
            .and_then(Value::as_str)
            .map(str::to_string);
        let args = params.and_then(|p| p.get("arguments")).map(|a| {
            shape::of(a, tool.as_deref().unwrap_or(""), &self.enums)
        });
        self.pending.insert(id, Pending { method, tool, args, sent_ms: now_ms });
    }

    pub fn on_inbound(&mut self, v: &Value, now_ms: u64) -> Option<CallRecord> {
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
            Some(id) => {
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
                    outcome: if is_error { "error".into() } else { "ok".into() },
                    error: if is_error { Some(error_info(v)) } else { None },
                    shim_self_us: 0,
                    kind: "real".into(),
                })
            }
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
    chrono::Utc::now().format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string()
}
```

`src/lib.rs`:

```rust
pub mod correlate;
pub mod errtemplate;
pub mod frame;
pub mod record;
pub mod shape;
pub mod store;
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test --test correlate`
Expected: PASS (5 tests).

- [ ] **Step 5: Commit**

```bash
git add src/correlate.rs src/lib.rs tests/correlate.rs
git commit -m "Add request/response correlation and enum learning"
```

---

### Task 7: The shim

**Files:**
- Create: `src/shim.rs`
- Modify: `src/lib.rs`, `src/main.rs`
- Test: `tests/shim.rs`
- Create: `tests/fixtures/echo_server.py`

**Interfaces:**
- Consumes: `frame`, `correlate`, `store`.
- Produces: `shim::run(server: String, cmd: Vec<String>) -> anyhow::Result<i32>` — returns the child's exit code.

Two threads pump bytes: agent stdin → child stdin, and child stdout → agent stdout. Forwarding happens first and is flushed before any recording work, so store I/O never sits between the agent and the server. Child stderr is inherited, untouched — server logs keep going where they always went.

- [ ] **Step 1: Write the failing test**

`tests/fixtures/echo_server.py`:

```python
#!/usr/bin/env python3
"""Minimal MCP-shaped stdio server for shim tests.

Answers tools/list with one tool, echoes tools/call as a result, and returns a
JSON-RPC error for the tool named "boom".
"""
import json
import sys

TOOLS = [{
    "name": "navigate",
    "inputSchema": {
        "type": "object",
        "properties": {"waitUntil": {"type": "string", "enum": ["commit", "networkIdle"]}},
    },
}]

for line in sys.stdin:
    line = line.strip()
    if not line:
        continue
    msg = json.loads(line)
    mid = msg.get("id")
    method = msg.get("method")
    if method == "tools/list":
        out = {"jsonrpc": "2.0", "id": mid, "result": {"tools": TOOLS}}
    elif method == "tools/call" and msg.get("params", {}).get("name") == "boom":
        out = {"jsonrpc": "2.0", "id": mid,
               "error": {"code": -32000, "message": "session 0be9b59c-af70-47b0-9169-d9de92330600 gone"}}
    else:
        out = {"jsonrpc": "2.0", "id": mid, "result": {"echo": True}}
    sys.stdout.write(json.dumps(out) + "\n")
    sys.stdout.flush()
```

`tests/shim.rs`:

```rust
use std::io::Write;
use std::process::{Command, Stdio};

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_mcpeval")
}

fn tempdir() -> std::path::PathBuf {
    let base = std::env::temp_dir().join(format!("mcpeval-shim-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&base).unwrap();
    base
}

fn read_records(home: &std::path::Path) -> Vec<serde_json::Value> {
    let dir = home.join("store");
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir).unwrap() {
        let body = std::fs::read_to_string(entry.unwrap().path()).unwrap();
        for line in body.lines() {
            out.push(serde_json::from_str(line).unwrap());
        }
    }
    out
}

#[test]
fn proxies_messages_unchanged_and_records_them() {
    let home = tempdir();
    let mut child = Command::new(bin())
        .args(["shim", "--server", "demo", "--",
               "python3", "tests/fixtures/echo_server.py"])
        .env("MCPEVAL_HOME", &home)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();

    let mut stdin = child.stdin.take().unwrap();
    writeln!(stdin, r#"{{"jsonrpc":"2.0","id":1,"method":"tools/list"}}"#).unwrap();
    writeln!(
        stdin,
        r#"{{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{{"name":"navigate","arguments":{{"waitUntil":"networkIdle","url":"https://www.example.com/secret?token=abc"}}}}}}"#
    )
    .unwrap();
    writeln!(
        stdin,
        r#"{{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{{"name":"boom","arguments":{{}}}}}}"#
    )
    .unwrap();
    drop(stdin);

    let out = child.wait_with_output().unwrap();
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert_eq!(stdout.lines().count(), 3, "every response must reach the agent");
    assert!(stdout.contains(r#""echo":true"#));

    let records = read_records(&home);
    assert_eq!(records.len(), 3);

    let call = records.iter().find(|r| r["tool"] == "navigate").unwrap();
    assert_eq!(call["args"]["waitUntil"], "enum:networkIdle");
    assert_eq!(call["args"]["url"], "url:example.com");
    assert_eq!(call["outcome"], "ok");

    let failed = records.iter().find(|r| r["outcome"] == "error").unwrap();
    assert_eq!(failed["error"]["template"], "session {uuid} gone");
}

#[test]
fn no_payload_content_reaches_disk() {
    let home = tempdir();
    let mut child = Command::new(bin())
        .args(["shim", "--server", "demo", "--",
               "python3", "tests/fixtures/echo_server.py"])
        .env("MCPEVAL_HOME", &home)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();

    let mut stdin = child.stdin.take().unwrap();
    writeln!(
        stdin,
        r#"{{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{{"name":"x","arguments":{{"secret":"CANARY-8f3a","path":"/Users/someone/private.pdf","url":"https://example.com/a?token=CANARY-9b2"}}}}}}"#
    )
    .unwrap();
    drop(stdin);
    child.wait().unwrap();

    let dir = home.join("store");
    for entry in std::fs::read_dir(dir).unwrap() {
        let body = std::fs::read_to_string(entry.unwrap().path()).unwrap();
        assert!(!body.contains("CANARY"), "planted secret reached disk: {body}");
        assert!(!body.contains("private.pdf"), "path reached disk: {body}");
        assert!(!body.contains("token"), "query string reached disk: {body}");
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --test shim`
Expected: FAIL — `shim not implemented yet`.

- [ ] **Step 3: Write minimal implementation**

`src/shim.rs`:

```rust
use std::io::{BufReader, Write};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{channel, Sender};
use std::thread;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use anyhow::Context;

use crate::correlate::Correlator;
use crate::frame::read_frame;
use crate::record::CallRecord;
use crate::store::Store;

enum Event {
    Outbound(serde_json::Value, u64),
    Inbound(serde_json::Value, u64),
    Unparsed(&'static str, u64),
    Done,
}

pub fn run(server: String, cmd: Vec<String>) -> anyhow::Result<i32> {
    let (program, args) = cmd.split_first().context("empty server command")?;
    let mut child: Child = Command::new(program)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .with_context(|| format!("spawning {program}"))?;

    let mut child_stdin = child.stdin.take().context("child stdin")?;
    let child_stdout = child.stdout.take().context("child stdout")?;

    let session = std::env::var("MCPEVAL_SESSION")
        .unwrap_or_else(|_| uuid::Uuid::new_v4().to_string());
    let (tx, rx) = channel::<Event>();

    let writer = {
        let tx = tx.clone();
        thread::spawn(move || {
            let stdin = std::io::stdin();
            let mut reader = BufReader::new(stdin.lock());
            while let Ok(Some(f)) = read_frame(&mut reader) {
                let started = Instant::now();
                let _ = child_stdin.write_all(&f.raw);
                let _ = child_stdin.flush();
                emit(&tx, &f.value, "outbound", started);
            }
            let _ = tx.send(Event::Done);
        })
    };

    let reader = {
        let tx = tx.clone();
        thread::spawn(move || {
            let mut reader = BufReader::new(child_stdout);
            let stdout = std::io::stdout();
            while let Ok(Some(f)) = read_frame(&mut reader) {
                let started = Instant::now();
                let mut out = stdout.lock();
                let _ = out.write_all(&f.raw);
                let _ = out.flush();
                drop(out);
                emit(&tx, &f.value, "inbound", started);
            }
            let _ = tx.send(Event::Done);
        })
    };

    drop(tx);

    let mut store = Store::open(None)?;
    let mut correlator = Correlator::new(server, session);
    let mut finished = 0;
    for event in rx {
        let (rec, self_us): (Option<CallRecord>, u64) = match event {
            Event::Outbound(v, us) => {
                correlator.on_outbound(&v, now_ms());
                (None, us)
            }
            Event::Inbound(v, us) => (correlator.on_inbound(&v, now_ms()), us),
            Event::Unparsed(dir, us) => (Some(correlator.on_unparsed(dir, now_ms())), us),
            Event::Done => {
                finished += 1;
                if finished == 2 {
                    break;
                }
                (None, 0)
            }
        };
        if let Some(mut rec) = rec {
            rec.shim_self_us = self_us;
            let _ = store.append(&rec);
        }
    }

    let _ = writer.join();
    let _ = reader.join();
    let status = child.wait()?;
    Ok(status.code().unwrap_or(0))
}

fn emit(tx: &Sender<Event>, value: &Option<serde_json::Value>, dir: &'static str, started: Instant) {
    let self_us = started.elapsed().as_micros() as u64;
    let _ = match (value, dir) {
        (Some(v), "outbound") => tx.send(Event::Outbound(v.clone(), self_us)),
        (Some(v), _) => tx.send(Event::Inbound(v.clone(), self_us)),
        (None, _) => tx.send(Event::Unparsed(dir, self_us)),
    };
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}
```

`src/lib.rs`, add `pub mod shim;`.

`src/main.rs`:

```rust
mod cli;

use clap::Parser;

fn main() -> anyhow::Result<()> {
    let cli = cli::Cli::parse();
    match cli.command {
        cli::Command::Shim { server, cmd } => {
            let code = mcpeval::shim::run(server, cmd)?;
            std::process::exit(code);
        }
        cli::Command::Index => anyhow::bail!("index not implemented yet"),
    }
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test --test shim`
Expected: PASS (2 tests).

- [ ] **Step 5: Commit**

```bash
git add src/shim.rs src/lib.rs src/main.rs tests/shim.rs tests/fixtures/echo_server.py
git commit -m "Add recording stdio shim"
```

---

### Task 8: Overhead budget test

**Files:**
- Test: `tests/overhead.rs`

**Interfaces:**
- Consumes: the `mcpeval` binary and `tests/fixtures/echo_server.py` from Task 7.
- Produces: nothing consumed by later tasks.

The budget is p95 under 2ms of *added* latency. The test measures the same 200 round trips twice — once directly against the fixture server, once through the shim — and compares p95.

- [ ] **Step 1: Write the failing test**

`tests/overhead.rs`:

```rust
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::time::Instant;

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_mcpeval")
}

fn p95(mut samples: Vec<u128>) -> u128 {
    samples.sort_unstable();
    let idx = ((samples.len() as f64 * 0.95) as usize).min(samples.len() - 1);
    samples[idx]
}

fn round_trips(stdin: &mut ChildStdin, stdout: &mut BufReader<ChildStdout>, n: usize) -> Vec<u128> {
    let mut samples = Vec::with_capacity(n);
    for i in 0..n {
        let msg = format!(
            r#"{{"jsonrpc":"2.0","id":{i},"method":"tools/call","params":{{"name":"navigate","arguments":{{"waitUntil":"commit"}}}}}}"#
        );
        let started = Instant::now();
        writeln!(stdin, "{msg}").unwrap();
        stdin.flush().unwrap();
        let mut line = String::new();
        stdout.read_line(&mut line).unwrap();
        samples.push(started.elapsed().as_micros());
    }
    samples
}

fn spawn(direct: bool, home: &std::path::Path) -> Child {
    if direct {
        Command::new("python3")
            .args(["tests/fixtures/echo_server.py"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap()
    } else {
        Command::new(bin())
            .args(["shim", "--server", "bench", "--",
                   "python3", "tests/fixtures/echo_server.py"])
            .env("MCPEVAL_HOME", home)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap()
    }
}

#[test]
fn shim_adds_under_two_milliseconds_at_p95() {
    let home = std::env::temp_dir().join(format!("mcpeval-bench-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&home).unwrap();

    let mut direct = spawn(true, &home);
    let mut d_in = direct.stdin.take().unwrap();
    let mut d_out = BufReader::new(direct.stdout.take().unwrap());
    let baseline = p95(round_trips(&mut d_in, &mut d_out, 200));
    drop(d_in);
    let _ = direct.wait();

    let mut shimmed = spawn(false, &home);
    let mut s_in = shimmed.stdin.take().unwrap();
    let mut s_out = BufReader::new(shimmed.stdout.take().unwrap());
    let through = p95(round_trips(&mut s_in, &mut s_out, 200));
    drop(s_in);
    let _ = shimmed.wait();

    let added = through.saturating_sub(baseline);
    assert!(
        added < 2_000,
        "shim added {added}us at p95 (baseline {baseline}us, shimmed {through}us)"
    );
}
```

- [ ] **Step 2: Run test to verify it fails or passes for the right reason**

Run: `cargo test --test overhead --release`
Expected: PASS. If it FAILS, the shim is doing work on the forwarding path — the store append must happen on the collector thread, never between read and write. Fix that rather than raising the budget.

- [ ] **Step 3: No implementation step**

This task adds a guard, not a feature. If Step 2 passed, proceed.

- [ ] **Step 4: Re-run the whole suite**

Run: `cargo test --release`
Expected: PASS, all tests.

- [ ] **Step 5: Commit**

```bash
git add tests/overhead.rs
git commit -m "Add shim overhead budget test"
```

---

### Task 9: SQLite index and failure windows

**Files:**
- Create: `src/index.rs`
- Modify: `src/lib.rs`, `src/main.rs`
- Test: `tests/index.rs`

**Interfaces:**
- Consumes: `store::Store`, `record::CallRecord`.
- Produces:
  - `index::build(root: &Path) -> anyhow::Result<index::Stats>`
  - `index::Stats { pub calls: usize, pub failures: usize }`
  - SQLite at `<root>/index.db` with tables `calls` and `windows`.

The window is derived here rather than in the shim: the shim would have to hold a record open until three later calls arrived, which puts state on the hot path for no benefit. `windows` holds one row per (failure, neighbour) pair with a signed offset — negative for the five before, positive for the three after, within the same session.

- [ ] **Step 1: Write the failing test**

`tests/index.rs`:

```rust
use mcpeval::index;
use mcpeval::record::CallRecord;
use mcpeval::store::Store;

fn rec(seq: u64, outcome: &str) -> CallRecord {
    CallRecord {
        ts: "2026-08-04T12:00:00Z".into(),
        session: "s1".into(),
        seq,
        server: "demo".into(),
        method: "tools/call".into(),
        tool: Some(format!("tool{seq}")),
        args: None,
        latency_ms: Some(5),
        outcome: outcome.into(),
        error: None,
        shim_self_us: 10,
        kind: "real".into(),
    }
}

fn tempdir() -> std::path::PathBuf {
    let base = std::env::temp_dir().join(format!("mcpeval-index-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&base).unwrap();
    base
}

#[test]
fn loads_records_and_derives_a_failure_window() {
    let dir = tempdir();
    let mut store = Store::open(Some(dir.clone())).unwrap();
    for seq in 1..=10 {
        let outcome = if seq == 7 { "error" } else { "ok" };
        store.append(&rec(seq, outcome)).unwrap();
    }

    let stats = index::build(&dir).unwrap();
    assert_eq!(stats.calls, 10);
    assert_eq!(stats.failures, 1);

    let db = rusqlite::Connection::open(dir.join("index.db")).unwrap();
    let offsets: Vec<i64> = db
        .prepare("SELECT offset FROM windows ORDER BY offset")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert_eq!(offsets, vec![-5, -4, -3, -2, -1, 1, 2, 3]);
}

#[test]
fn a_failure_near_the_start_gets_a_short_window() {
    let dir = tempdir();
    let mut store = Store::open(Some(dir.clone())).unwrap();
    store.append(&rec(1, "error")).unwrap();
    store.append(&rec(2, "ok")).unwrap();

    index::build(&dir).unwrap();
    let db = rusqlite::Connection::open(dir.join("index.db")).unwrap();
    let count: i64 = db
        .query_row("SELECT COUNT(*) FROM windows", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 1, "only the one following call exists");
}

#[test]
fn rebuilding_is_idempotent() {
    let dir = tempdir();
    let mut store = Store::open(Some(dir.clone())).unwrap();
    store.append(&rec(1, "ok")).unwrap();

    index::build(&dir).unwrap();
    let stats = index::build(&dir).unwrap();
    assert_eq!(stats.calls, 1);

    let db = rusqlite::Connection::open(dir.join("index.db")).unwrap();
    let count: i64 = db
        .query_row("SELECT COUNT(*) FROM calls", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 1, "rebuild must not duplicate rows");
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --test index`
Expected: FAIL — `mcpeval::index` does not exist.

- [ ] **Step 3: Write minimal implementation**

`src/index.rs`:

```rust
use std::path::Path;

use anyhow::Context;
use rusqlite::{params, Connection};

use crate::record::CallRecord;

pub struct Stats {
    pub calls: usize,
    pub failures: usize,
}

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS calls (
  id INTEGER PRIMARY KEY,
  ts TEXT NOT NULL, session TEXT NOT NULL, seq INTEGER NOT NULL,
  server TEXT NOT NULL, method TEXT NOT NULL, tool TEXT,
  latency_ms INTEGER, outcome TEXT NOT NULL,
  err_code TEXT, err_template TEXT, err_retryable INTEGER,
  args TEXT, kind TEXT NOT NULL,
  UNIQUE (session, seq)
);
CREATE TABLE IF NOT EXISTS windows (
  failure_id INTEGER NOT NULL REFERENCES calls(id),
  neighbour_id INTEGER NOT NULL REFERENCES calls(id),
  offset INTEGER NOT NULL,
  PRIMARY KEY (failure_id, neighbour_id)
);
CREATE INDEX IF NOT EXISTS calls_issue ON calls (server, tool, err_template);
";

pub fn build(root: &Path) -> anyhow::Result<Stats> {
    let db = Connection::open(root.join("index.db")).context("opening index.db")?;
    db.execute_batch(SCHEMA)?;

    let mut records: Vec<CallRecord> = Vec::new();
    let store_dir = root.join("store");
    for entry in std::fs::read_dir(&store_dir)
        .with_context(|| format!("reading {}", store_dir.display()))?
    {
        let body = std::fs::read_to_string(entry?.path())?;
        for line in body.lines() {
            records.push(serde_json::from_str(line)?);
        }
    }
    records.sort_by(|a, b| (a.session.as_str(), a.seq).cmp(&(b.session.as_str(), b.seq)));

    let failures = records.iter().filter(|r| r.outcome == "error").count();
    for r in &records {
        let err = r.error.as_ref();
        db.execute(
            "INSERT OR IGNORE INTO calls
             (ts, session, seq, server, method, tool, latency_ms, outcome,
              err_code, err_template, err_retryable, args, kind)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13)",
            params![
                r.ts, r.session, r.seq, r.server, r.method, r.tool, r.latency_ms, r.outcome,
                err.and_then(|e| e.code.as_ref()).map(ToString::to_string),
                err.and_then(|e| e.template.clone()),
                err.and_then(|e| e.retryable).map(|b| b as i64),
                r.args.as_ref().map(ToString::to_string),
                r.kind,
            ],
        )?;
    }

    db.execute("DELETE FROM windows", [])?;
    for (i, r) in records.iter().enumerate() {
        if r.outcome != "error" {
            continue;
        }
        let failure_id = row_id(&db, &r.session, r.seq)?;
        for back in 1..=5usize {
            let Some(j) = i.checked_sub(back) else { break };
            let n = &records[j];
            if n.session != r.session {
                break;
            }
            let nid = row_id(&db, &n.session, n.seq)?;
            db.execute(
                "INSERT OR IGNORE INTO windows VALUES (?1, ?2, ?3)",
                params![failure_id, nid, -(back as i64)],
            )?;
        }
        for fwd in 1..=3usize {
            let Some(n) = records.get(i + fwd) else { break };
            if n.session != r.session {
                break;
            }
            let nid = row_id(&db, &n.session, n.seq)?;
            db.execute(
                "INSERT OR IGNORE INTO windows VALUES (?1, ?2, ?3)",
                params![failure_id, nid, fwd as i64],
            )?;
        }
    }

    Ok(Stats { calls: records.len(), failures })
}

fn row_id(db: &Connection, session: &str, seq: u64) -> anyhow::Result<i64> {
    Ok(db.query_row(
        "SELECT id FROM calls WHERE session = ?1 AND seq = ?2",
        params![session, seq],
        |r| r.get(0),
    )?)
}
```

`src/lib.rs`, add `pub mod index;`.

`src/main.rs`, replace the `Index` arm:

```rust
        cli::Command::Index => {
            let store = mcpeval::store::Store::open(None)?;
            let stats = mcpeval::index::build(store.root())?;
            println!("indexed {} calls, {} failures", stats.calls, stats.failures);
            Ok(())
        }
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test --test index`
Expected: PASS (3 tests).

- [ ] **Step 5: Commit**

```bash
git add src/index.rs src/lib.rs src/main.rs tests/index.rs
git commit -m "Add SQLite index and failure window derivation"
```

---

### Task 10: Install docs and first live capture

**Files:**
- Create: `README.md`
- Create: `docs/install.md`
- Test: manual, recorded in `docs/install.md`

**Interfaces:**
- Consumes: everything above.
- Produces: nothing consumed by later tasks.

- [ ] **Step 1: Write the docs**

`README.md`:

```markdown
# mcp-eval

Captures what MCP servers cost agents, and turns repeated friction into findings a
development agent can act on.

Phase 1 ships the capture path: a redacting stdio shim and a queryable store.

## Quick start

    cargo build --release
    ./target/release/mcpeval shim --server demo -- your-mcp-server --flags
    ./target/release/mcpeval index

Records land in `$MCPEVAL_HOME` (default `~/.mcp-eval`).

## What is recorded

Argument *shapes*, never values: keys, types, string length buckets, container sizes,
schema-declared enum values, numbers, booleans, and the registrable domain of URLs.
Error messages are stored only as templates with values replaced by placeholders.

There is no verbose mode. See `docs/design/2026-08-04-mcp-eval.md`.
```

`docs/install.md`:

```markdown
# Installing the shim in front of a server

Wrap the server command in your MCP client config. Before:

    { "command": "/path/to/server", "args": ["serve"] }

After:

    { "command": "/path/to/mcpeval",
      "args": ["shim", "--server", "myserver", "--", "/path/to/server", "serve"] }

Environment variables the server needs are inherited unchanged. The server's stderr is
passed through untouched.

Set `MCPEVAL_SESSION` to correlate a capture with an agent session id; otherwise the
shim generates one per process.

## Verifying a live capture

1. Restart the client so it respawns the server through the shim.
2. Exercise the server normally.
3. Run `mcpeval index`.
4. Confirm counts:

       sqlite3 ~/.mcp-eval/index.db \
         "SELECT server, tool, outcome, COUNT(*) FROM calls GROUP BY 1,2,3 ORDER BY 4 DESC;"

5. Confirm redaction on real traffic:

       grep -ric "@\|/Users/\|token=" ~/.mcp-eval/store/*.jsonl

   Expected: 0. A non-zero result is a redaction bug — stop and fix it before capturing
   further.
```

- [ ] **Step 2: Run the live capture**

Install the shim in front of one real server. Exercise it for a normal working session.

- [ ] **Step 3: Verify**

Run: `mcpeval index`
Expected: a non-zero call count, and the `grep` in `docs/install.md` returning 0.

- [ ] **Step 4: Run the full suite once more**

Run: `cargo test --release`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add README.md docs/install.md
git commit -m "Add install docs and live capture procedure"
```

---

## Out of scope for this plan

Phases 2 through 4 of the design get their own plans, each producing working software:

- **Phase 2** — promoter and reporter: issue aggregation, Wilson-bounded scoring, threshold calibration against the seed corpus, findings output.
- **Phase 3** — probe battery, including `degradation-over-n` and `instruction-fidelity`, plus the `mcp-eval.manifest.json` sandbox declaration.
- **Phase 4** — generated verification probes, the self-closing finding lifecycle, HTTP/SSE shim transport, and the annotation skill plus slash commands.
