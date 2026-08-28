#!/usr/bin/env bash
# Collect readiness scores from popular public MCP servers into
# data/readiness-corpus.json. Each server is probed with a generic
# manifest (discovery + token budget + pagination + surface listing) so
# the corpus is comparable across heterogeneous servers.
#
# Usage: scripts/corpus/collect.sh [extra npx args...]
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
OUT="${CORPUS_OUT:-$ROOT/data/readiness-corpus.json}"
WORK="${CORPUS_WORK:-$(mktemp -d)}"
mkdir -p "$WORK"

# label|npx package[@version]|args...
SERVERS=(
  "memory|@modelcontextprotocol/server-memory|"
  "filesystem|@modelcontextprotocol/server-filesystem|/tmp"
  "everything|@modelcontextprotocol/server-everything|stdio"
  "git|@modelcontextprotocol/server-git|--repository /tmp"
  "sequential-thinking|@modelcontextprotocol/server-sequential-thinking|"
  "time|@modelcontextprotocol/server-time|"
  "fetch|@modelcontextprotocol/server-fetch|"
  "gdrive|@modelcontextprotocol/server-gdrive|"
  "postgres|@modelcontextprotocol/server-postgres|postgresql://localhost/invalid"
  "slack|@modelcontextprotocol/server-slack|xoxp-not-real"
)

MANIFEST="$WORK/corpus.manifest.json"
cat > "$MANIFEST" <<'EOF'
{"version":1,"probes":[
  {"id":"discovery-budget","probe":"discovery-cost","access":"read_only","max_tools":50,"max_schema_bytes":200000},
  {"id":"token-budget","probe":"token-cost","access":"read_only","max_total_tokens":100000,"max_tool_tokens":20000},
  {"id":"pages","probe":"pagination","access":"read_only","max_pages":5},
  {"id":"surfaces","probe":"surface-listing","access":"read_only","max_pages":5}
]}
EOF

RESULTS="$WORK/scores.jsonl"
: > "$RESULTS"

for entry in "${SERVERS[@]}"; do
  IFS='|' read -r label package args <<< "$entry"
  echo "== probing $label ($package) =="
  set +e
  score=$("$ROOT/target/release/mcpeval" probe \
    --server "$label" --manifest "$MANIFEST" --format json \
    -- npx -y "$package" $args 2>/dev/null \
    | python3 -c "import json,sys
try:
    doc=json.load(sys.stdin)
    print(doc['readiness']['score'])
except Exception:
    print('')")
  set -e
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
if not observations:
    sys.exit("no observations collected")
doc = {
    "schema": "mcpeval.readiness-corpus/v1",
    "source": "battery over popular public MCP servers; see scripts/corpus/collect.sh",
    "observations": sorted(observations, key=lambda o: o["server"]),
}
os.makedirs(os.path.dirname(out_path), exist_ok=True)
json.dump(doc, open(out_path, "w"), indent=2)
open(out_path, "a").write("\n")
print(f"wrote {len(observations)} observations to {out_path}")
PYEOF