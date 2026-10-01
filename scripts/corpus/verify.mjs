#!/usr/bin/env node
// Corpus drift check: re-score every observation in
// data/readiness-corpus.json with the current binary's standard battery
// (`mcpeval score`, as the collector runs it) and fail when an area moved.
// Every area but reliability is a function of the server's code and must
// match exactly; reliability includes latency bands, which depend on the
// machine and network, so it may move by RELIABILITY_TOLERANCE. The
// corpus's claim is "reproducible by anyone"; this is the check that keeps
// it honest between releases.
//
// Both scripts run every server with the same ISOLATION environment, so no
// server reaches the machine's Kubernetes context or Docker daemon and a
// re-run sees what the collector saw.
//
// Observations also carry `tool_count` and `catalog_tokens`; those are
// informational and are not compared.
//
// A server that legitimately fixed or broke something moves its score —
// that is a deliberate corpus refresh: run scripts/corpus/collect.sh and
// commit the result with an explanation. This script exists so drift is
// never silent.
//
// Usage: node scripts/corpus/verify.mjs [--json]
//   --json   emit the per-server result document instead of prose
import { execFileSync } from "node:child_process";
import { mkdtemp, readFile, rm, stat } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../..");
const CORPUS_REL = "data/readiness-corpus.json";
const BINARY = path.join(ROOT, "target/release/mcpeval");
// The standard battery calls every read-only tool; a server whose calls
// all time out costs at most a few minutes.
const TIMEOUT_MS = 600_000;

/** One latency band (100 → 80 → 50 …) on every exercised tool. */
const RELIABILITY_TOLERANCE = 10;

/** Environment both scripts give every server; collect.sh exports the same. */
const ISOLATION = {
  KUBECONFIG: path.join(ROOT, "scripts/corpus/empty-kubeconfig.yaml"),
  DOCKER_HOST: "unix:///nonexistent/docker.sock",
};

const NPM_SERVERS = new Map([
  ["everything", ["stdio"]],
  ["memory", []],
  ["filesystem", ["/tmp"]],
  ["sequential-thinking", []],
  ["puppeteer", []],
  ["github", ["ghp-sample"]],
  ["notion", []],
  ["todoist", ["sample"]],
  ["desktop-commander", []],
  ["context7", []],
  ["browserbase", []],
  ["kubernetes", []],
  ["playwright", []],
  ["postgres", ["postgresql://localhost/invalid"]],
  ["airbnb", []],
  ["sqlite", []],
  ["sqlite-npx", []],
  ["docker", []],
  ["docker-mcp", []],
  ["mermaid", []],
  ["terraform", []],
  ["tavily", []],
  ["ollama", []],
  ["calculator", []],
  ["wikipedia-npm", []],
  ["searxng", []],
]);

const UVX_SERVERS = new Map([
  ["fetch", []],
  ["time", []],
  ["markitdown", []],
  ["mcp-atlassian", []],
  ["pandoc", []],
  ["git", []],
  ["arxiv", []],
  ["wikipedia-uvx", []],
]);

function commandFor(server) {
  if (NPM_SERVERS.has(server)) {
    return ["npx", "-y", packageFor(server), ...NPM_SERVERS.get(server)].filter(Boolean);
  }
  if (UVX_SERVERS.has(server)) {
    return ["uvx", packageFor(server), ...UVX_SERVERS.get(server)].filter(Boolean);
  }
  return null;
}

export {
  commandFor,
  ISOLATION,
  NPM_PACKAGES,
  RELIABILITY_TOLERANCE,
  UVX_PACKAGES,
  NPM_SERVERS,
  UVX_SERVERS,
};

/**
 * How a re-scored observation differs from the corpus: one entry per area
 * that moved beyond its allowance (`"catalog 64→65"`); empty when it
 * reproduces. An observation without areas compares its score.
 */
