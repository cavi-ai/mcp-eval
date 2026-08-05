# mcp-eval Phase 1.5 (restore discrimination, add annotations, prove redaction)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or superpowers:executing-plans. Steps use checkbox (`- [ ]`) syntax.

**Goal:** Make the captured journal discriminating enough for Phase 2's promotion math, add the annotation channel the design requires, and turn the redaction guarantee into something executable.

**Architecture:** Phase 1 collapsed every error message to the constant `{message}` and bucketed string error codes, which erased the `(server, tool, error-template)` aggregation key. This phase restores discrimination without storing content: a salted, non-invertible fingerprint of the message skeleton, and verbatim retention of error codes that are identifiers rather than prose. It then adds the `annotation` record type and an executable redaction check.

**Tech Stack:** Rust, existing dependencies only — `sha2` is already present; do not add new crates.

## Global Constraints

- No stored value may be readable content. A fingerprint is acceptable only if it is salted with a per-install secret that never leaves the machine.
- Redaction still happens before anything is written. There is no verbose mode and no raw frame buffer.
- The proxied stream stays byte-transparent.
- Store root is `$MCPEVAL_HOME` when set, else `~/.mcp-eval`.
- Identifier grammar, used in several places below, means: non-empty, at most 64 bytes, matches `^[A-Za-z][A-Za-z0-9_.:-]*$`. Prose fails it because prose contains spaces.
- Every task ends with `cargo test` green and pristine output.

---

### Task 1: Salted error fingerprints and identifier-preserving codes

**Files:**
- Create: `src/fingerprint.rs`
- Modify: `src/errtemplate.rs`, `src/record.rs`, `src/correlate.rs`, `src/index.rs`, `src/shim.rs`
- Test: `tests/fingerprint.rs`, and extend `tests/store.rs`, `tests/index.rs`

**Interfaces:**
- Produces:
  - `fingerprint::Salt` — opaque 32 bytes. `Salt::load(root: &Path) -> anyhow::Result<Salt>` reads `<root>/salt`, creating it with 32 bytes from two `uuid::Uuid::new_v4()` values if absent, written with mode 0600 on Unix. `Salt::for_tests() -> Salt` returns a fixed value.
  - `fingerprint::template_id(salt: &Salt, message: &str) -> String` — lowercase hex of the first 8 bytes of `SHA256(salt_bytes || 0x00 || errtemplate::skeleton(message))`.
  - `errtemplate::skeleton(message: &str) -> String` — content-collapsing normalizer used only as fingerprint input, never stored.
  - `record::error_info(payload: &Value, salt: &Salt) -> ErrorInfo` — signature gains the salt.
  - `record::ErrorInfo.template_id: Option<String>`.

The skeleton collapses variable content so that two messages describing the same defect fingerprint identically: lowercase the message; replace UUIDs with `u`; replace quoted runs (single or double) with `q`; replace `scheme://…` runs with `l`; replace absolute paths with `p`; replace digit runs with `0`; collapse whitespace runs to one space; trim.

`{message}` stays exactly as it is in `template`. The fingerprint is additive.

- [ ] **Step 1: Write the failing tests**

`tests/fingerprint.rs`:

