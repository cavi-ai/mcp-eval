import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { mkdir, mkdtemp, readFile, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import test from "node:test";

import {
  MANIFEST,
  NPM_PACKAGES,
  NPM_SERVERS,
  UVX_PACKAGES,
  UVX_SERVERS,
  commandFor,
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

test("collector writes the drift check's battery and catalog measurements", async () => {
  const collect = await readFile(path.join(ROOT, "scripts/corpus/collect.sh"), "utf8");
  const manifest = heredoc(collect, `cat > "$MANIFEST" <<'EOF'`, "EOF");
  const writer = heredoc(collect, `python3 - "$REPORTS" "$MANIFEST" "$OUT" <<'PYEOF'`, "PYEOF");

  const work = await mkdtemp(path.join(os.tmpdir(), "mcpeval-corpus-collect-"));
  const reports = path.join(work, "reports");
  await mkdir(reports);
  const manifestPath = path.join(work, "corpus.manifest.json");
  await writeFile(manifestPath, manifest);
  const report = (score, cases) => JSON.stringify({ readiness: { score }, cases });
  for (let index = 0; index < 10; index += 1) {
    await writeFile(
      path.join(reports, `server-${index}.json`),
      report(100 - index, [
        { probe: "discovery-cost", measurements: { tool_count: index + 1, schema_bytes: 10 } },
        { probe: "token-cost", measurements: { tool_count: index + 1, total_tokens: 100 * (index + 1) } },
        { probe: "pagination", measurements: { pages: 1 } },
      ]),
    );
  }
  await writeFile(path.join(reports, "unmeasured.json"), report(50, [{ probe: "pagination", measurements: {} }]));
  await writeFile(path.join(reports, "could-not-run.json"), "");
  await writeFile(
    path.join(reports, "transport.json"),
    report(50, [{ probe: "pagination", reason: "transport-timeout", measurements: {} }]),
  );
  await writeFile(
    path.join(reports, "empty.json"),
    report(100, [{ probe: "discovery-cost", measurements: { tool_count: 0 } }]),
  );
  const out = path.join(work, "corpus.json");
  execFileSync("python3", ["-", reports, manifestPath, out], { input: writer, stdio: ["pipe", "ignore", "inherit"] });

  const document = JSON.parse(await readFile(out, "utf8"));
  assert.deepEqual(document.battery, MANIFEST.probes.map((probe) => probe.probe));
  assert.equal(document.schema, "mcpeval.readiness-corpus/v1");
  assert.deepEqual(
    document.observations.find((observation) => observation.server === "server-2"),
    { server: "server-2", score: 98, tool_count: 3, catalog_tokens: 300 },
  );
  assert.deepEqual(
    document.observations.find((observation) => observation.server === "unmeasured"),
    { server: "unmeasured", score: 50 },
  );
  assert.equal(document.observations.length, 11);
  assert.ok(!document.observations.some((observation) => observation.server === "could-not-run"));
  assert.ok(!document.observations.some((observation) => observation.server === "transport"));
  assert.ok(!document.observations.some((observation) => observation.server === "empty"));
});
