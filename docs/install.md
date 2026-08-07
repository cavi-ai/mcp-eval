# Installing the shim in front of a server

Build the release binary:

```sh
cargo build --release
```

On Unix the binary is `target/release/mcpeval`; on Windows it is
`target\release\mcpeval.exe`. Copy it to a stable location or reference that
release path directly from the MCP client configuration.

The shim supports Unix and Windows. Other targets return an unsupported-target
error. It proxies newline-delimited JSON-RPC over stdio, inherits the child
server's environment unchanged, and passes the child's stderr through untouched.

## Configure an MCP client

Wrap the server command in the client config. Before:

```json
{ "command": "/path/to/server", "args": ["serve"] }
```

After:

```json
{
  "command": "/path/to/mcpeval",
  "args": [
    "shim",
    "--server",
    "myserver",
    "--",
    "/path/to/server",
    "serve"
  ]
}
```

Windows JSON uses escaped backslashes and the `.exe` binary:

```json
{
  "command": "C:\\path\\to\\mcpeval.exe",
  "args": [
    "shim",
    "--server",
    "myserver",
    "--",
    "C:\\path\\to\\server.exe",
    "serve"
  ]
}
```

Set `MCPEVAL_HOME` in the client environment to choose the capture root. If it
is unset, the default is `$HOME/.mcp-eval` when `HOME` is available. Windows
users should set `MCPEVAL_HOME` explicitly when their client does not provide a
`HOME` variable.

Set `MCPEVAL_SESSION` to correlate the capture with an agent session identifier.
The raw value is never persisted: the shim stores a stable `session:<sha256>`
token. If unset, the shim generates a UUID and hashes it once per process.

## What crosses the persistence boundary

The shim forwards the live MCP byte stream unchanged. For completed calls and
inbound server notifications, its journal keeps metadata and shaped
`params.arguments`, but not raw response bodies.

Every invalid or otherwise unparseable frame produces a content-free record with
only `ts`, `session`, `seq`, `server`, an `unparsed/{direction}` method, the
`unparsed` outcome, `shim_self_us`, and `kind`. The direction is `outbound` or
`inbound`; the frame's raw bytes are forwarded unchanged but never stored.

Human error messages are never stored as text. Every error becomes the
constant `{message}`, plus `template_id` — a salted, non-invertible
fingerprint of the message with UUIDs, quoted runs, URLs, paths, and digits
collapsed out first, used only to tell whether two failures share the same
underlying defect; a constant `{message}` alone cannot do that, since it
collapses every distinct defect into one bucket. String error codes, layers,
and kinds become length buckets, except a string code shaped like an
identifier — non-empty, ASCII, starting with a letter, at most 64 bytes —
which is kept verbatim instead. Scalar numeric and boolean codes may remain.

HTTP(S) URLs retain only the true public-suffix registrable domain. Literal IP
hosts, `localhost`, and nonregistrable hosts become the constants `ip`,
`localhost`, and `host`. Other strings become length buckets unless the called
tool's own input schema declared the value as an enum.

Server names must be bounded ASCII labels and methods must use bounded
slash-separated labels. A tool name remains queryable whenever it satisfies
the tool-name grammar — non-empty, ASCII alphanumeric plus `_`, `-`, `.`, or
`:`, at most 128 bytes, with no leading-letter rule — whether or not
`tools/list` ever declared it. This is a different, wider grammar than the
64-byte identifier grammar error codes use above; servers that load schemas
on demand still get a usable tool dimension either way. A call whose tool
name doesn't fit it (prose: spaces, slashes, and the like) uses `unlisted`
instead. Enum learning still requires a declared schema: only a value a
tool's own `tools/list` schema named as an enum member is kept verbatim. The
store revalidates these fields before serialization. Argument keys,
schema-declared enum values, numeric and boolean arguments, and registrable
domains remain queryable. There is no verbose or raw-payload mode.

An agent may also record an `annotation`: a short, typed observation (`kind`
is one of a fixed set, e.g. `false-success`, `workaround`) tied to a
`(session, seq)` call, plus `note` — a free-text field bounded to 240
characters and scrubbed of control characters. `note` is the one deliberate
prose channel in the store, written to `annotations-*.jsonl` alongside the
`calls-*.jsonl` journal; every other field described above is structured,
content-free metadata.

