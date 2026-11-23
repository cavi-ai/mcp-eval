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
  assert.match(job, /dry_run: false/u);
  assert.match(job, /secrets: inherit/u);

  const publish = await read(".github/workflows/publish-distribution.yml");
  assert.match(publish, /workflow_call:[\s\S]*dry_run:\n\s+default: false\n\s+type: boolean/u);
  assert.match(publish, /workflow_dispatch:[\s\S]*dry_run:[\s\S]*default: true\n\s+required: true\n\s+type: boolean/u, "a manual run defaults to a dry run");
  assert.match(publish, /node scripts\/distribution\/verify\.mjs --root "\$RELEASE_ROOT" --online --strict/u);
  assert.match(publish, /brew install [^\n]*mcpeval/u);
  assert.match(publish, /brew test [^\n]*mcpeval/u);
  assert.match(publish, /if: \$\{\{ !inputs\.dry_run \}\}\n[\s\S]*node scripts\/distribution\/publish-pins\.mjs --root "\$RELEASE_ROOT" --tag "\$TAG"/u);
});

test("a manual binary backfill resolves its tag once for building and publishing", async () => {
  const release = await read(".github/workflows/release-binaries.yml");
  const [, source, build, publish] = /\n  source:\n([\s\S]+?)\n  build:\n([\s\S]+?)\n  release:\n([\s\S]+?)\n  docs:/u.exec(release) ?? [];
  assert.ok(source && build && publish, "source, build and release jobs must be present");
  assert.match(source, /ref: \$\{\{ inputs\.tag && format\('refs\/tags\/\{0\}', inputs\.tag\) \|\| github\.sha \}\}/u);
  assert.match(source, /git rev-parse HEAD/u);
  assert.match(build, /needs: source/u);
  assert.match(build, /ref: \$\{\{ needs\.source\.outputs\.commit \}\}/u);
  assert.match(publish, /needs: \[source, build\]/u);
  assert.match(publish, /ref: \$\{\{ needs\.source\.outputs\.commit \}\}/u);
  const docs = /\n  docs:\n([\s\S]+?)\n  distribution:/u.exec(release)?.[1] ?? "";
  assert.match(docs, /needs: \[source, release\]/u);
  assert.match(docs, /source_ref: \$\{\{ needs\.source\.outputs\.commit \}\}/u);

  const docsWorkflow = await read(".github/workflows/publish-docs.yml");
  assert.match(docsWorkflow, /ref: \$\{\{ inputs\.source_ref \|\| github\.sha \}\}/u);
});

test("distribution uses current tooling against a separate tag checkout", async () => {
  const publish = await read(".github/workflows/publish-distribution.yml");
  const checkouts = [...publish.matchAll(/uses: actions\/checkout@\S+\n        with:\n((?: {10}[^\n]+\n)+)/gu)];
  assert.equal(checkouts.length, 2);
  assert.match(checkouts[0][1], /ref: \$\{\{ github\.sha \}\}/u);
  assert.match(checkouts[1][1], /ref: refs\/tags\/\$\{\{ inputs\.tag \}\}/u);
  assert.match(checkouts[1][1], /path: \$\{\{ env\.RELEASE_ROOT \}\}/u);
  assert.match(publish, /node scripts\/distribution\/publish-pins\.mjs --root "\$RELEASE_ROOT" --tag "\$TAG" --dry-run/u);
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