/**
 * Why the corpus cannot be re-scored here, or null. Servers may describe
 * their tools per operating system (desktop-commander does), so a catalog
 * reproduces only on the platform it was collected on.
 */
export function platformMismatch(corpus, platform) {
  if (!corpus.platform) return "corpus names no platform; recollect with scripts/corpus/collect.sh";
  if (corpus.platform !== platform) {
    return `corpus was collected on ${corpus.platform}; re-score it on ${corpus.platform}, not ${platform}`;
  }
  return null;
}

export function driftOf(expected, observed) {
  const areas = expected.areas ?? {};
  if (Object.keys(areas).length === 0) {
    return expected.score === observed.score ? [] : [`score ${expected.score}→${observed.score}`];
  }
  const moved = [];
  for (const [name, before] of Object.entries(areas)) {
    const after = observed.areas?.[name];
    const allowance = name === "reliability" ? RELIABILITY_TOLERANCE : 0;
    if (!Number.isInteger(after) || Math.abs(after - before) > allowance) {
      moved.push(`${name} ${before}→${after ?? "none"}`);
    }
  }
  return moved;
}

/** The readiness score; null when the standard battery could not start. */
export function readinessScore(document) {
  const score = document?.readiness?.score;
  return Number.isInteger(score) ? score : null;
}

export function scoreArguments(server) {
  return ["score", "--server", server, "--format", "json"];
}

// label -> package name; kept here rather than re-parsed from collect.sh so
// the drift check is explicit about what each label launches. collect.sh's
// arrays and these maps must agree; the tests below cross-check them.
const NPM_PACKAGES = new Map([
  ["everything", "@modelcontextprotocol/server-everything"],
  ["memory", "@modelcontextprotocol/server-memory"],
  ["filesystem", "@modelcontextprotocol/server-filesystem"],
  ["sequential-thinking", "@modelcontextprotocol/server-sequential-thinking"],
  ["puppeteer", "@modelcontextprotocol/server-puppeteer"],
  ["github", "@modelcontextprotocol/server-github"],
  ["notion", "@notionhq/notion-mcp-server"],
  ["todoist", "mcp-todoist"],
  ["desktop-commander", "@wonderwhy-er/desktop-commander"],
  ["context7", "@upstash/context7-mcp"],
  ["browserbase", "@browserbasehq/mcp-server-browserbase"],
  ["kubernetes", "mcp-server-kubernetes"],
  ["playwright", "@executeautomation/playwright-mcp-server"],
  ["postgres", "@modelcontextprotocol/server-postgres"],
  ["airbnb", "@openbnb/mcp-server-airbnb"],
  ["sqlite", "mcp-server-sqlite"],
  ["sqlite-npx", "mcp-sqlite"],
  ["docker", "mcp-server-docker"],
  ["docker-mcp", "mcp-docker-server"],
  ["mermaid", "mermaid-mcp-server"],
  ["terraform", "mcp-server-terraform"],
  ["tavily", "tavily-mcp"],
  ["ollama", "ollama-mcp-server"],
  ["calculator", "calculator-mcp"],
  ["wikipedia-npm", "wikipedia-mcp"],
  ["searxng", "mcp-searxng"],
]);

const UVX_PACKAGES = new Map([
  ["fetch", "mcp-server-fetch"],
  ["time", "mcp-server-time"],
  ["markitdown", "markitdown-mcp"],
  ["mcp-atlassian", "mcp-atlassian"],
  ["pandoc", "mcp-pandoc"],
  ["git", "mcp-server-git"],
  ["arxiv", "arxiv-mcp-server"],
  ["wikipedia-uvx", "mcp-server-wikipedia"],
]);

function packageFor(server) {
  return NPM_PACKAGES.get(server) ?? UVX_PACKAGES.get(server);
}

function observed(document) {
  const score = readinessScore(document);
  if (score === null) return { status: "could-not-run", observed: null };
  const areas = Object.fromEntries(
    (document.readiness.areas ?? []).map((area) => [area.name, area.score]),
  );
  return { status: "observed", observed: score, areas };
}

