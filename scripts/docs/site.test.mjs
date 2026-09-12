import assert from "node:assert/strict";
import { cp, mkdtemp, readFile, readdir, rm, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import test from "node:test";

import { slug } from "./render-site.mjs";

const ROOT = path.resolve(import.meta.dirname, "../..");
const DOCS = path.join(ROOT, "docs/mcp-eval");

async function renderInto(temporary, versions) {
  const docsRoot = path.join(temporary, "docs/mcp-eval");
  for (const version of versions) {
    await cp(path.join(DOCS, `v${version}`), path.join(docsRoot, `v${version}`), {
      recursive: true,
    });
  }
  const output = path.join(temporary, "site");
  const { renderSite } = await import(`./render-site.mjs?case=${versions.join("-")}`);
  return { site: await renderSite({ docsRoot, output }), output };
}

async function readSite(output, relative) {
  return readFile(path.join(output, relative), "utf8");
}

test("site renders every navigated page as html with navigation", async (context) => {
  const temporary = await mkdtemp(path.join(os.tmpdir(), "mcpeval-site-"));
  context.after(() => rm(temporary, { recursive: true, force: true }));
  const { site, output } = await renderInto(temporary, ["0.1.0", "0.2.0"]);
  assert.equal(site.versions.length, 2);
  assert.equal(site.currentVersion, "0.2.0");

  const navigation = JSON.parse(await readFile(path.join(DOCS, "v0.2.0/navigation.json"), "utf8"));
  const expectedPages = navigation.sections
    .flatMap((section) => section.pages)
    .map((page) => `docs/mcp-eval/v0.2.0/${page.path.replace(/\.md$/u, ".html")}`);
  for (const relative of expectedPages) {
    assert.ok(site.written.includes(relative), `missing site page ${relative}`);
    const html = await readSite(output, relative);
    assert.ok(html.startsWith("<!doctype html>"), `${relative} is not html`);
    assert.ok(html.includes("<nav "), `${relative} has no navigation element`);
    for (const section of navigation.sections) {
      assert.ok(html.includes(section.title), `${relative}: section ${section.title} missing`);
    }
  }
});

test("site escapes html in prose and code spans", async (context) => {
  const temporary = await mkdtemp(path.join(os.tmpdir(), "mcpeval-site-"));
  context.after(() => rm(temporary, { recursive: true, force: true }));
  const { output } = await renderInto(temporary, ["0.2.0"]);
  const cli = await readSite(output, "docs/mcp-eval/v0.2.0/reference/cli.html");
  // The CLI reference contains `--server <SERVER> -- <CMD>...` inside a
  // code span; the angle brackets must be entity-escaped.
  assert.ok(cli.includes("&lt;SERVER&gt;"), "code-span angle brackets unescaped");
  assert.ok(!cli.includes("<SERVER>"), "raw angle brackets leaked into html");
});

test("site keeps markdown tables as html tables", async (context) => {
  const temporary = await mkdtemp(path.join(os.tmpdir(), "mcpeval-site-"));
  context.after(() => rm(temporary, { recursive: true, force: true }));
  const { output } = await renderInto(temporary, ["0.2.0"]);
  const readiness = await readSite(output, "docs/mcp-eval/v0.2.0/guides/readiness-reporting.html");
  assert.ok(readiness.includes("<table>"), "table missing");
  assert.ok(readiness.includes("<th>Category</th>"), "table header missing");
});

test("latest version redirect and stable alias point at the current overview", async (context) => {
  const temporary = await mkdtemp(path.join(os.tmpdir(), "mcpeval-site-"));
  context.after(() => rm(temporary, { recursive: true, force: true }));
  const { output } = await renderInto(temporary, ["0.1.0", "0.2.0"]);
  const alias = await readSite(output, "docs/index.html");
  assert.ok(
    alias.includes("/docs/mcp-eval/v0.2.0/introduction/overview.html"),
    `stable alias points at the wrong version: ${alias}`,
  );
  const root = await readSite(output, "index.html");
  assert.ok(root.includes("/docs/mcp-eval/"), "root index must redirect into the docs alias");
  const perVersion = await readSite(output, "docs/mcp-eval/v0.1.0/index.html");
  assert.ok(
    perVersion.includes("/docs/mcp-eval/v0.1.0/introduction/overview.html"),
    "per-version index must open that version's first page",
  );
});

test("manifest version mismatch is rejected", async (context) => {
  const temporary = await mkdtemp(path.join(os.tmpdir(), "mcpeval-site-"));
  context.after(() => rm(temporary, { recursive: true, force: true }));
  const docsRoot = path.join(temporary, "docs/mcp-eval");
  await cp(path.join(DOCS, "v0.2.0"), path.join(docsRoot, "v0.2.0"), { recursive: true });
  const manifestPath = path.join(docsRoot, "v0.2.0/manifest.json");
  const manifest = JSON.parse(await readFile(manifestPath, "utf8"));
  manifest.version = "9.9.9";
  await writeFile(manifestPath, JSON.stringify(manifest, null, 2));
  const { renderSite } = await import("./render-site.mjs");
  await assert.rejects(
    () => renderSite({ docsRoot, output: path.join(temporary, "site") }),
    /does not match directory/,
  );
});

test("slug produces stable heading anchors", () => {
  assert.equal(slug("State of MCP servers"), "state-of-mcp-servers");
  assert.equal(slug("What's recorded?"), "what-s-recorded");
});