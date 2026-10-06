import { createHash } from "node:crypto";
import { readFile } from "node:fs/promises";
import path from "node:path";
import { spawn, execFileSync } from "node:child_process";
import { validateDeployment, deploymentMatches } from "./deployment.mjs";

export const ROOT = path.resolve(import.meta.dirname, "../..");
export const SCHEMA = "mcpeval.readiness-corpus/v3";
export const AREAS = ["protocol", "catalog", "context", "error-honesty", "reliability", "coverage"];
export const COUNTS = ["successful_calls", "tool_errors", "rpc_errors", "rejected_calls", "transport_errors", "untested_tools"];
export const ISOLATION = { KUBECONFIG: path.join(ROOT, "scripts/corpus/empty-kubeconfig.yaml"), DOCKER_HOST: "unix:///nonexistent/docker.sock" };
export const sha256 = (bytes) => createHash("sha256").update(bytes).digest("hex");
export const digestFile = async (file) => sha256(await readFile(file));
export const scoreArguments = (server) => ["score", "--server", server, "--format", "json"];
const label = (value) => typeof value === "string" && /^[A-Za-z][A-Za-z0-9_.:-]{0,127}$/u.test(value);
const digest = (value) => typeof value === "string" && /^[a-f0-9]{64}$/u.test(value);
const count = (value) => Number.isSafeInteger(value) && value >= 0;
const score = (value) => count(value) && value <= 100;
const explicitCommand = (value) => Array.isArray(value) && value.length > 0 && value.every((arg) => typeof arg === "string" && arg.length > 0 && arg.length <= 8192 && !arg.includes("\0"));
function require(condition, message) { if (!condition) throw new Error(message); }

export function commandFor(target) {
  require(target && label(target.server), "target requires a server label");
  require(["npm", "uvx"].includes(target.runtime), "target requires npm or uvx runtime");
  const packagePattern = target.runtime === "npm" ? /^(?:@[a-z0-9._-]+\/)?[a-z0-9][a-z0-9._-]{0,127}$/u : /^[a-zA-Z0-9][a-zA-Z0-9._-]{0,127}$/u;
  const versionPattern = target.runtime === "npm" ? /^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?$/u : /^\d+(?:\.\d+){1,3}(?:(?:a|b|rc|\.post|\.dev)\d+)?$/u;
  require(typeof target.package === "string" && packagePattern.test(target.package), "invalid package name");
  require(typeof target.version === "string" && versionPattern.test(target.version), "target requires an exact package version");
  require(typeof target.bin === "string" && /^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$/u.test(target.bin), "target requires an explicit executable name");
  require(Array.isArray(target.args) && target.args.every((arg) => typeof arg === "string" && arg.length <= 8192 && !arg.includes("\0")), "target requires an argument array");
  if (target.deployment !== undefined) {
    const deployment = validateDeployment(target.deployment);
    return [path.join(deployment.root, deployment.executable), ...(deployment.entrypoint === undefined ? [] : [path.join(deployment.root, deployment.entrypoint)]), ...target.args];
  }
  return target.runtime === "npm"
    ? ["npx", "--yes", "--package", `${target.package}@${target.version}`, "--", target.bin, ...target.args]
    : ["uvx", "--from", `${target.package}==${target.version}`, target.bin, ...target.args];
}

export function validateTargets(document) {
  require(document?.schema === "mcpeval.corpus-targets/v1", "unsupported targets schema");
  require(/^mcpeval-standard\/\d+$/u.test(document.standard ?? ""), "targets require a standard");
  require(Array.isArray(document.targets) && document.targets.length > 0, "targets are empty");
  const seen = new Set();
  for (const target of document.targets) {
    commandFor(target);
    require(!seen.has(target.server), "duplicate target label"); seen.add(target.server);
    const prerequisites = target.prerequisites;
    require(prerequisites && Array.isArray(prerequisites.environment) && Array.isArray(prerequisites.checks), "declare prerequisite environment and checks, including empty arrays");
    require(prerequisites.environment.every((name) => typeof name === "string" && /^[A-Z_][A-Z0-9_]{0,127}$/u.test(name) && !["HOME", "USERPROFILE", "MCPEVAL_HOME", "KUBECONFIG", "DOCKER_HOST"].includes(name)), "invalid prerequisite environment name");
    require(new Set(prerequisites.environment).size === prerequisites.environment.length, "duplicate environment prerequisite");
    const names = new Set();
    for (const check of prerequisites.checks) {
      require(label(check.name) && !names.has(check.name), "invalid or duplicate prerequisite check"); names.add(check.name);
      require(explicitCommand(check.command), "prerequisite check requires an explicit command array");
    }
    require(prerequisites.state_checks === undefined || Array.isArray(prerequisites.state_checks), "state checks require an array");
    for (const check of prerequisites.state_checks ?? []) {
      require(check && label(check.name) && !names.has(check.name) && digest(check.sha256), "invalid or duplicate state check identity"); names.add(check.name);
      require(explicitCommand(check.command), "state check requires an explicit command array");
    }
  }
  return document;
}

export function readinessScore(document) { return score(document?.readiness?.score) ? document.readiness.score : null; }

