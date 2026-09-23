#!/usr/bin/env bash
# Collect readiness scores from popular public MCP servers into
# data/readiness-corpus.json. Each server is probed with a generic
# manifest (discovery + token budget + pagination + surface listing) so
# the corpus is comparable across heterogeneous servers. The document
# records that manifest's probe kinds as its battery, and each observation
# carries the catalog's tool count and token estimate beside the score.
#
# Servers that require live credentials or services (gdrive, slack,
# sentry, supabase, ...) are skipped by the harness and must be probed
# by hand with real credentials before a release.
#
# Usage: scripts/corpus/collect.sh
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
OUT="${CORPUS_OUT:-$ROOT/data/readiness-corpus.json}"
case "$OUT" in
  /*) ;;
  *) OUT="$(pwd)/$OUT" ;;
esac
WORK="${CORPUS_WORK:-$(mktemp -d)}"
mkdir -p "$WORK"
WORK="$(cd "$WORK" && pwd)"

MANIFEST="$WORK/corpus.manifest.json"
cat > "$MANIFEST" <<'EOF'
{"version":1,"probes":[
  {"id":"discovery-budget","probe":"discovery-cost","access":"read_only","max_tools":80,"max_schema_bytes":400000},
  {"id":"token-budget","probe":"token-cost","access":"read_only","max_total_tokens":200000,"max_tool_tokens":40000},
  {"id":"pages","probe":"pagination","access":"read_only","max_pages":5},
  {"id":"surfaces","probe":"surface-listing","access":"read_only","max_pages":5}
]}
EOF

REPORTS="$(mktemp -d "$WORK/reports.XXXXXX")"

BIN="$ROOT/target/release/mcpeval"
if [ ! -x "$BIN" ]; then
  echo "build the release binary first: cargo build --release" >&2
  exit 1
fi

# Keeps the full JSON report per server; a failing battery still prints
# its report, so only a server that could not run leaves no score.
probe_report() {
  "$BIN" probe --server "$1" --manifest "$MANIFEST" --format json \
    -- "${@:2}" > "$REPORTS/$1.json" 2>/dev/null || true
}

cd "$WORK"

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
  "airbnb|@openbnb/mcp-server-airbnb|"
  "sqlite|mcp-server-sqlite|"
  "sqlite-npx|mcp-sqlite|"
  "docker|mcp-server-docker|"
  "docker-mcp|mcp-docker-server|"
  "mermaid|mermaid-mcp-server|"
  "terraform|mcp-server-terraform|"
  "tavily|tavily-mcp|"
  "ollama|ollama-mcp-server|"
  "calculator|calculator-mcp|"
  "wikipedia-npm|wikipedia-mcp|"
  "searxng|mcp-searxng|"
)

for entry in "${NPM_SERVERS[@]}"; do
  IFS='|' read -r label package args <<< "$entry"
  echo "== probing $label (npx $package) =="
  probe_report "$label" npx -y "$package" $args
done

# uvx-based servers: label|package|args...
UVX_SERVERS=(
  "fetch|mcp-server-fetch|"
  "time|mcp-server-time|"
  "markitdown|markitdown-mcp|"
  "mcp-atlassian|mcp-atlassian|"
  "pandoc|mcp-pandoc|"
  "git|mcp-server-git|"
  "arxiv|arxiv-mcp-server|"
  "wikipedia-uvx|mcp-server-wikipedia|"
)

for entry in "${UVX_SERVERS[@]}"; do
  IFS='|' read -r label package args <<< "$entry"
  echo "== probing $label (uvx $package) =="
  probe_report "$label" uvx "$package" $args
done

python3 - "$REPORTS" "$MANIFEST" "$OUT" <<'PYEOF'
import json, sys, os
reports_dir, manifest_path, out_path = sys.argv[1], sys.argv[2], sys.argv[3]
battery = [probe["probe"] for probe in json.load(open(manifest_path))["probes"]]

def measurement(report, probe, key):
    for case in report["cases"]:
        value = case.get("measurements", {}).get(key)
        if case["probe"] == probe and value is not None:
            return value
    return None

observations = []
for name in sorted(os.listdir(reports_dir)):
    server = name[: -len(".json")]
    try:
        report = json.load(open(os.path.join(reports_dir, name)))
        observation = {"server": server, "score": report["readiness"]["score"]}
    except (ValueError, KeyError, TypeError):
        print(f"   {server}: skipped (battery could not run)")
        continue
    if any((case.get("reason") or "").startswith("transport-") for case in report["cases"]):
        print(f"   {server}: skipped (transport failure)")
        continue
    for field, probe, key in (
        ("tool_count", "discovery-cost", "tool_count"),
        ("catalog_tokens", "token-cost", "total_tokens"),
    ):
        value = measurement(report, probe, key)
        if value is not None:
            observation[field] = value
    ran_discovery = any(case["probe"] == "discovery-cost" for case in report["cases"])
    if ran_discovery and not observation.get("tool_count"):
        print(f"   {server}: skipped (no tools listed)")
        continue
    observations.append(observation)
    print(f"   {server}: score={observation['score']}")
if len(observations) < 10:
    sys.exit(f"only {len(observations)} observations collected; refusing to ship a thin corpus")
doc = {
    "schema": "mcpeval.readiness-corpus/v1",
    "source": "readiness battery over popular public MCP servers, collected via scripts/corpus/collect.sh; servers requiring live credentials or services are re-run before each release",
    "battery": battery,
    "observations": sorted(observations, key=lambda o: (o["score"], o["server"])),
}
os.makedirs(os.path.dirname(out_path), exist_ok=True)
json.dump(doc, open(out_path, "w"), indent=2)
open(out_path, "a").write("\n")
scores = sorted(o["score"] for o in observations)
print(f"wrote {len(observations)} observations to {out_path} (median {scores[len(scores)//2]}, min {scores[0]})")
PYEOF