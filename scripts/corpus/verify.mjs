#!/usr/bin/env node
// Corpus drift check: re-probe every observation in
// data/readiness-corpus.json with the current binary and the same generic
// manifest the collector uses, and fail on any score that moved. The
// corpus's only claim is "deterministic verdict, reproducible by anyone";
// this is the check that keeps the claim honest between releases.
//
// A server that legitimately fixed or broke something moves its score —
// that is a deliberate corpus refresh: run scripts/corpus/collect.sh and
// commit the result with an explanation. This script exists so drift is
// never silent.
//
// Usage: node scripts/corpus/verify.mjs [--json]
//   --json   emit the per-server result document instead of prose
import { execFileSync } from "node:child_process";
import { mkdtemp, readFile, rm, stat, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../..");
const CORPUS_REL = "data/readiness-corpus.json";
const BINARY = path.join(ROOT, "target/release/mcpeval");
const TIMEOUT_MS = 120_000;

const MANIFEST = {
  version: 1,
  probes: [
    { id: "discovery-budget", probe: "discovery-cost", access: "read_only", max_tools: 80, max_schema_bytes: 400000 },
    { id: "token-budget", probe: "token-cost", access: "read_only", max_total_tokens: 200000, max_tool_tokens: 40000 },
    { id: "pages", probe: "pagination", access: "read_only", max_pages: 5 },
    { id: "surfaces", probe: "surface-listing", access: "read_only", max_pages: 5 },
  ],
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

export { commandFor, NPM_PACKAGES, UVX_PACKAGES, NPM_SERVERS, UVX_SERVERS };

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

async function probeScore(server, work) {
  const command = commandFor(server);
  if (!command) return { status: "unknown-server", expected: null, observed: null };
  const manifestPath = path.join(work, `${server}.manifest.json`);
  await writeFile(manifestPath, JSON.stringify(MANIFEST));
  const home = path.join(work, `${server}-home`);
  try {
    const stdout = execFileSync(BINARY, [
      "probe",
      "--server",
      server,
      "--manifest",
      manifestPath,
      "--format",
      "json",
      "--",
      ...command,
    ], {
      timeout: TIMEOUT_MS,
      encoding: "utf8",
      cwd: work,
      env: { ...process.env, MCPEVAL_HOME: home },
      stdio: ["ignore", "pipe", "ignore"],
    });
    const document = JSON.parse(stdout);
    return { status: "observed", observed: document.readiness.score };
  } catch (error) {
    // A failing battery exits non-zero but still prints the JSON report on
    // stdout; only a missing report counts as "could not run".
    if (error.stdout) {
      try {
        const document = JSON.parse(error.stdout);
        return { status: "observed", observed: document.readiness.score };
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
  if (corpus.schema !== "mcpeval.readiness-corpus/v1") {
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