#!/usr/bin/env node
import { mkdtemp, mkdir, readFile, rm } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { parseArgs, isDeepStrictEqual } from "node:util";
import { ROOT, validateCorpus, validateTargets, sha256, digestFile, evaluatorIdentity, evaluateTarget, observationFrom, runCommand } from "./contract.mjs";
export { commandFor, ISOLATION, readinessScore, scoreArguments } from "./contract.mjs";

export const RELIABILITY_TOLERANCE = 10;
export function platformMismatch(corpus, platform) {
  return !corpus.platform ? "corpus names no platform" : corpus.platform !== platform ? `corpus was collected on ${corpus.platform}; use that platform for verification` : null;
}
export function driftOf(expected, observed) {
  return Object.entries(expected.areas).flatMap(([name, before]) => {
    const after = observed.areas[name];
    return !Number.isInteger(after) || Math.abs(after - before) > (name === "reliability" ? RELIABILITY_TOLERANCE : 0) ? [`${name} ${before}→${after ?? "none"}`] : [];
  });
}
export function driftedResults(results) { return results.filter((result) => result.status !== "observed" || result.drifted); }

export async function verifyCorpus(options = {}) {
  // Validate provenance before touching a binary, package runner, or service.
  const corpus = validateCorpus(JSON.parse(await readFile(options.corpusPath ?? path.join(ROOT, "data/readiness-corpus.json"), "utf8")));
  if (!options.targetsPath || !options.reportsPath) throw new Error("verification requires --targets and --reports from collection");
  const bytes = await readFile(options.targetsPath);
  const targets = validateTargets(JSON.parse(bytes));
  if (sha256(bytes) !== corpus.targets_sha256 || targets.standard !== corpus.standard || !isDeepStrictEqual(targets.targets.map((target) => target.server), corpus.population.map((entry) => entry.server))) throw new Error("targets provenance mismatch");
  const mismatch = platformMismatch(corpus, process.platform); if (mismatch) throw new Error(mismatch);
  const binary = path.resolve(options.binary ?? path.join(ROOT, "target/release/mcpeval"));
  if (await digestFile(binary) !== corpus.evaluator.sha256) throw new Error("evaluator provenance mismatch");
  const execute = options.execute ?? runCommand;
  const evaluator = await evaluatorIdentity(binary, execute);
  if (!isDeepStrictEqual(evaluator, corpus.evaluator)) throw new Error("evaluator provenance mismatch");
  // Check the original report bytes and projection before re-scoring anything.
  for (const expected of corpus.observations) {
    const raw = await readFile(path.join(options.reportsPath, `${expected.server}.json`), "utf8");
    if (sha256(raw) !== expected.provenance.report_sha256) throw new Error("original report digest mismatch");
    const target = targets.targets.find((item) => item.server === expected.server);
    const result = observationFrom(raw, target, evaluator, corpus.standard);
    if (result.status !== "observed" || !isDeepStrictEqual(result.observation, expected)) throw new Error("original report projection mismatch");
  }
  const work = await mkdtemp(path.join(os.tmpdir(), "mcpeval-corpus-verify-"));
  const results = [];
  try {
    for (const entry of corpus.population) {
      if (entry.status !== "observed") { results.push({ ...entry, drifted: false }); continue; }
      if (await digestFile(binary) !== evaluator.sha256) throw new Error("evaluator changed during verification");
      const target = targets.targets.find((item) => item.server === entry.server);
      const home = path.join(work, entry.server); await mkdir(home);
      const result = await evaluateTarget(target, { binary, evaluator, standard: corpus.standard, home, execute, environment: options.environment ?? process.env });
      const expected = corpus.observations.find((item) => item.server === entry.server);
      const moved = result.observation ? driftOf(expected, result.observation) : [];
      results.push({ server: entry.server, status: result.status, ...(result.reason ? { reason: result.reason } : {}), expected: expected.score, observed: result.observation?.score ?? null, moved, drifted: moved.length > 0 });
    }
    if (await digestFile(binary) !== evaluator.sha256) throw new Error("evaluator changed during verification");
  } finally { await rm(work, { recursive: true, force: true }); }
  return { observations: corpus.observations.length, population: corpus.population.length, results };
}

if (process.argv[1] && path.resolve(process.argv[1]) === import.meta.filename) {
  try {
    const { values } = parseArgs({ options: { corpus: { type: "string" }, targets: { type: "string" }, reports: { type: "string" }, binary: { type: "string" }, json: { type: "boolean", default: false } } });
    const result = await verifyCorpus({ corpusPath: values.corpus, targetsPath: values.targets, reportsPath: values.reports, binary: values.binary });
    if (values.json) console.log(JSON.stringify(result, null, 2));
    else for (const entry of result.results) console.log(`${entry.drifted ? "DRIFT" : entry.status} ${entry.server} expected=${entry.expected ?? "-"} observed=${entry.observed ?? "-"}`);
    process.exitCode = driftedResults(result.results).length ? 1 : 0;
  } catch (error) { console.error(error.message); process.exitCode = 1; }
}
