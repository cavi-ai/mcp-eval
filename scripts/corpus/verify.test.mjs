import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { mkdir, mkdtemp, readFile, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import test from "node:test";

import {
  ISOLATION,
  NPM_PACKAGES,
  NPM_SERVERS,
  UVX_PACKAGES,
  UVX_SERVERS,
  RELIABILITY_TOLERANCE,
  commandFor,
  driftOf,
  readinessScore,
  scoreArguments,
} from "./verify.mjs";

const ROOT = path.resolve(import.meta.dirname, "../..");

test("drift check launches exactly what collect.sh launches", async () => {
  const collect = await readFile(path.join(ROOT, "scripts/corpus/collect.sh"), "utf8");
  const sectionBody = (name) => {
    // Anchor on the array declaration line, not the for-loop usage that
    // references the same name later in the script. Bash arrays are
    // parenthesized: `NAME=( entries... )`.
    const declaration = collect.indexOf(`${name}=(`);
    assert.notEqual(declaration, -1, `collector is missing ${name}`);
    const open = collect.indexOf("(", declaration);
    const close = collect.indexOf("\n)", open);
    assert.notEqual(close, -1, `collector array ${name} is unterminated`);
    return collect.slice(open + 1, close);
  };
  const labelsOf = (name) => {
    const body = sectionBody(name);
    return body
      .split("\n")
      .map((line) => line.trim().replace(/,\s*$/, ""))
      .filter((line) => line.startsWith('"'))
      .map((line) => line.split("|")[0].slice(1));
  };
  const npmLabels = labelsOf("NPM_SERVERS");
  const uvxLabels = labelsOf("UVX_SERVERS");
  assert.ok(npmLabels.length >= 20, `unexpectedly few npm labels: ${npmLabels.length}`);
  assert.ok(uvxLabels.length >= 5, `unexpectedly few uvx labels: ${uvxLabels.length}`);
  assert.deepEqual([...NPM_SERVERS.keys()].sort(), npmLabels.sort());
  assert.deepEqual([...UVX_SERVERS.keys()].sort(), uvxLabels.sort());

  // Every label resolves to a launch command.
  for (const label of [...npmLabels, ...uvxLabels]) {
    const command = commandFor(label);
    assert.ok(command, `no launch command for ${label}`);
    assert.ok(command.length >= 2, `launch command too short for ${label}: ${command}`);
  }

  // Package names agree: the drift check must launch the same bytes as
  // the collector for every label.
  for (const label of npmLabels) {
    const expected = packageFromEntry(collect, "NPM_SERVERS", label);
    assert.equal(NPM_PACKAGES.get(label), expected, `npm package mismatch for ${label}`);
  }
  for (const label of uvxLabels) {
    const expected = packageFromEntry(collect, "UVX_SERVERS", label);
    assert.equal(UVX_PACKAGES.get(label), expected, `uvx package mismatch for ${label}`);
  }
});

function packageFromEntry(collect, name, label) {
  const declaration = collect.indexOf(`${name}=(`);
  const open = collect.indexOf("(", declaration);
  const close = collect.indexOf("\n)", open);
  const body = collect.slice(open + 1, close);
  const entry = body
    .split("\n")
    .map((line) => line.trim().replace(/,\s*$/, ""))
    .find((line) => line.startsWith(`"${label}|`));
  assert.ok(entry, `collector entry for ${label} not found`);
  return entry.split("|")[1];
}

test("the corpus score is the standard's readiness", async () => {
  assert.equal(readinessScore({ gate: null, readiness: { score: 71, standard: "s" } }), 71);
  assert.equal(readinessScore({ readiness: { score: 0, standard: "s" } }), 0);
  assert.equal(readinessScore({ readiness: null, readiness_error: "transport-closed" }), null);
  assert.equal(readinessScore({}), null);

  // Both scripts run the standard battery with no manifest.
  assert.deepEqual(scoreArguments("demo"), ["score", "--server", "demo", "--format", "json"]);
  const collect = await readFile(path.join(ROOT, "scripts/corpus/collect.sh"), "utf8");
  const scoreLine = collect.split("\n").find((line) => line.includes('"$BIN" score'));
  assert.ok(scoreLine?.includes("--format json"), `collector score line: ${scoreLine}`);
  assert.ok(!collect.includes("--manifest"), "collector still runs a manifest");
});

test("drift: every area but reliability must match; reliability may move one latency band", () => {
  const areas = { protocol: 100, catalog: 64, context: 32, "error-honesty": 0, reliability: 91, coverage: 100 };
  const expected = { server: "notion", score: 66, areas };
  const observed = (changes) => ({ score: 66, areas: { ...areas, ...changes } });
  assert.deepEqual(driftOf(expected, observed({})), []);
  // Latency depends on the machine and network: one band on every tool.
  assert.deepEqual(driftOf(expected, { ...observed({ reliability: 100 }), score: 67 }), []);
  assert.deepEqual(driftOf(expected, observed({ reliability: 91 - RELIABILITY_TOLERANCE })), []);
  assert.deepEqual(driftOf(expected, observed({ reliability: 91 - RELIABILITY_TOLERANCE - 1 })), [
    `reliability 91→${90 - RELIABILITY_TOLERANCE}`,
  ]);
  // Every other area is a function of the server's code: exact.
  assert.deepEqual(driftOf(expected, observed({ catalog: 65 })), ["catalog 64→65"]);
  assert.deepEqual(driftOf(expected, observed({ coverage: undefined })), ["coverage 100→none"]);
  // An observation without areas compares the score.
  assert.deepEqual(driftOf({ server: "a", score: 40 }, { score: 41, areas: {} }), ["score 40→41"]);
  assert.equal(RELIABILITY_TOLERANCE, 10);
});

test("both scripts keep servers away from this machine's cluster and containers", async () => {
  const collect = await readFile(path.join(ROOT, "scripts/corpus/collect.sh"), "utf8");
  assert.ok(Object.keys(ISOLATION).length >= 2);
  for (const [name, value] of Object.entries(ISOLATION)) {
    const exported = collect.match(new RegExp(`^export ${name}=(.+)$`, "mu"))?.[1];
    assert.ok(exported, `collector does not export ${name}`);
    assert.equal(exported.replaceAll('"', "").replace("$ROOT", ROOT), value, name);
  }
  // A valid kubeconfig with no cluster: the Kubernetes server starts and
  // reaches nothing.
  const kubeconfig = await readFile(ISOLATION.KUBECONFIG, "utf8");
  assert.match(kubeconfig, /^clusters: \[\]$/mu);
  assert.match(kubeconfig, /^contexts: \[\]$/mu);
});

test("every corpus observation has a launch command", async () => {
  const corpus = JSON.parse(
    await readFile(path.join(ROOT, "data/readiness-corpus.json"), "utf8"),
  );
  for (const observation of corpus.observations) {
    assert.ok(
      commandFor(observation.server),
      `corpus observation ${observation.server} has no launch command`,
    );
  }
});
function heredoc(collect, opener, terminator) {
  const start = collect.indexOf(opener);
  assert.notEqual(start, -1, `collector is missing ${opener}`);
  const body = collect.indexOf("\n", start) + 1;
  const end = collect.indexOf(`\n${terminator}`, body);
  const after = collect[end + terminator.length + 1];
  assert.ok(end !== -1 && (after === undefined || after === "\n"), `collector heredoc ${opener} is unterminated`);
  return collect.slice(body, end);
}

test("collector records readiness, areas, and catalog measurements", async () => {
  const collect = await readFile(path.join(ROOT, "scripts/corpus/collect.sh"), "utf8");
  const writer = heredoc(collect, `python3 - "$REPORTS" "$OUT" <<'PYEOF'`, "PYEOF");

  const work = await mkdtemp(path.join(os.tmpdir(), "mcpeval-corpus-collect-"));
  const reports = path.join(work, "reports");
  await mkdir(reports);
  const STANDARD = "mcpeval-standard/1";
  const report = (score, tools, tokens) =>
    JSON.stringify({
      schema: "mcpeval.probe-report/v2",
      gate: null,
      readiness: {
        standard: STANDARD,
        score,
        surface: { tools, read_only: tools, writers: 0, exercised: 0 },
        areas: [
          { name: "protocol", weight: 15, score: 100, measurements: {}, checks: [] },
          { name: "context", weight: 15, score: 90, measurements: { catalog_tokens: tokens }, checks: [] },
          { name: "coverage", weight: 15, score: score, measurements: {}, checks: [] },
        ],
      },
      cases: [],
    });
  for (let index = 0; index < 10; index += 1) {
    await writeFile(path.join(reports, `server-${index}.json`), report(90 - index, index + 1, 100 * (index + 1)));
  }
  await writeFile(path.join(reports, "could-not-run.json"), "");
  await writeFile(
    path.join(reports, "unmeasured.json"),
    JSON.stringify({ schema: "mcpeval.probe-report/v2", gate: null, readiness: null, readiness_error: "transport-closed", cases: [] }),
  );
  await writeFile(path.join(reports, "empty.json"), report(20, 0, 0));
  const out = path.join(work, "corpus.json");
  execFileSync("python3", ["-", reports, out], { input: writer, stdio: ["pipe", "ignore", "inherit"] });

  const document = JSON.parse(await readFile(out, "utf8"));
  assert.equal(document.schema, "mcpeval.readiness-corpus/v2");
  assert.equal(document.standard, STANDARD);
  assert.equal(document.battery, undefined);
  assert.deepEqual(
    document.observations.find((observation) => observation.server === "server-2"),
    {
      server: "server-2",
      score: 88,
      areas: { protocol: 100, context: 90, coverage: 88 },
      tool_count: 3,
      catalog_tokens: 300,
    },
  );
  assert.equal(document.observations.length, 10);
  for (const skipped of ["could-not-run", "unmeasured", "empty"]) {
    assert.ok(!document.observations.some((observation) => observation.server === skipped), skipped);
  }
});
