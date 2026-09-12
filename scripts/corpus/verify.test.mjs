import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import path from "node:path";
import test from "node:test";

import {
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