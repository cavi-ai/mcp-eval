#!/usr/bin/env bash
# Collect readiness scores from popular public MCP servers into
# data/readiness-corpus.json. Each server is scored by mcpeval's standard
# battery (`mcpeval score`, no manifest), so the corpus is comparable
# across heterogeneous servers. The document names the standard, and each
# observation carries its area scores, tool count, and catalog token
# estimate beside the score.
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

# Every server runs with no Kubernetes context and no Docker daemon, so the
# battery's read-only calls never reach this machine's cluster or
# containers; scripts/corpus/verify.mjs uses the same ISOLATION.
export KUBECONFIG="$ROOT/scripts/corpus/empty-kubeconfig.yaml"
export DOCKER_HOST=unix:///nonexistent/docker.sock

REPORTS="$(mktemp -d "$WORK/reports.XXXXXX")"

BIN="$ROOT/target/release/mcpeval"
if [ ! -x "$BIN" ]; then
  echo "build the release binary first: cargo build --release" >&2
  exit 1
fi

# Keeps the full JSON report per server; only a server whose standard
# battery could not start leaves no score.
probe_report() {
  "$BIN" score --server "$1" --format json \
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

python3 - "$REPORTS" "$OUT" <<'PYEOF'
import json, sys, os
reports_dir, out_path = sys.argv[1], sys.argv[2]

observations = []
standards = set()
for name in sorted(os.listdir(reports_dir)):
    server = name[: -len(".json")]
    try:
        readiness = json.load(open(os.path.join(reports_dir, name)))["readiness"]
        observation = {
            "server": server,
            "score": readiness["score"],
            "areas": {area["name"]: area["score"] for area in readiness["areas"]},
            "tool_count": readiness["surface"]["tools"],
        }
        standard = readiness["standard"]
    except (ValueError, KeyError, TypeError):
        print(f"   {server}: skipped (standard battery could not run)")
        continue
    if not observation["tool_count"]:
        print(f"   {server}: skipped (no tools listed)")
        continue
    for area in readiness["areas"]:
        tokens = area.get("measurements", {}).get("catalog_tokens")
        if area["name"] == "context" and tokens is not None:
            observation["catalog_tokens"] = tokens
    standards.add(standard)
    observations.append(observation)
    print(f"   {server}: score={observation['score']}")
if len(observations) < 10:
    sys.exit(f"only {len(observations)} observations collected; refusing to ship a thin corpus")
if len(standards) != 1:
    sys.exit(f"observations span standards {sorted(standards)}; collect with one build")
doc = {
    "schema": "mcpeval.readiness-corpus/v2",
    "source": "mcpeval standard battery over popular public MCP servers, collected via scripts/corpus/collect.sh; servers requiring live credentials or services run without them",
    "standard": standards.pop(),
    "observations": sorted(observations, key=lambda o: (o["score"], o["server"])),
}
os.makedirs(os.path.dirname(out_path), exist_ok=True)
json.dump(doc, open(out_path, "w"), indent=2)
open(out_path, "a").write("\n")
scores = sorted(o["score"] for o in observations)
print(f"wrote {len(observations)} observations to {out_path} (median {scores[len(scores)//2]}, min {scores[0]}, at 100: {scores.count(100)})")
PYEOF