## Where the fingerprint salt lives

`template_id` is non-invertible only while its salt stays secret: the
skeleton space of MCP error messages is small and guessable, so salt plus
journal would let someone recover which messages produced which fingerprint.
The salt is generated once and stored at `<MCPEVAL_HOME>/.salt` (mode 0600 on
Unix) — a dotfile sibling of `store/`, deliberately outside it.

**Only `<MCPEVAL_HOME>/store/` is safe to share.** Never copy, tar, or attach
`.salt` alongside it. `mcpeval doctor --check-redaction` prints the salt path
on its own line as a must-not-share reminder every time it runs.

## Verify a live capture

1. Restart the MCP client so it respawns the server through the shim.
2. Exercise one clearly read-only server tool normally.
3. Run the indexer with the same `MCPEVAL_HOME`:

   ```sh
   mcpeval index
   ```

   The command must report a nonzero call count.

4. Inspect grouped counts in SQLite:

   ```sh
   sqlite3 "$MCPEVAL_HOME/index.db" \
     "SELECT server, tool, outcome, COUNT(*) FROM calls GROUP BY 1,2,3 ORDER BY 4 DESC;"
   ```

5. Scan the JSONL journal for common secret-bearing forms:

   ```sh
   mcpeval doctor --check-redaction
   ```

   Expected: exit status 0, with the scanned file count, no findings, and a
   line naming the salt path as a must-not-share item. Any finding names a
   file and line number — never the matched text — and is a redaction bug;
   stop capturing and inspect that line before proceeding.

   Exit 0 is an automatic guarantee for `calls-*.jsonl` only. `note` in
   `annotations-*.jsonl` is deliberately exempt from every detector — it is
   free-form prose by design — so `doctor` separately prints how many notes
   are present (without failing the check) and asks for a human pass over
   them before that file is shared. This is a minimum smoke scan, not proof
   that arbitrary metadata is non-sensitive.

## Promote issues and read findings

After indexing at least two real sessions, aggregate and score the indexed
calls:

```sh
mcpeval promote
mcpeval findings --format agent
```

Use `--format md` for a human-facing report or `--format json` for structured
output. Promotion requires both a score at or above the active threshold and
failures in at least two distinct sessions. The two-session rule cannot be
disabled by setting the threshold to zero.

The default threshold is recalibrated from the bundled synthetic seed corpus.
To configure it, create `<MCPEVAL_HOME>/config.json`:

```json
{
  "promotion_threshold": 0.15
}
```

`mcpeval promote --threshold 0.2` overrides that value for one run. Values must
be finite and non-negative. Running `mcpeval index` removes stale derived issue
and finding tables, so rerun `mcpeval promote` whenever the journal is rebuilt.

Findings contain aggregate counts, score components, sanitized server/tool and
error identifiers, and shape-level arguments. They exclude raw error templates,
annotation prose, sessions, paths, salt, and raw request values. This makes the
report safe by the same automated boundary as indexed calls; the underlying
`annotations-*.jsonl` files still require the human review described above.

## Run the probe battery

Create a strict `mcp-eval.manifest.json` beside the server project. Unknown fields,
unsupported versions, unsafe access/sandbox combinations, and invalid expectations
are rejected before the server process starts:

```json
{
  "version": 1,
  "probes": [
    {
      "id": "bounded-discovery",
      "probe": "discovery-cost",
      "access": "read_only",
      "max_tools": 10,
      "max_schema_bytes": 1000
    },
    {
      "id": "naive-status",
      "probe": "schema-guessability",
      "tool": "describe_status",
      "access": "read_only",
      "arguments": {}
    },
    {
      "id": "repeat-read",
      "probe": "degradation-over-n",
      "tool": "read_counter",
      "access": "read_only",
      "arguments": {},
      "max_attempts": 5
    },
    {
      "id": "literal-status",
      "probe": "instruction-fidelity",
      "tool": "describe_status",
      "access": "read_only",
      "arguments": {},
      "expect": {
        "outcome": "ok",
        "required_result_fields": ["status"],
        "equals": {"status": "ready"}
      }
    }
  ]
}
```

Run every case or select one probe kind:

```sh
mcpeval probe --server demo --manifest mcp-eval.manifest.json -- your-server
mcpeval probe --server demo --manifest mcp-eval.manifest.json \
  --probe degradation-over-n -- your-server
```

For a Streamable HTTP server, replace the stdio command with a loopback endpoint:

```sh
mcpeval probe --server demo --manifest mcp-eval.manifest.json \
  --url http://127.0.0.1:8080/mcp
```

The client accepts `application/json` and `text/event-stream` POST responses, carries
the negotiated session header, sends the MCP protocol-version header, disables
redirects, bounds responses to 8 MiB, and applies five-second network timeouts. Remote
hosts require both HTTPS and `--allow-remote-http`. URLs containing credentials, query
strings, or fragments are rejected. If authorization is required, provide the complete
header value through `MCPEVAL_HTTP_AUTHORIZATION`; it is used in memory only and never
stored or printed. Manifest mutation gates apply identically to stdio and HTTP.

The command exits zero only when every selected case passes. Summaries contain case
IDs, probe kinds, attempt counts, first-failure positions, and fixed reason labels;
they never contain actual arguments, responses, or errors. Probe calls are recorded
as privacy-sanitized `synthetic` calls.

`discovery-cost` measures the number of declared tools and encoded bytes in the
`tools/list` catalog against manifest limits. `schema-guessability` validates the
selected tool's object schema, checks that required fields exist in both `properties`
and the declared naive arguments, then requires that call to succeed. Tool descriptions
and full schemas are held only for the process lifetime; output contains counts and
fixed failure reasons, never catalog text or schema content.

An `error-honesty` case declares a bounded retry count and expected retryability.
The first call must fail, error codes must remain stable, retryability must match,
and a retryable error must recover before the bound. A non-retryable error must remain
stable across two observations. Output contains only attempt counts and fixed reasons.

`state-recovery` declares three tools and argument objects: one that demonstrates the
failure, one that performs recovery, and one that validates health afterward. The
failure must fail and both later calls must succeed. All three calls are sanitized and
recorded as synthetic. If the sequence mutates state, declare a sandbox and pass
`--allow-mutation`; validation and authorization happen before server launch.

`contention` starts a second independent MCP client, synchronizes both clients at the
selected tool call, and requires both responses to succeed. Both calls are recorded as
sanitized synthetic observations. Mutating cases still require a declared sandbox and
explicit `--allow-mutation` authorization.

Mutation has two independent gates. The manifest must declare a named sandbox and the
case must reference it:

```json
{
  "version": 1,
  "sandboxes": {
    "fixture": {"description": "isolated disposable fixture state"}
  },
  "probes": [
    {
      "id": "reset-fixture",
      "probe": "instruction-fidelity",
      "tool": "reset_counter",
      "access": "mutating",
      "sandbox": "fixture",
      "arguments": {},
      "expect": {"outcome": "ok", "equals": {"reset": true}}
    }
  ]
}
```

The operator must then opt in explicitly:

```sh
mcpeval probe --server demo --manifest mcp-eval.manifest.json \
  --allow-mutation -- your-server
```

Use only an isolated, disposable target. Without both gates, mcp-eval fails before
launch. `instruction-fidelity` is deterministic and structural in Phase 3; it does
not invoke an LLM or disclose data to a network service. Manifest arguments and
sandbox descriptions may themselves be sensitive. They are never persisted by
mcp-eval, but the manifest file is not a share-safe artifact.

## Verify and close a finding

`mcpeval findings` emits a stable `finding-*` identifier, lifecycle state, attached
probe ID, and consecutive-pass count. Verify a finding with exactly one manifest
case whose tool matches the finding:

```sh
mcpeval verify --finding finding-0123456789abcdef \
  --case literal-status --manifest mcp-eval.manifest.json \
  -- your-server
```

A failing first run records `fix-claimed`. Passing runs move the finding through
`verifying` and close it after three consecutive greens. Any later failure reopens
the finding and resets the streak. Verification history is append-only and survives
`index` and `promote` rebuilds. A missing finding, unknown case, or tool mismatch is
rejected before the server launches.

Findings without a probe remain open for manual handling and are capped at medium
severity. The `verify` command inherits the probe battery's privacy boundary and
read-only default. For a mutating case, the manifest must declare its sandbox and
the command must include `--allow-mutation`; verification never weakens either gate.
