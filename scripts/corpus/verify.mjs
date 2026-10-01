#!/usr/bin/env node
// Corpus drift check: re-score every observation in
// data/readiness-corpus.json with the current binary's standard battery
// (`mcpeval score`, as the collector runs it) and fail on any score that
// moved. The corpus's only claim is "deterministic verdict, reproducible by
// anyone"; this is the check that keeps the claim honest between releases.
//
// Both scripts run every server with the same ISOLATION environment, so no
// server reaches the machine's Kubernetes context or Docker daemon and a
// re-run sees what the collector saw.
//
// Observations also carry `areas`, `tool_count`, and `catalog_tokens`;
// those are informational and are not compared.
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

export { commandFor, ISOLATION, NPM_PACKAGES, UVX_PACKAGES, NPM_SERVERS, UVX_SERVERS };

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
  return score === null
    ? { status: "could-not-run", observed: null }
    : { status: "observed", observed: score };
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
  const work = options.work ?? (await mkdtemp(path.join(os.tmpdir(), "mcpeval-corpus-verify-")));
  const results = [];
  for (const observation of corpus.observations) {
    const outcome = await probeScore(observation.server, work);
    const drifted =
      outcome.status === "observed" && outcome.observed !== observation.score;
    results.push({ server: observation.server, expected: observation.score, ...outcome, drifted });
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
        console.log(`${mark.padEnd(5)} ${result.server.padEnd(20)} expected=${result.expected} observed=${score}`);
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