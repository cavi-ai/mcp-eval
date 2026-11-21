import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

import { describedVersion, planUpdate, releaseTargets } from "./publish-pins.mjs";

const ROOT = new URL("../../", import.meta.url).pathname;
const read = (file) => readFile(new URL(file, new URL("../../", import.meta.url)), "utf8");
const manifest = await read("distribution/release.json");
const formula = await read("Formula/mcpeval.rb");
const version = JSON.parse(manifest).version;

function bumped(text, to) {
  return text.replaceAll(`/download/v${version}/`, `/download/v${to}/`).replaceAll(`"version": "${version}"`, `"version": "${to}"`);
}

test("a tag publishes the distribution and the tap through publish-distribution", async () => {
  const release = await read(".github/workflows/release-binaries.yml");
  const job = /\n  distribution:\n([\s\S]*?)(?=\n  [a-z-]+:\n|$)/u.exec(release)?.[1] ?? "";
  assert.match(job, /needs: release/u);
  assert.match(job, /uses: \.\/\.github\/workflows\/publish-distribution\.yml/u);
  assert.match(job, /dry_run: "false"/u);
  assert.match(job, /secrets: inherit/u);

  const publish = await read(".github/workflows/publish-distribution.yml");
  assert.match(publish, /workflow_dispatch:[\s\S]*dry_run:[\s\S]*default: "true"/u, "a manual run defaults to a dry run");
  assert.match(publish, /node scripts\/distribution\/verify\.mjs --online --strict/u);
  assert.match(publish, /brew install [^\n]*mcpeval/u);
  assert.match(publish, /brew test [^\n]*mcpeval/u);
  assert.match(publish, /if: \$\{\{ inputs\.dry_run != 'true' \}\}\n[\s\S]*node scripts\/distribution\/publish-pins\.mjs --tag "\$TAG"/u);
});

test("the version a manifest or formula describes", () => {
  assert.equal(describedVersion("distribution/release.json", manifest), version);
  assert.equal(describedVersion("Formula/mcpeval.rb", formula), version);
  assert.throws(() => describedVersion("Formula/mcpeval.rb", formula.replace(`/download/v${version}/`, "/download/v0.0.1/")), /one release version/u);
});

test("a newer release updates the pins, an equal one is current", () => {
  const next = { "Formula/mcpeval.rb": bumped(formula, "99.0.0"), "distribution/release.json": bumped(manifest, "99.0.0") };
  const now = { "Formula/mcpeval.rb": formula, "distribution/release.json": manifest };
  assert.deepEqual(planUpdate(now, next), { action: "update", changed: ["Formula/mcpeval.rb", "distribution/release.json"] });
  assert.deepEqual(planUpdate(now, now), { action: "current", changed: [] });
  assert.deepEqual(planUpdate({ "Formula/mcpeval.rb": null }, { "Formula/mcpeval.rb": formula }), { action: "update", changed: ["Formula/mcpeval.rb"] });
});

test("pins never move backwards or rewrite a published version", () => {
  const ahead = { "Formula/mcpeval.rb": bumped(formula, "99.0.0") };
  assert.deepEqual(planUpdate(ahead, { "Formula/mcpeval.rb": formula }), { action: "newer", changed: ["Formula/mcpeval.rb"] });
  assert.throws(
    () => planUpdate({ "Formula/mcpeval.rb": formula }, { "Formula/mcpeval.rb": `${formula}# edited\n` }),
    /already published version/u,
  );
});

test("each repository receives the release's files under one version", async () => {
  const [source, tap] = await releaseTargets({ root: ROOT, version });
  assert.equal(source.repository, "cavi-ai/mcp-eval");
  assert.deepEqual(source.files, { "distribution/release.json": manifest, "Formula/mcpeval.rb": formula });
  assert.equal(source.title, `chore: update ${version} distribution`);
  assert.equal(tap.repository, "cavi-ai/homebrew-tap");
  assert.deepEqual(tap.files, { "Formula/mcpeval.rb": formula });
  assert.equal(tap.title, `chore: update mcpeval to ${version}`);
  await assert.rejects(releaseTargets({ root: ROOT, version: "99.0.0" }), /does not describe 99\.0\.0/u);
});