export function observationFrom(raw, target, evaluator, standard) {
  let document;
  try { document = JSON.parse(raw); } catch { throw new Error("invalid-report"); }
  require(document.schema === "mcpeval.probe-report/v2" && document.server === target.server && document.generator?.name === "mcpeval" && document.generator.version === evaluator.version, "report-identity-mismatch");
  if (document.readiness === null) return { status: "errored", reason: "readiness-unmeasured" };
  const readiness = document.readiness;
  require(readiness?.standard === standard && readinessScore(document) !== null, "report-standard-or-score-mismatch");
  require(Array.isArray(readiness.areas) && readiness.areas.length === AREAS.length, "invalid-report-areas");
  const areas = {};
  for (const area of readiness.areas) {
    require(AREAS.includes(area.name) && !(area.name in areas) && score(area.score), "invalid-report-area");
    areas[area.name] = area.score;
  }
  const surface = readiness.surface;
  require(surface && ["tools", "read_only", "writers", "exercised"].every((key) => count(surface[key])) && surface.read_only + surface.writers === surface.tools && surface.exercised <= surface.read_only, "invalid-report-surface");
  const calls = readiness.areas.find((area) => area.name === "reliability").measurements;
  require(calls && COUNTS.every((key) => count(calls[key])) && calls.untested_tools <= surface.read_only, "invalid-report-call-counts");
  const tokens = readiness.areas.find((area) => area.name === "context").measurements?.catalog_tokens;
  require(count(tokens), "invalid-report-catalog");
  if (surface.tools === 0) return { status: "untested", reason: "no-tools" };
  if (COUNTS.slice(0, -1).every((key) => calls[key] === 0)) return { status: "untested", reason: "no-tool-calls" };
  return { status: "observed", observation: { server: target.server, score: readiness.score, areas, tool_count: surface.tools, catalog_tokens: tokens,
    calls: Object.fromEntries(COUNTS.map((key) => [key, calls[key]])), provenance: {
      runtime: target.runtime, package: target.package, version: target.version, bin: target.bin,
      ...(target.deployment === undefined ? {} : { deployment_sha256: target.deployment.sha256 }),
      ...((target.prerequisites.state_checks?.length ?? 0) === 0 ? {} : { state_checks: target.prerequisites.state_checks.map(({ name, sha256 }) => ({ name, sha256 })) }),
      launch_sha256: sha256(JSON.stringify(commandFor(target))), report_sha256: sha256(raw),
      prerequisites: [...target.prerequisites.environment.map((name) => `env:${name}`), ...target.prerequisites.checks.map((check) => `check:${check.name}`), ...(target.prerequisites.state_checks ?? []).map((check) => `state:${check.name}`)],
    } } };
}

export function validateCorpus(corpus) {
  require(corpus?.schema === SCHEMA, "corpus lacks v3 provenance; historical data cannot be replayed as a pinned collection");
  require(typeof corpus.source === "string" && corpus.source.length > 0, "invalid corpus source");
  require(typeof corpus.platform === "string" && corpus.platform.length > 0 && typeof corpus.standard === "string" && /^mcpeval-standard\/\d+$/u.test(corpus.standard), "invalid corpus context");
  require(typeof corpus.evaluator?.version === "string" && corpus.evaluator.version.length > 0 && digest(corpus.evaluator.sha256) && digest(corpus.targets_sha256), "invalid evaluator or targets provenance");
  require(Array.isArray(corpus.population) && corpus.population.length > 0 && Array.isArray(corpus.observations), "invalid corpus population");
  const seen = new Set(); const observed = new Set();
  for (const entry of corpus.population) {
    require(label(entry.server) && !seen.has(entry.server) && ["observed", "untested", "errored"].includes(entry.status), "invalid or duplicate population entry"); seen.add(entry.server);
    if (entry.status === "observed") { require(entry.reason === undefined, "observed target cannot have a failure reason"); observed.add(entry.server); }
    else require(["missing-environment", "prerequisite-failed", "no-tools", "no-tool-calls", "readiness-unmeasured", "evaluation-failed", "invalid-report", "deployment-mismatch", "state-check-failed", "state-mismatch"].includes(entry.reason), "invalid unobserved reason");
  }
  const rows = new Set();
  for (const observation of corpus.observations) {
    require(observed.has(observation.server) && !rows.has(observation.server) && score(observation.score), "observation does not match population"); rows.add(observation.server);
    require(AREAS.every((key) => score(observation.areas?.[key])) && Object.keys(observation.areas).length === AREAS.length, "invalid corpus areas");
    require(count(observation.tool_count) && observation.tool_count > 0 && count(observation.catalog_tokens) && COUNTS.every((key) => count(observation.calls?.[key])) && COUNTS.slice(0, -1).some((key) => observation.calls[key] > 0), "invalid corpus measurements");
    const source = observation.provenance;
    require(source && digest(source.report_sha256) && digest(source.launch_sha256) && Array.isArray(source.prerequisites) && source.prerequisites.every((name) => /^(env|check|state):[A-Za-z_][A-Za-z0-9_.:-]{0,127}$/u.test(name)) && new Set(source.prerequisites).size === source.prerequisites.length, "invalid observation provenance");
    require(source.deployment_sha256 === undefined || digest(source.deployment_sha256), "invalid deployment provenance");
    require(source.state_checks === undefined || Array.isArray(source.state_checks), "invalid state provenance");
    const states = new Set();
    for (const check of source.state_checks ?? []) {
      require(check && label(check.name) && digest(check.sha256) && !states.has(check.name) && source.prerequisites.includes(`state:${check.name}`), "invalid or duplicate state provenance");
      states.add(check.name);
    }
    require(states.size === source.prerequisites.filter((name) => name.startsWith("state:")).length, "incomplete state provenance");
    commandFor({ ...source, server: observation.server, args: [] });
  }
  require(rows.size === observed.size, "population observations are incomplete");
  return corpus;
}

