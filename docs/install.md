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
If it is unset, the shim generates one UUID when the process starts and uses it
for every record from that process.

## What crosses the persistence boundary

The shim forwards the live MCP byte stream unchanged. For completed calls and
inbound server notifications, its journal keeps metadata and shaped
`params.arguments`, but not raw response bodies.

Every invalid or otherwise unparseable frame produces a content-free record with
only `ts`, `session`, `seq`, `server`, an `unparsed/{direction}` method, the
`unparsed` outcome, `shim_self_us`, and `kind`. The direction is `outbound` or
`inbound`; the frame's raw bytes are forwarded unchanged but never stored. Human
error messages always become the constant `{message}`.

HTTP(S) URLs retain only the true public-suffix registrable domain. Literal IP
hosts, `localhost`, and nonregistrable hosts become the constants `ip`,
`localhost`, and `host`. Other strings become length buckets unless the called
tool's own input schema declared the value as an enum.

The configured server name, `MCPEVAL_SESSION`, method and tool names, argument
keys, schema-declared enum values, numeric and boolean arguments, and
registrable domains remain queryable. Treat those fields as non-secret metadata.
There is no verbose or raw-payload mode.

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
   grep -Erni '@|/Users/|token=' "$MCPEVAL_HOME"/store/*.jsonl
   ```

   Expected: no output and exit status 1. Any match is a redaction bug; stop
   capturing and inspect the journal before proceeding. This is a minimum smoke
   scan, not proof that arbitrary metadata is non-sensitive.

PowerShell users can perform the same scan with:

```powershell
$leaks = Get-ChildItem "$env:MCPEVAL_HOME\store\*.jsonl" |
  Select-String -Pattern '@', '/Users/', 'token=' -CaseSensitive:$false
$leaks.Count
```

Expected: `0`.

## Bobby Browser live validation — 2026-08-04

The release shim was validated against a fresh Bobby Browser stdio gateway child
connected to the already-running service on `127.0.0.1:7777`; the service and its
repository were not changed, stopped, or reconfigured.

The MCP 2025-11-25 handshake succeeded, `tools/list` returned 43 tools, and the
selected `runtime_info` tool was annotated read-only and non-destructive. Its
normal `tools/call` succeeded. A separate invalid-argument request to that same
read-only tool exercised redaction without executing a browser action.

`mcpeval index` printed `indexed 4 calls, 1 failures`. SQLite then confirmed that
all 4 records were real and belonged to 1 session, with 3 successful calls, 1
error, and 3 failure-window rows. The planted-value scan and the common-pattern
scan above each found 0 persisted matches. The redaction-probe arguments contained
only string buckets, `url:example.co.uk`, `url:ip`, `url:localhost`, and
`url:host`. Its error fields contained a scalar code and the constant `{message}`;
no raw request or response payload is included here.