```rust
use mcpeval::errtemplate::skeleton;
use mcpeval::fingerprint::{template_id, Salt};

#[test]
fn same_defect_different_values_fingerprints_the_same() {
    let salt = Salt::for_tests();
    let a = template_id(&salt, "session 0be9b59c-af70-47b0-9169-d9de92330600 died after 5 actions");
    let b = template_id(&salt, "session f5a8fb32-922f-4f72-b09a-474045fd0094 died after 12 actions");
    assert_eq!(a, b);
}

#[test]
fn different_defects_fingerprint_differently() {
    let salt = Salt::for_tests();
    let a = template_id(&salt, "failed to bind companion server");
    let b = template_id(&salt, "journal line 6 is corrupt");
    assert_ne!(a, b);
}

#[test]
fn a_different_salt_changes_the_fingerprint() {
    let mine = template_id(&Salt::for_tests(), "boom");
    let other = template_id(&Salt::from_bytes([7u8; 32]), "boom");
    assert_ne!(mine, other);
}

#[test]
fn the_fingerprint_does_not_contain_the_message() {
    let id = template_id(&Salt::for_tests(), "Cannot upload /Users/someone/private.pdf");
    assert_eq!(id.len(), 16);
    assert!(id.chars().all(|c| c.is_ascii_hexdigit()));
    assert!(!id.contains("private"));
}

#[test]
fn skeleton_collapses_values_but_keeps_structure() {
    assert_eq!(
        skeleton("Session 0BE9B59C-AF70-47B0-9169-D9DE92330600 died after 5 actions"),
        "session u died after 0 actions"
    );
    assert_eq!(skeleton("cannot open \"/tmp/x\""), "cannot open q");
    assert_eq!(skeleton("ws://127.0.0.1:9222/session unreachable"), "l unreachable");
    assert_eq!(skeleton("descriptor at /Users/a/b.json missing"), "descriptor at p missing");
}

#[test]
fn salt_persists_across_loads() {
    let dir = std::env::temp_dir().join(format!("mcpeval-salt-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let first = Salt::load(&dir).unwrap();
    let second = Salt::load(&dir).unwrap();
    assert_eq!(
        template_id(&first, "same message"),
        template_id(&second, "same message")
    );
}
```

Extend `tests/store.rs` with:

```rust
#[test]
fn identifier_error_codes_are_kept_and_prose_codes_are_bucketed() {
    use mcpeval::fingerprint::Salt;
    use serde_json::json;

    let salt = Salt::for_tests();
    let identifier = mcpeval::record::error_info(
        &json!({ "code": "browserCommandFailed", "message": "boom" }),
        &salt,
    );
    assert_eq!(identifier.code.unwrap(), json!("browserCommandFailed"));

    let prose = mcpeval::record::error_info(
        &json!({ "code": "Cannot upload /Users/someone/private.pdf", "message": "boom" }),
        &salt,
    );
    let stored = serde_json::to_string(&prose).unwrap();
    assert!(!stored.contains("private"), "prose code leaked: {stored}");
    assert!(!stored.contains("Users"), "prose code leaked: {stored}");
}

#[test]
fn error_info_carries_a_fingerprint_and_still_hides_the_message() {
    use mcpeval::fingerprint::Salt;
    use serde_json::json;

    let info = mcpeval::record::error_info(
        &json!({ "code": -32000, "message": "session 0be9b59c-af70-47b0-9169-d9de92330600 gone" }),
        &Salt::for_tests(),
    );
    assert_eq!(info.template.as_deref(), Some("{message}"));
    let id = info.template_id.unwrap();
    assert_eq!(id.len(), 16);

    let stored = serde_json::to_string(&info).unwrap();
    assert!(!stored.contains("gone"));
    assert!(!stored.contains("0be9b59c"));
}
```

Extend `tests/index.rs` with:

```rust
#[test]
fn distinct_error_fingerprints_stay_distinct_rows() {
    // Build two failures on the same server and tool with different fingerprints,
    // index them, and assert the issue key separates them.
    let dir = tempdir();
    let mut store = Store::open(Some(dir.clone())).unwrap();

    let mut first = rec(1, "error");
    first.error = Some(mcpeval::record::ErrorInfo {
        code: Some(serde_json::json!("browserCommandFailed")),
        layer: None,
        retryable: Some(false),
        kind: None,
        template: Some("{message}".into()),
        template_id: Some("aaaaaaaaaaaaaaaa".into()),
    });
    let mut second = rec(2, "error");
    second.error = Some(mcpeval::record::ErrorInfo {
        code: Some(serde_json::json!("browserCommandFailed")),
        layer: None,
        retryable: Some(false),
        kind: None,
        template: Some("{message}".into()),
        template_id: Some("bbbbbbbbbbbbbbbb".into()),
    });
    store.append(&first).unwrap();
    store.append(&second).unwrap();

    index::build(&dir).unwrap();
    let db = rusqlite::Connection::open(dir.join("index.db")).unwrap();
    let distinct: i64 = db
        .query_row(
            "SELECT COUNT(*) FROM (SELECT DISTINCT server, tool, err_code, err_template_id FROM calls WHERE outcome = 'error')",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(distinct, 2, "two causes must not collapse into one issue");
}
```

