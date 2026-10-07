import { readFile } from "node:fs/promises";
import path from "node:path";
import { isDeepStrictEqual } from "node:util";
import { ROOT, validateCorpus, validateTargets, sha256, observationFrom } from "./contract.mjs";

export const RELIABILITY_TOLERANCE = 10;
export const VERIFICATION_SCHEMA = "mcpeval.corpus-verification/v1";
export function driftOf(expected, observed) {
  return Object.entries(expected.areas).flatMap(([name, before]) => {
    const after = observed.areas[name];
    return !Number.isInteger(after) || Math.abs(after - before) > (name === "reliability" ? RELIABILITY_TOLERANCE : 0) ? [`${name} ${before}→${after ?? "none"}`] : [];
  });
}
export function driftedResults(results) { return results.filter((result) => result.status !== "observed" || result.drifted); }

export function parseDocument(raw, message) {
  try { return JSON.parse(raw); } catch { throw new Error(message); }
}

/** Read each input once and check its byte identity and report projection without execution. */
export async function readCollection(options, read = readFile) {
  const corpusBytes = await read(options.corpusPath ?? path.join(ROOT, "data/readiness-corpus.json"));
  const corpus = validateCorpus(parseDocument(corpusBytes, "invalid corpus JSON"));
  if (!options.targetsPath || !options.reportsPath) throw new Error("verification requires --targets and --reports from collection");
  const targetBytes = await read(options.targetsPath);
  const targets = validateTargets(parseDocument(targetBytes, "invalid targets JSON"));
  if (sha256(targetBytes) !== corpus.targets_sha256 || targets.standard !== corpus.standard || !isDeepStrictEqual(targets.targets.map((target) => target.server), corpus.population.map((entry) => entry.server))) throw new Error("targets provenance mismatch");
  for (const expected of corpus.observations) {
    const raw = await read(path.join(options.reportsPath, `${expected.server}.json`));
    if (sha256(raw) !== expected.provenance.report_sha256) throw new Error("original report digest mismatch");
    const target = targets.targets.find((item) => item.server === expected.server);
    const result = observationFrom(raw, target, corpus.evaluator, corpus.standard);
    // Existing v3 artifacts did not retain this optional field. Preserve their
    // projection without inventing a historical measurement profile.
    if (expected.measurement_profile === undefined && result.observation) delete result.observation.measurement_profile;
    if (result.status !== "observed" || !isDeepStrictEqual(result.observation, expected)) throw new Error("original report projection mismatch");
  }
  return { corpus, corpusBytes, targets, targetBytes };
}

export function replayResult(entry, expected, replay) {
  if (entry.status !== "observed") return { server: entry.server, status: entry.status, reason: entry.reason, expected: null, observed: null, moved: [], drifted: false, expected_report_sha256: null, replay_report_sha256: null };
  const moved = replay.observation ? driftOf(expected, replay.observation) : [];
  if (expected.measurement_profile !== undefined && replay.observation
    && !isDeepStrictEqual(expected.measurement_profile, replay.observation.measurement_profile)) moved.push("measurement-profile");
  return { server: entry.server, status: replay.status, ...(replay.reason ? { reason: replay.reason } : {}), expected: expected.score, observed: replay.observation?.score ?? null, moved, drifted: moved.length > 0,
    expected_report_sha256: expected.provenance.report_sha256, replay_report_sha256: replay.raw === undefined ? null : sha256(replay.raw) };
}

export function verificationFor({ corpus, corpusBytes, targetBytes }, results) {
  return { schema: VERIFICATION_SCHEMA, standard: corpus.standard, platform: corpus.platform,
    corpus_sha256: sha256(corpusBytes), targets_sha256: sha256(targetBytes), evaluator: { version: corpus.evaluator.version, sha256: corpus.evaluator.sha256 },
    policy: { reliability_tolerance: RELIABILITY_TOLERANCE }, passed: driftedResults(results).length === 0,
    observations: corpus.observations.length, population: corpus.population.length, results };
}
