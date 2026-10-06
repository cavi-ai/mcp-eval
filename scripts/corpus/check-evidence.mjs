#!/usr/bin/env node
import { lstat, readFile, readdir } from "node:fs/promises";
import path from "node:path";
import { isDeepStrictEqual, parseArgs } from "node:util";
import { observationFrom, sha256 } from "./contract.mjs";
import { parseDocument, readCollection, replayResult, verificationFor } from "./replay-contract.mjs";

async function directory(location) {
  if (!(await lstat(location)).isDirectory()) throw new Error("evidence requires directories, not links or special files");
}
async function regularFile(location) {
  if (!(await lstat(location)).isFile()) throw new Error("evidence requires regular files, not links or special files");
  return readFile(location);
}
function same(actual, expected, message) {
  if (!isDeepStrictEqual(actual, expected)) throw new Error(message);
}
const resultKeys = ["server", "status", "expected", "observed", "moved", "drifted", "expected_report_sha256", "replay_report_sha256"];
function resultFields(row) {
  if (!row || typeof row !== "object" || Array.isArray(row)) throw new Error("invalid evidence result");
  return Object.fromEntries([...resultKeys, ...("reason" in row ? ["reason"] : [])].map((key) => [key, row[key]]));
}
// These outcomes discard or never produce report bytes in evaluateTarget.
const withoutReport = {
  "missing-environment": ["untested"], "prerequisite-failed": ["untested"],
  "deployment-mismatch": ["errored"], "state-check-failed": ["untested", "errored"],
  "state-mismatch": ["errored"], "evaluation-failed": ["errored"], "invalid-report": ["errored"],
};

/** Check stored byte identities and re-derive the verdict; never execute a target or evaluator. */
export async function checkEvidence(options = {}) {
  if (!options.evidencePath || !options.corpusPath || !options.targetsPath || !options.reportsPath) throw new Error("checking requires --evidence, --corpus, --targets, and --reports");
  const root = path.resolve(options.evidencePath);
  await directory(root);
  same((await readdir(root)).sort(), ["reports", "verification.json"], "incomplete evidence or unexpected bundle entries");
  const reports = path.join(root, "reports");
  await directory(reports);
  const saved = parseDocument(await regularFile(path.join(root, "verification.json")), "invalid evidence JSON");
  const collection = await readCollection(options, regularFile);
  const { corpus, targets } = collection;
  if (!corpus.platform || !Array.isArray(saved?.results) || saved.results.length !== corpus.population.length) throw new Error("invalid evidence population or platform");
  const results = []; const expectedFiles = [];
  for (let index = 0; index < corpus.population.length; index++) {
    const entry = corpus.population[index];
    const row = resultFields(saved.results[index]);
    let replay;
    if (entry.status === "observed") {
      if (row.replay_report_sha256 === null) {
        if (typeof row.reason !== "string" || !Object.hasOwn(withoutReport, row.reason) || !withoutReport[row.reason].includes(row.status)) throw new Error("unsupported evidence outcome without report");
        replay = { status: row.status, reason: row.reason };
      } else {
        const name = `${entry.server}.json`; expectedFiles.push(name);
        const raw = await regularFile(path.join(reports, name));
        same(sha256(raw), row.replay_report_sha256, "replay report digest mismatch");
        const target = targets.targets.find((item) => item.server === entry.server);
        replay = { ...observationFrom(raw, target, corpus.evaluator, corpus.standard), raw };
      }
    }
    const expected = corpus.observations.find((item) => item.server === entry.server);
    const result = replayResult(entry, expected, replay);
    same(row, result, "evidence result does not match retained reports or original population");
    results.push(result);
  }
  same((await readdir(reports)).sort(), expectedFiles.sort(), "missing or unexpected replay reports");
  const verified = verificationFor(collection, results);
  // v1 permits extensions; check all contract fields without echoing arbitrary metadata.
  const fields = Object.fromEntries(Object.keys(verified).map((key) => [key, saved[key]]));
  fields.evaluator = { version: saved.evaluator?.version, sha256: saved.evaluator?.sha256 };
  fields.policy = { reliability_tolerance: saved.policy?.reliability_tolerance };
  fields.results = results; // Each result's contract fields were checked above.
  same(fields, verified, "evidence identity, policy, counts, or verdict mismatch");
  return verified;
}

if (process.argv[1] && path.resolve(process.argv[1]) === import.meta.filename) {
  try {
    const { values } = parseArgs({ options: { evidence: { type: "string" }, corpus: { type: "string" }, targets: { type: "string" }, reports: { type: "string" }, json: { type: "boolean", default: false } } });
    const result = await checkEvidence({ evidencePath: values.evidence, corpusPath: values.corpus, targetsPath: values.targets, reportsPath: values.reports });
    console.log(values.json ? JSON.stringify(result, null, 2) : `evidence intact; replay ${result.passed ? "passed" : "failed"}`);
    process.exitCode = result.passed ? 0 : 1;
  } catch (error) { console.error(error.message); process.exitCode = 2; }
}