- [ ] **Step 2: Run the tests and watch them fail**

Run: `cargo test --test fingerprint --test store --test index`
Expected: FAIL — `mcpeval::fingerprint` does not exist, `error_info` takes one argument, `ErrorInfo` has no `template_id`.

- [ ] **Step 3: Implement**

Write `src/fingerprint.rs` with `Salt` (a newtype over `[u8; 32]` with `load`, `for_tests`, `from_bytes`, and a private `bytes()` accessor) and `template_id` as specified in Interfaces. Salt file creation on Unix uses `std::os::unix::fs::OpenOptionsExt` mode `0o600`; on other platforms plain create is acceptable.

Add `errtemplate::skeleton` implementing the collapse order given above, keeping `normalize` returning `{message}` unchanged.

Add `template_id: Option<String>` to `ErrorInfo` (serialized when present), and change `error_info` to take `&Salt` and populate it from the raw message before the message is discarded. Preserve string codes verbatim when they satisfy the identifier grammar; otherwise bucket them exactly as today.

Thread a `Salt` from the shim (loaded once from the store root) through `Correlator` to `error_info`. `Correlator::new` gains the salt parameter.

In `src/index.rs`, add an `err_template_id TEXT` column, populate it, and change the `calls_issue` index to `(server, tool, err_code, err_template_id)`.

- [ ] **Step 4: Run the tests and watch them pass**

Run: `cargo test`
Expected: PASS, all tests, output pristine.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "Restore error discrimination with salted fingerprints"
```

---

### Task 2: The annotation channel

**Files:**
- Modify: `src/record.rs`, `src/store.rs`, `src/index.rs`, `src/cli.rs`, `src/main.rs`
- Test: `tests/annotate.rs`

**Interfaces:**
- Produces:
  - `record::AnnotationRecord { ts: String, session: String, seq: u64, kind: String, note: String }`
  - `record::ANNOTATION_KINDS: [&str; 5]` = `["blocked-optimal-path", "undocumented-behavior", "false-success", "instruction-divergence", "workaround"]`
  - `store::Store::append_annotation(&mut self, rec: &AnnotationRecord) -> anyhow::Result<()>` writing `annotations-YYYY-MM-DD.jsonl` in the same directory, with the same session hashing and file locking as `append`
  - CLI subcommand `Annotate { session: String, seq: u64, kind: String, note: String }`, invoked as `mcpeval annotate --session <s> --seq <n> --kind <k> --note <text>`

An annotation targets a call by `(session, seq)` — the pair is the call's identity in the store. Notes are capped at 240 characters and any control characters or newlines are rejected, so a note cannot smuggle a payload or corrupt the JSONL framing. An unknown kind is rejected with the list of valid kinds.

- [ ] **Step 1: Write the failing test**

`tests/annotate.rs`:

```rust
use std::process::Command;

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_mcpeval")
}

fn tempdir() -> std::path::PathBuf {
    let base = std::env::temp_dir().join(format!("mcpeval-annotate-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&base).unwrap();
    base
}

#[test]
fn writes_an_annotation_record() {
    let home = tempdir();
    let out = Command::new(bin())
        .args(["annotate", "--session", "s1", "--seq", "7",
               "--kind", "false-success", "--note", "click reported success, nothing changed"])
        .env("MCPEVAL_HOME", &home)
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));

    let mut found = None;
    for entry in std::fs::read_dir(home.join("store")).unwrap() {
        let path = entry.unwrap().path();
        if path.file_name().unwrap().to_str().unwrap().starts_with("annotations-") {
            found = Some(std::fs::read_to_string(path).unwrap());
        }
    }
    let body = found.expect("an annotations file must exist");
    let value: serde_json::Value = serde_json::from_str(body.lines().next().unwrap()).unwrap();
    assert_eq!(value["kind"], "false-success");
    assert_eq!(value["seq"], 7);
    assert!(value["session"].as_str().unwrap().starts_with("session:"),
            "session must be hashed, got {}", value["session"]);
}

