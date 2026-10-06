import assert from "node:assert/strict";
import { mkdir, readFile, writeFile, rm } from "node:fs/promises";
import path from "node:path";
import test from "node:test";
import { spawnSync } from "node:child_process";
import { ROOT } from "./contract.mjs";
import { collectCorpus } from "./collect.mjs";
import { verifyCorpus } from "./verify.mjs";
import { fixture, target, report } from "./fixtures/evidence-fixture.mjs";

test("evidence action gates passing, failed, and invalid bundles with safe outputs and summaries", async (t) => {
  let catalog = 50;
  const f = await fixture(t, [target("PRIVATE_SERVER_LABEL")], (server) => {
    const value = report(server); value.private_note = "PRIVATE_REPORT_PAYLOAD";
    value.readiness.areas.find((area) => area.name === "catalog").score = catalog;
    return value;
  });
  const collection = await collectCorpus(f.options);
  const options = { ...f.options, corpusPath: f.options.output, reportsPath: collection.reports };
  const cases = [];
  for (const [label, score, code] of [["passing", 50, 0], ["failed", 51, 1], ["invalid", 50, 2]]) {
    catalog = score;
    const evidencePath = path.join(f.dir, `PRIVATE_BUNDLE ${label}`);
    await verifyCorpus({ ...options, evidencePath });
    if (code === 2) await writeFile(path.join(evidencePath, "verification.json"), "PRIVATE_INVALID_JSON\n::error::PRIVATE_INJECTED_COMMAND");
    cases.push({ evidencePath, code, label });
  }
  await rm(f.options.binary);
  for (const { evidencePath, code, label } of cases) {
    const output = path.join(f.dir, `${label}-output`); const summary = path.join(f.dir, `${label}-summary`);
    await writeFile(output, "previous=value\n"); await writeFile(summary, "Previous summary\n");
    const run = spawnSync(process.execPath, [path.join(ROOT, "actions/corpus-evidence/index.mjs")], { cwd: f.dir, encoding: "utf8", timeout: 10_000, env: {
      ...process.env, INPUT_EVIDENCE: evidencePath, INPUT_CORPUS: options.corpusPath, INPUT_TARGETS: options.targetsPath, INPUT_REPORTS: options.reportsPath,
      GITHUB_OUTPUT: output, GITHUB_STEP_SUMMARY: summary,
    } });
    assert.equal(run.status, code, run.stderr);
    const outputs = await readFile(output, "utf8"); const markdown = await readFile(summary, "utf8");
    assert.equal(outputs, `previous=value\nevidence-valid=${code !== 2}\npassed=${code === 0}\nexit-code=${code}\nobservations=${code === 2 ? "" : "1"}\npopulation=${code === 2 ? "" : "1"}\n`);
    assert.ok(markdown.startsWith("Previous summary\n"));
    assert.match(markdown, code === 2 ? /\| Integrity \| invalid \|/u : /\| Integrity \| valid \|/u);
    assert.match(markdown, new RegExp(`\\| Replay verdict \\| ${code === 0 ? "passed" : code === 1 ? "failed" : "unavailable"} \\|`, "u"));
    const emitted = [run.stdout, run.stderr, outputs, markdown].join("\n");
    assert.ok(!emitted.includes("PRIVATE_")); assert.ok(!emitted.includes(f.dir));
  }
});

test("evidence action publication failures do not expose private errors or passing outputs", async (t) => {
  const f = await fixture(t, [target("one")]);
  const collection = await collectCorpus(f.options);
  const options = { ...f.options, corpusPath: f.options.output, reportsPath: collection.reports, evidencePath: path.join(f.dir, "evidence") };
  await verifyCorpus(options);
  const output = path.join(f.dir, "output"); const summary = path.join(f.dir, "PRIVATE_SUMMARY_DIRECTORY");
  await writeFile(output, "previous=value\n"); await mkdir(summary);
  const env = { ...process.env, INPUT_EVIDENCE: options.evidencePath, INPUT_CORPUS: options.corpusPath, INPUT_TARGETS: options.targetsPath, INPUT_REPORTS: options.reportsPath, GITHUB_OUTPUT: output, GITHUB_STEP_SUMMARY: summary };
  const action = path.join(ROOT, "actions/corpus-evidence/index.mjs");
  const failed = spawnSync(process.execPath, [action], { encoding: "utf8", timeout: 10_000, env });
  assert.equal(failed.status, 2);
  assert.equal(await readFile(output, "utf8"), "previous=value\n");
  assert.ok(!failed.stderr.includes("PRIVATE_")); assert.ok(!failed.stderr.includes(f.dir));
  const missing = { ...env }; delete missing.GITHUB_OUTPUT;
  assert.equal(spawnSync(process.execPath, [action], { encoding: "utf8", timeout: 10_000, env: missing }).status, 2);
});
