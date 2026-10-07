#!/usr/bin/env node
import { link, mkdtemp, mkdir, rm, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { parseArgs, isDeepStrictEqual } from "node:util";
import { ROOT, digestFile, evaluatorIdentity, evaluateTarget, runCommand, requireDeployments } from "./contract.mjs";
import { readCollection, replayResult, verificationFor } from "./replay-contract.mjs";
export { commandFor, ISOLATION, readinessScore, scoreArguments } from "./contract.mjs";
export { RELIABILITY_TOLERANCE, VERIFICATION_SCHEMA, driftOf, driftedResults } from "./replay-contract.mjs";

export function platformMismatch(corpus, platform) {
  return !corpus.platform ? "corpus names no platform" : corpus.platform !== platform ? `corpus was collected on ${corpus.platform}; use that platform for verification` : null;
}

export async function verifyCorpus(options = {}) {
  // Validate provenance before touching a binary, package runner, or service.
  const collection = await readCollection(options);
  const { corpus, targets } = collection;
  if (options.requireDeployments) requireDeployments(targets);
  const mismatch = platformMismatch(corpus, process.platform); if (mismatch) throw new Error(mismatch);
  const binary = path.resolve(options.binary ?? path.join(ROOT, "target/release/mcpeval"));
  if (await digestFile(binary) !== corpus.evaluator.sha256) throw new Error("evaluator provenance mismatch");
  const evidencePath = options.evidencePath === undefined ? null : path.resolve(options.evidencePath);
  // Exclusive reservation precedes execution. A rejected reservation is never cleaned up.
  if (evidencePath) await mkdir(evidencePath, { mode: 0o700 });
  let published = false;
  try {
    if (evidencePath) await mkdir(path.join(evidencePath, "reports"), { mode: 0o700 });
    const execute = options.execute ?? runCommand;
    const evaluator = await evaluatorIdentity(binary, execute);
    if (!isDeepStrictEqual(evaluator, corpus.evaluator)) throw new Error("evaluator provenance mismatch");
    const work = await mkdtemp(path.join(os.tmpdir(), "mcpeval-corpus-verify-"));
    const results = [];
    try {
      for (const entry of corpus.population) {
        if (entry.status !== "observed") {
          results.push(replayResult(entry));
          continue;
        }
        if (await digestFile(binary) !== evaluator.sha256) throw new Error("evaluator changed during verification");
        const target = targets.targets.find((item) => item.server === entry.server);
        const home = path.join(work, entry.server); await mkdir(home);
        const result = await evaluateTarget(target, { binary, evaluator, standard: corpus.standard, home, execute, environment: options.environment ?? process.env });
        if (evidencePath && result.raw !== undefined) await writeFile(path.join(evidencePath, "reports", `${entry.server}.json`), result.raw, { flag: "wx", mode: 0o600 });
        const expected = corpus.observations.find((item) => item.server === entry.server);
        results.push(replayResult(entry, expected, result));
      }
      if (await digestFile(binary) !== evaluator.sha256) throw new Error("evaluator changed during verification");
    } finally { await rm(work, { recursive: true, force: true }); }
    const verification = verificationFor(collection, results);
    // The verification file is the completion marker, including for a failed verdict.
    if (evidencePath) {
      const temporary = path.join(evidencePath, "verification.json.tmp");
      await writeFile(temporary, `${JSON.stringify(verification, null, 2)}\n`, { flag: "wx", mode: 0o600 });
      await link(temporary, path.join(evidencePath, "verification.json"));
      await rm(temporary);
    }
    published = true;
    return verification;
  } finally {
    if (evidencePath && !published) await rm(evidencePath, { recursive: true, force: true });
  }
}

if (process.argv[1] && path.resolve(process.argv[1]) === import.meta.filename) {
  try {
    const { values } = parseArgs({ options: { corpus: { type: "string" }, targets: { type: "string" }, reports: { type: "string" }, binary: { type: "string" }, out: { type: "string" }, json: { type: "boolean", default: false }, "require-deployments": { type: "boolean", default: false } } });
    const result = await verifyCorpus({ corpusPath: values.corpus, targetsPath: values.targets, reportsPath: values.reports, binary: values.binary, evidencePath: values.out, requireDeployments: values["require-deployments"] });
    if (values.json) console.log(JSON.stringify(result, null, 2));
    else for (const entry of result.results) console.log(`${entry.drifted ? "DRIFT" : entry.status} ${entry.server} expected=${entry.expected ?? "-"} observed=${entry.observed ?? "-"}`);
    process.exitCode = result.passed ? 0 : 1;
  } catch (error) { console.error(error.message); process.exitCode = 1; }
}