#[test]
fn rejects_an_unknown_kind() {
    let home = tempdir();
    let out = Command::new(bin())
        .args(["annotate", "--session", "s1", "--seq", "1",
               "--kind", "vibes", "--note", "n"])
        .env("MCPEVAL_HOME", &home)
        .output()
        .unwrap();
    assert!(!out.status.success());
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("false-success"), "error must list valid kinds: {err}");
}

#[test]
fn rejects_an_overlong_or_multiline_note() {
    let home = tempdir();
    for note in [&"x".repeat(241), "line one\nline two"] {
        let out = Command::new(bin())
            .args(["annotate", "--session", "s1", "--seq", "1",
                   "--kind", "workaround", "--note", note])
            .env("MCPEVAL_HOME", &home)
            .output()
            .unwrap();
        assert!(!out.status.success(), "note should be rejected: {note:?}");
    }
}

#[test]
fn index_loads_annotations_and_links_them_to_calls() {
    use mcpeval::record::{AnnotationRecord, CallRecord};
    use mcpeval::store::Store;

    let home = tempdir();
    let mut store = Store::open(Some(home.clone())).unwrap();
    store
        .append(&CallRecord {
            ts: "2026-08-04T12:00:00Z".into(),
            session: "s1".into(),
            seq: 7,
            server: "demo".into(),
            method: "tools/call".into(),
            tool: Some("click".into()),
            args: None,
            latency_ms: Some(5),
            outcome: "ok".into(),
            error: None,
            shim_self_us: 1,
            kind: "real".into(),
        })
        .unwrap();
    store
        .append_annotation(&AnnotationRecord {
            ts: "2026-08-04T12:00:01Z".into(),
            session: "s1".into(),
            seq: 7,
            kind: "false-success".into(),
            note: "reported success, nothing changed".into(),
        })
        .unwrap();

    mcpeval::index::build(&home).unwrap();
    let db = rusqlite::Connection::open(home.join("index.db")).unwrap();
    let linked: i64 = db
        .query_row(
            "SELECT COUNT(*) FROM annotations a JOIN calls c
             ON c.session = a.session AND c.seq = a.seq",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(linked, 1);
}
```

- [ ] **Step 2: Run the test and watch it fail**

Run: `cargo test --test annotate`
Expected: FAIL — no `annotate` subcommand, no `AnnotationRecord`.

- [ ] **Step 3: Implement**

Add `AnnotationRecord` and `ANNOTATION_KINDS` to `src/record.rs`, with a validator returning a descriptive error for an unknown kind, a note over 240 characters, or a note containing a control character.

Add `Store::append_annotation`, mirroring `append`: same directory, `annotations-<day>.jsonl`, session hashed through `privacy::opaque_session`, same file locking.

Add the `Annotate` subcommand to `src/cli.rs` and wire it in `src/main.rs`: validate, build the record with the current UTC timestamp, append, print nothing on success.

In `src/index.rs`, create `annotations (session TEXT, seq INTEGER, ts TEXT, kind TEXT, note TEXT)`, load `annotations-*.jsonl` alongside the call files, and clear it on rebuild like the other tables.

- [ ] **Step 4: Run the tests and watch them pass**

Run: `cargo test`
Expected: PASS, all tests.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "Add annotation record type and CLI"
```

---

### Task 3: Tool-name grammar, deterministic ordering, executable redaction check

**Files:**
- Modify: `src/correlate.rs`, `src/index.rs`, `src/cli.rs`, `src/main.rs`
- Create: `src/doctor.rs`
- Test: `tests/redaction_property.rs`, extend `tests/correlate.rs` and `tests/index.rs`

**Interfaces:**
- Produces:
  - `doctor::check_redaction(root: &Path) -> anyhow::Result<doctor::Report>` with `Report { files: usize, findings: Vec<String> }`
  - CLI subcommand `Doctor { check_redaction: bool }`, invoked as `mcpeval doctor --check-redaction`, exiting non-zero when findings is non-empty

Three independent corrections:

1. **Tool names.** Today a tool name is kept only after a `tools/list` response declares it, so servers whose schemas load on demand record every call as `unlisted` and lose the tool dimension of the issue key. Keep the name whenever it satisfies the identifier grammar (`privacy::valid_tool`); fall back to `unlisted` only when it does not. Enum learning still requires a declared schema — that is unchanged.
2. **Ordering.** `index::build` sorts by session alone and relies on read order for sequence. Sort by `(session, seq)` so windows follow logical order even when two processes share a session id.
3. **Redaction check.** Make the manual grep in `docs/install.md` a command, and add a property test that plants high-entropy canaries at random depths in generated argument trees.

- [ ] **Step 1: Write the failing tests**

`tests/redaction_property.rs`:

```rust
use mcpeval::shape::{self, EnumIndex};
use serde_json::{json, Value};

/// Deterministic pseudo-random generator: tests must reproduce on failure.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        self.0 >> 33
    }
}

fn canary(index: usize) -> String {
    format!("CANARY-{index:04x}-8f3ad91b2c7e")
}

fn build_tree(rng: &mut Rng, depth: usize, next_canary: &mut usize) -> Value {
    if depth == 0 || rng.next() % 4 == 0 {
        let index = *next_canary;
        *next_canary += 1;
        return match rng.next() % 3 {
            0 => json!(canary(index)),
            1 => json!(format!("/Users/someone/{}.pdf", canary(index))),
            _ => json!(format!("https://example.com/a?token={}", canary(index))),
        };
    }
    if rng.next() % 2 == 0 {
        let len = (rng.next() % 4) as usize + 1;
        Value::Array((0..len).map(|_| build_tree(rng, depth - 1, next_canary)).collect())
    } else {
        let len = (rng.next() % 4) as usize + 1;
        let mut map = serde_json::Map::new();
        for field in 0..len {
            map.insert(format!("field{field}"), build_tree(rng, depth - 1, next_canary));
        }
        Value::Object(map)
    }
}

#[test]
fn no_generated_canary_survives_shaping() {
    let enums = EnumIndex::new();
    for seed in 0..200u64 {
        let mut rng = Rng(seed);
        let mut next_canary = 0;
        let tree = build_tree(&mut rng, 4, &mut next_canary);
        let shaped = shape::of(&tree, "tool", &enums).to_string();
        assert!(
            !shaped.contains("CANARY"),
            "seed {seed} leaked a canary: {shaped}"
        );
        assert!(!shaped.contains("/Users/"), "seed {seed} leaked a path: {shaped}");
        assert!(!shaped.contains("token="), "seed {seed} leaked a query: {shaped}");
    }
}
```

Extend `tests/correlate.rs` with:

```rust
#[test]
fn an_undeclared_but_identifier_shaped_tool_name_is_kept() {
    let mut c = Correlator::new("demo".into(), "sess".into(), mcpeval::fingerprint::Salt::for_tests());
    c.on_outbound(
        &serde_json::json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call",
                             "params": { "name": "never_listed_tool", "arguments": {} } }),
        0,
    );
    let rec = c
        .on_inbound(&serde_json::json!({ "jsonrpc": "2.0", "id": 1, "result": {} }), 1)
        .unwrap();
    assert_eq!(rec.tool.as_deref(), Some("never_listed_tool"));
}

#[test]
fn a_prose_shaped_tool_name_becomes_unlisted() {
    let mut c = Correlator::new("demo".into(), "sess".into(), mcpeval::fingerprint::Salt::for_tests());
    c.on_outbound(
        &serde_json::json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call",
                             "params": { "name": "upload /Users/someone/private.pdf", "arguments": {} } }),
        0,
    );
    let rec = c
        .on_inbound(&serde_json::json!({ "jsonrpc": "2.0", "id": 1, "result": {} }), 1)
        .unwrap();
    assert_eq!(rec.tool.as_deref(), Some("unlisted"));
}
```

Extend `tests/index.rs` with:

```rust
#[test]
fn windows_follow_sequence_not_file_order() {
    let dir = tempdir();
    let mut store = Store::open(Some(dir.clone())).unwrap();
    // Append out of sequence order on purpose.
    for seq in [3u64, 1, 2] {
        let outcome = if seq == 3 { "error" } else { "ok" };
        store.append(&rec(seq, outcome)).unwrap();
    }

    index::build(&dir).unwrap();
    let db = rusqlite::Connection::open(dir.join("index.db")).unwrap();
    let neighbours: Vec<i64> = db
        .prepare("SELECT c.seq FROM windows w JOIN calls c ON c.id = w.neighbour_id ORDER BY w.offset")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert_eq!(neighbours, vec![1, 2], "the failure at seq 3 follows seqs 1 and 2");
}
```

And a doctor test in `tests/annotate.rs` is not appropriate; add `tests/doctor.rs`:

```rust
use std::process::Command;

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_mcpeval")
}

fn tempdir() -> std::path::PathBuf {
    let base = std::env::temp_dir().join(format!("mcpeval-doctor-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(base.join("store")).unwrap();
    base
}

#[test]
fn passes_on_a_clean_store() {
    let home = tempdir();
    std::fs::write(
        home.join("store").join("calls-2026-08-04.jsonl"),
        "{\"ts\":\"2026-08-04T00:00:00Z\",\"session\":\"session:ab\",\"seq\":1,\"server\":\"demo\",\"method\":\"tools/call\",\"outcome\":\"ok\",\"shim_self_us\":1,\"kind\":\"real\"}\n",
    )
    .unwrap();
    let out = Command::new(bin())
        .args(["doctor", "--check-redaction"])
        .env("MCPEVAL_HOME", &home)
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stdout));
}

#[test]
fn fails_when_a_store_file_contains_content() {
    let home = tempdir();
    std::fs::write(
        home.join("store").join("calls-2026-08-04.jsonl"),
        "{\"ts\":\"2026-08-04T00:00:00Z\",\"note\":\"mail me at someone@example.com\"}\n",
    )
    .unwrap();
    let out = Command::new(bin())
        .args(["doctor", "--check-redaction"])
        .env("MCPEVAL_HOME", &home)
        .output()
        .unwrap();
    assert!(!out.status.success(), "an email address must be reported");
}
```

- [ ] **Step 2: Run the tests and watch them fail**

Run: `cargo test --test redaction_property --test doctor --test correlate --test index`
Expected: FAIL — no `doctor` subcommand, `Correlator::new` takes two arguments, tool gating and sort not yet corrected.

- [ ] **Step 3: Implement**

In `src/correlate.rs`, replace the declared-only tool gate with the identifier-grammar rule, keeping enum learning as it is.

In `src/index.rs`, sort by `(session, seq)`.

Create `src/doctor.rs` with `check_redaction`, scanning every `*.jsonl` under `<root>/store` for: an `@` between word characters, `/Users/` or `/home/`, `token=`, `password`, and any string value longer than 128 characters that is not one of the known shape tokens. Each hit becomes a finding naming the file and line number, never the matched text.

Add the `Doctor` subcommand to `src/cli.rs` and `src/main.rs`, printing the file count and each finding, exiting non-zero when findings exist.

Update `docs/install.md` to call `mcpeval doctor --check-redaction` in place of the manual grep.

- [ ] **Step 4: Run the tests and watch them pass**

Run: `cargo test`
Expected: PASS, all tests.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "Keep identifier tool names, order windows by sequence, add redaction doctor"
```
