import { appendFile } from "node:fs/promises";
import { checkEvidence } from "../../scripts/corpus/check-evidence.mjs";

async function run() {
  const env = process.env;
  if (!env.GITHUB_OUTPUT || !env.GITHUB_STEP_SUMMARY) throw new Error("runner file commands unavailable");
  let verified;
  try {
    verified = await checkEvidence({ evidencePath: env.INPUT_EVIDENCE, corpusPath: env.INPUT_CORPUS, targetsPath: env.INPUT_TARGETS, reportsPath: env.INPUT_REPORTS });
  } catch {
    // Do not send input paths, raw JSON, labels, or adapter diagnostics to runner logs.
  }
  const valid = verified !== undefined;
  const passed = verified?.passed === true;
  const code = valid ? (passed ? 0 : 1) : 2;
  const verdict = valid ? (passed ? "passed" : "failed") : "unavailable";
  const summary = `\n## Corpus replay evidence\n\n| Check | Result |\n| --- | --- |\n| Integrity | ${valid ? "valid" : "invalid"} |\n| Replay verdict | ${verdict} |\n| Original observations | ${verified?.observations ?? "unavailable"} |\n| Population | ${verified?.population ?? "unavailable"} |\n`;
  // Publish summary first so a summary failure cannot leave passing step outputs.
  await appendFile(env.GITHUB_STEP_SUMMARY, summary);
  await appendFile(env.GITHUB_OUTPUT, `evidence-valid=${valid}\npassed=${passed}\nexit-code=${code}\nobservations=${verified?.observations ?? ""}\npopulation=${verified?.population ?? ""}\n`);
  if (code === 1) console.error("::error::Corpus replay verdict failed.");
  else if (code === 2) console.error("::error::Corpus evidence is invalid or incomplete; inspect inputs privately.");
  else console.log("Corpus evidence intact; replay passed.");
  return code;
}

try { process.exitCode = await run(); }
catch { console.error("::error::Corpus evidence action could not publish results."); process.exitCode = 2; }