/** Bounded process group: timeouts and output overflow reap descendants too. */
export async function runCommand(command, options = {}) {
  return new Promise((resolve) => {
    const child = spawn(command[0], command.slice(1), { cwd: options.cwd, env: options.env, detached: process.platform !== "win32", stdio: ["ignore", "pipe", "ignore"] });
    const chunks = []; let size = 0; let reason = null;
    const stop = () => {
      if (!child.pid) return;
      try {
        if (process.platform === "win32") execFileSync("taskkill", ["/pid", String(child.pid), "/T", "/F"], { stdio: "ignore", timeout: 5000 });
        else process.kill(-child.pid, "SIGKILL");
      } catch { /* The process group may already have exited. */ }
    };
    const timer = setTimeout(() => { reason = "timeout"; stop(); }, options.timeoutMs ?? 350_000);
    child.stdout.on("data", (chunk) => {
      size += chunk.length;
      if (size > (options.maxBytes ?? 8 * 1024 * 1024)) { reason = "output-limit"; stop(); }
      else chunks.push(chunk);
    });
    child.on("error", () => { reason = "launch-failed"; });
    child.on("close", (code) => { clearTimeout(timer); stop(); resolve({ code, reason, stdout: Buffer.concat(chunks).toString("utf8") }); });
  });
}

export async function evaluatorIdentity(binary, execute = runCommand) {
  const result = await execute([binary, "--version"], { timeoutMs: 10_000, maxBytes: 1024 });
  const match = /^mcpeval ([0-9A-Za-z.+-]+)\s*$/u.exec(result.stdout);
  require(!result.reason && result.code === 0 && match, "invalid evaluator version");
  return { version: match[1], sha256: await digestFile(binary) };
}

export function environmentFor(target, home, environment = process.env) {
  const env = {};
  for (const name of ["PATH", "SystemRoot", "WINDIR", "COMSPEC", "PATHEXT", "TMPDIR", "TEMP", "TMP", "LANG", "LC_ALL", ...target.prerequisites.environment]) {
    if (environment[name] !== undefined) env[name] = environment[name];
  }
  return { ...env, ...ISOLATION, HOME: home, USERPROFILE: home, MCPEVAL_HOME: path.join(home, "mcpeval") };
}

async function stateCheckFailure(target, context, env) {
  for (const check of target.prerequisites.state_checks ?? []) {
    const result = await context.execute(check.command, { env, cwd: context.home, timeoutMs: 10_000, maxBytes: 1024 });
    const identity = /^([a-f0-9]{64})(?:\r?\n)?$/u.exec(result.stdout ?? "");
    if (result.reason || result.code !== 0 || !identity) return "state-check-failed";
    if (identity[1] !== check.sha256) return "state-mismatch";
  }
  return null;
}

export async function evaluateTarget(target, context) {
  if (target.prerequisites.environment.some((name) => !context.environment[name]?.trim())) return { status: "untested", reason: "missing-environment" };
  const mismatch = { status: "errored", reason: "deployment-mismatch" };
  if (!await deploymentMatches(target.deployment)) return mismatch;
  const env = environmentFor(target, context.home, context.environment);
  for (const check of target.prerequisites.checks) {
    const result = await context.execute(check.command, { env, cwd: context.home, timeoutMs: 10_000, maxBytes: 1024 });
    if (!await deploymentMatches(target.deployment)) return mismatch;
    if (result.reason || result.code !== 0) return { status: "untested", reason: "prerequisite-failed" };
  }
  const before = await stateCheckFailure(target, context, env);
  if (target.prerequisites.state_checks?.length && !await deploymentMatches(target.deployment)) return mismatch;
  if (before) return { status: before === "state-mismatch" ? "errored" : "untested", reason: before };
  const result = await context.execute([context.binary, ...scoreArguments(target.server), "--", ...commandFor(target)], { env, cwd: context.home });
  const after = await stateCheckFailure(target, context, env);
  if (!await deploymentMatches(target.deployment)) return mismatch;
  if (after) return { status: "errored", reason: after };
  if (result.reason || ![0, 2].includes(result.code)) return { status: "errored", reason: "evaluation-failed" };
  try { return { ...observationFrom(result.stdout, target, context.evaluator, context.standard), raw: result.stdout }; }
  catch { return { status: "errored", reason: "invalid-report" }; }
}
