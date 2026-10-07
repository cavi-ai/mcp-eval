#!/usr/bin/env node
import { mkdtemp, mkdir, readFile, rename, rm, lstat, link, writeFile } from "node:fs/promises";
import { randomUUID } from "node:crypto";
import os from "node:os";
import path from "node:path";
import { parseArgs } from "node:util";
import { ROOT, SCHEMA, digestFile, sha256, validateTargets, validateCorpus, evaluatorIdentity, evaluateTarget, runCommand, requireDeployments } from "./contract.mjs";

export async function collectCorpus(options) {
  if (!options?.targetsPath || !options.output) throw new Error("collection requires --targets and --out");
  const bytes = await readFile(options.targetsPath);
  const targets = validateTargets(JSON.parse(bytes));
  if (options.requireDeployments) requireDeployments(targets);
  const output = path.resolve(options.output);
  const existing = await lstat(output).catch((error) => { if (error.code === "ENOENT") return null; throw error; });
  if (!options.force && existing) throw new Error("output exists; pass --force to replace it");
  const minimum = options.minimum ?? 10;
  if (!Number.isSafeInteger(minimum) || minimum < 1) throw new Error("minimum must be a positive integer");
  const binary = path.resolve(options.binary ?? path.join(ROOT, "target/release/mcpeval"));
  const execute = options.execute ?? runCommand;
  const evaluator = await evaluatorIdentity(binary, execute);
  const work = await mkdtemp(path.join(os.tmpdir(), "mcpeval-corpus-"));
  const reports = `${output}.reports-${randomUUID()}`;
  const corpus = { schema: SCHEMA, source: "Pinned package targets with declared prerequisites; collected by scripts/corpus/collect.mjs", standard: targets.standard, platform: process.platform,
    evaluator, targets_sha256: sha256(bytes), population: [], observations: [] };
  try {
    await mkdir(reports, { recursive: true });
    for (const target of targets.targets) {
      if (await digestFile(binary) !== evaluator.sha256) throw new Error("evaluator changed during collection");
      const home = path.join(work, target.server); await mkdir(home);
      const result = await evaluateTarget(target, { binary, evaluator, standard: targets.standard, home, execute, environment: options.environment ?? process.env });
      corpus.population.push({ server: target.server, status: result.status, ...(result.reason ? { reason: result.reason } : {}) });
      if (result.raw) await writeFile(path.join(reports, `${target.server}.json`), result.raw, { flag: "wx" });
      if (result.observation) corpus.observations.push(result.observation);
    }
    if (await digestFile(binary) !== evaluator.sha256) throw new Error("evaluator changed during collection");
    corpus.observations.sort((a, b) => a.score - b.score || a.server.localeCompare(b.server, "en"));
    validateCorpus(corpus);
    await mkdir(path.dirname(output), { recursive: true });
    const temporary = `${output}.tmp-${randomUUID()}`;
    try {
      await writeFile(temporary, `${JSON.stringify(corpus, null, 2)}\n`, { flag: "wx" });
      if (options.force) await rename(temporary, output);
      else await link(temporary, output); // Atomic no-clobber even if another writer raced collection.
    }
    finally { await rm(temporary, { force: true }); }
    return { corpus, reports, sufficient: corpus.observations.length >= minimum };
  } finally { await rm(work, { recursive: true, force: true }); }
}

if (process.argv[1] && path.resolve(process.argv[1]) === import.meta.filename) {
  try {
    const { values } = parseArgs({ options: { targets: { type: "string" }, out: { type: "string" }, binary: { type: "string" }, "min-observations": { type: "string", default: "10" }, force: { type: "boolean", default: false }, "require-deployments": { type: "boolean", default: false } } });
    const result = await collectCorpus({ targetsPath: values.targets, output: values.out, binary: values.binary, minimum: Number(values["min-observations"]), force: values.force, requireDeployments: values["require-deployments"] });
    console.log(`collected ${result.corpus.observations.length} observed of ${result.corpus.population.length} targets; reports: ${result.reports}`);
    if (!result.sufficient) { console.error("insufficient observed targets; retained candidate is not a calibration baseline"); process.exitCode = 1; }
  } catch (error) { console.error(error.message); process.exitCode = 1; }
}