async function probeScore(server, work) {
  const command = commandFor(server);
  if (!command) return { status: "unknown-server", expected: null, observed: null };
  const home = path.join(work, `${server}-home`);
  try {
    const stdout = execFileSync(BINARY, [...scoreArguments(server), "--", ...command], {
      timeout: TIMEOUT_MS,
      encoding: "utf8",
      cwd: work,
      env: { ...process.env, ...ISOLATION, MCPEVAL_HOME: home },
      stdio: ["ignore", "pipe", "ignore"],
    });
    return observed(JSON.parse(stdout));
  } catch (error) {
    // A non-zero exit can still print the JSON report on stdout; only a
    // missing report or readiness counts as "could not run".
    if (error.stdout) {
      try {
        return observed(JSON.parse(error.stdout));
      } catch {
        // fall through
      }
    }
    return { status: "could-not-run", observed: null };
  }
}

export async function verifyCorpus(options = {}) {
  const binaryStat = await stat(BINARY).catch(() => null);
  if (!binaryStat?.isFile()) {
    throw new Error("build the release binary first: cargo build --release");
  }
  const corpus = JSON.parse(await readFile(path.join(ROOT, CORPUS_REL), "utf8"));
  if (corpus.schema !== "mcpeval.readiness-corpus/v2" || !corpus.standard) {
    throw new Error(`unsupported corpus schema ${corpus.schema}`);
  }
  const mismatch = platformMismatch(corpus, process.platform);
  if (mismatch) {
    throw new Error(mismatch);
  }
  const work = options.work ?? (await mkdtemp(path.join(os.tmpdir(), "mcpeval-corpus-verify-")));
  const results = [];
  for (const observation of corpus.observations) {
    const outcome = await probeScore(observation.server, work);
    const moved =
      outcome.status === "observed"
        ? driftOf(observation, { score: outcome.observed, areas: outcome.areas })
        : [];
    results.push({
      server: observation.server,
      expected: observation.score,
      expectedAreas: observation.areas ?? {},
      ...outcome,
      moved,
      drifted: moved.length > 0,
    });
  }
  if (!options.keepWork) {
    await rm(work, { recursive: true, force: true });
  }
  return { corpusPath: CORPUS_REL, observations: corpus.observations.length, results };
}

export function driftedResults(results) {
  return results.filter((result) => result.drifted || result.status === "could-not-run");
}

if (process.argv[1] && import.meta.filename === process.argv[1]) {
  const asJson = process.argv.includes("--json");
  verifyCorpus().then(({ observations, results }) => {
    if (asJson) {
      process.stdout.write(`${JSON.stringify({ observations, results }, null, 2)}\n`);
    } else {
      for (const result of results) {
        const mark = result.drifted ? "DRIFT" : result.status === "could-not-run" ? "UNRUN" : "ok";
        const score = result.status === "observed" ? result.observed : "-";
        // Every area that changed, within its allowance or not.
        const changed = Object.entries(result.expectedAreas)
          .filter(([name, before]) => result.areas && result.areas[name] !== before)
          .map(([name, before]) => `${name} ${before}→${result.areas[name] ?? "none"}`);
        const moved = changed.length ? ` (${changed.join(", ")})` : "";
        console.log(
          `${mark.padEnd(5)} ${result.server.padEnd(20)} expected=${result.expected} observed=${score}${moved}`,
        );
      }
    }
    const bad = driftedResults(results);
    if (bad.length > 0) {
      console.error(`corpus drift: ${bad.map((result) => result.server).join(", ")}`);
      process.exitCode = 1;
    } else {
      console.log(`corpus verified: ${observations} observations reproduce`);
    }
  }).catch((error) => {
    process.stderr.write(`${error instanceof Error ? error.message : String(error)}\n`);
    process.exitCode = 1;
  });
}