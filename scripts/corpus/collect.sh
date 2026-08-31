#!/usr/bin/env bash
# Collect readiness scores from popular public MCP servers into
# data/readiness-corpus.json. Each server is probed with a generic
# manifest (discovery + token budget + pagination + surface listing) so
# the corpus is comparable across heterogeneous servers.
#
# Servers that require live credentials or services (gdrive, slack,
# sentry, supabase, ...) are skipped by the harness and must be probed
# by hand with real credentials before a release.
#
# Usage: scripts/corpus/collect.sh
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
OUT="${CORPUS_OUT:-$ROOT/data/readiness-corpus.json}"
WORK="${CORPUS_WORK:-$(mktemp -d)}"
mkdir -p "$WORK"

MANIFEST="$WORK/corpus.manifest.json"
cat > "$MANIFEST" <<'EOF'
{"version":1,"probes":[
  {"id":"discovery-budget","probe":"discovery-cost","access":"read_only","max_tools":80,"max_schema_bytes":400000},
  {"id":"token-budget","probe":"token-cost","access":"read_only","max_total_tokens":200000,"max_tool_tokens":40000},
  {"id":"pages","probe":"pagination","access":"read_only","max_pages":5},
  {"id":"surfaces","probe":"surface-listing","access":"read_only","max_pages":5}
]}
EOF

RESULTS="$WORK/scores.jsonl"
: > "$RESULTS"

BIN="$ROOT/target/release/mcpeval"
if [ ! -x "$BIN" ]; then
  echo "build the release binary first: cargo build --release" >&2
  exit 1
fi

probes_score() {
  "$BIN" probe --server "$1" --manifest "$MANIFEST" --format json \
    -- "${@:2}" 2>/dev/null | python3 -c "
import json,sys
try:
    print(json.load(sys.stdin)['readiness']['score'])
except Exception:
    print('')"
}

# npx-based servers: label|package|args...
NPM_SERVERS=(
  "everything|@modelcontextprotocol/server-everything|stdio"
  "memory|@modelcontextprotocol/server-memory|"
  "filesystem|@modelcontextprotocol/server-filesystem|/tmp"
  "sequential-thinking|@modelcontextprotocol/server-sequential-thinking|"
  "puppeteer|@modelcontextprotocol/server-puppeteer|"
  "github|@modelcontextprotocol/server-github|ghp-sample"
  "notion|@notionhq/notion-mcp-server|"
  "todoist|mcp-todoist|sample"
  "desktop-commander|@wonderwhy-er/desktop-commander|"
  "context7|@upstash/context7-mcp|"
  "browserbase|@browserbasehq/mcp-server-browserbase|"
  "kubernetes|mcp-server-kubernetes|"
  "playwright|@executeautomation/playwright-mcp-server|"
  "postgres|@modelcontextprotocol/server-postgres|postgresql://localhost/invalid"
)

for entry in "${NPM_SERVERS[@]}"; do
  IFS='|' read -r label package args <<< "$entry"
  echo "== probing $label (npx $package) =="
  score=$(probes_score "$label" npx -y "$package" $args || true)
  if [ -n "$score" ]; then
    printf '{"server": "%s", "score": %s}\n' "$label" "$score" >> "$RESULTS"
    echo "   score=$score"
  else
    echo "   skipped (battery could not run)"
  fi
done

# uvx-based servers: label|package|args...
UVX_SERVERS=(
  "fetch|mcp-server-fetch|"
  "time|mcp-server-time|"
  "markitdown|markitdown-mcp|"
  "mcp-atlassian|mcp-atlassian|"
  "pandoc|mcp-pandoc|"
)

for entry in "${UVX_SERVERS[@]}"; do
  IFS='|' read -r label package args <<< "$entry"
  echo "== probing $label (uvx $package) =="
  score=$(probes_score "$label" uvx "$package" $args || true)
  if [ -n "$score" ]; then
    printf '{"server": "%s", "score": %s}\n' "$label" "$score" >> "$RESULTS"
    echo "   score=$score"
  else
    echo "   skipped (battery could not run)"
  fi
done

python3 - "$RESULTS" "$OUT" <<'PYEOF'
import json, sys, os
results_path, out_path = sys.argv[1], sys.argv[2]
observations = [json.loads(line) for line in open(results_path) if line.strip()]
if len(observations) < 10:
    sys.exit(f"only {len(observations)} observations collected; refusing to ship a thin corpus")
doc = {
    "schema": "mcpeval.readiness-corpus/v1",
    "source": "readiness battery over popular public MCP servers, collected via scripts/corpus/collect.sh; servers requiring live credentials or services are re-run before each release",
    "observations": sorted(observations, key=lambda o: (o["score"], o["server"])),
}
os.makedirs(os.path.dirname(out_path), exist_ok=True)
json.dump(doc, open(out_path, "w"), indent=2)
open(out_path, "a").write("\n")
scores = sorted(o["score"] for o in observations)
print(f"wrote {len(observations)} observations to {out_path} (median {scores[len(scores)//2]}, min {scores[0]})")
PYEOF