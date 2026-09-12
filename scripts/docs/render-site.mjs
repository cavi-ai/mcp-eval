// Render the built documentation tree into a static HTML site.
//
// Input: docs/mcp-eval/v<VERSION>/ (the deterministic build output: pages,
// navigation.json, manifest.json). Output: docs/site/ with one .html file
// per page, an index.html redirecting to the latest version's overview, a
// per-version index.html, and a 404 page. Zero runtime dependencies; the
// markdown subset is the one the docs source actually uses (headings,
// fenced code, pipe tables, bullet lists, inline code, bold/italic, and
// external links).
//
// Deterministic: identical input produces byte-identical output, so the
// site can be shipped beside the docs archive and verified the same way.
//
// Usage: node scripts/docs/render-site.mjs [--docs-root <dir>] [--output <dir>]
import { mkdir, readFile, rm, stat, writeFile } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";

import {
  PRODUCT_ID,
  REPO_ROOT,
  comparePortablePaths,
  listFilesRecursive,
  navigationPaths,
  parseOptions,
} from "./lib.mjs";

const DEFAULT_DOCS_ROOT = path.join(
  REPO_ROOT,
  `docs/${PRODUCT_ID}/v0.2.0`,
).replace(/v0\.2\.0$/u, "");

function escapeHtml(text) {
  return text
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;");
}

function escapeAttribute(text) {
  return escapeHtml(text).replaceAll("'", "&#39;");
}

/// Inline markdown: code spans, bold, italics, links. Applies HTML
/// escaping itself — every input is raw markdown text.
function renderInline(raw) {
  const codes = [];
  let text = raw.replace(/`([^`]+)`/g, (_, content) => {
    codes.push(escapeHtml(content));
    return `\u0000${codes.length - 1}\u0000`;
  });
  text = escapeHtml(text)
    .replace(/\*\*([^*]+)\*\*/g, "<strong>$1</strong>")
    .replace(/\*([^*]+)\*/g, "<em>$1</em>")
    .replace(
      /\[([^\]]+)\]\((https?:\/\/[^)\s]+)\)/gu,
      (_, label, url) => `<a href="${escapeAttribute(url)}">${label}</a>`,
    );
  return text.replace(/\u0000(\d+)\u0000/g, (_, index) => `<code>${codes[Number(index)]}</code>`);
}

/// The markdown subset the docs use. Returns HTML fragment lines.
function renderMarkdown(body) {
  const lines = body.split("\n");
  const out = [];
  let index = 0;
  let inFence = false;
  let fenceLanguage = "";
  const fenceBuffer = [];
  let listOpen = false;
  let tableBuffer = [];
  let paragraph = [];

  const flushParagraph = () => {
    if (paragraph.length) {
      out.push(`    <p>${renderInline(paragraph.join(" "))}</p>`);
      paragraph = [];
    }
  };
  const flushList = () => {
    if (listOpen) {
      out.push("    </ul>");
      listOpen = false;
    }
  };
  const flushTable = () => {
    if (!tableBuffer.length) return;
    const rows = tableBuffer.map((row) => row.replace(/^\||\|\s*$/g, "").split("|").map((cell) => cell.trim()));
    const [header, ...rest] = rows;
    const body = rest.filter((row) => !row.every((cell) => /^:?-{3,}:?$/.test(cell)));
    out.push("    <table>");
    out.push("      <thead>");
    out.push(`        <tr>${header.map((cell) => `<th>${renderInline(cell)}</th>`).join("")}</tr>`);
    out.push("      </thead>");
    out.push("      <tbody>");
    for (const row of body) {
      out.push(`        <tr>${row.map((cell) => `<td>${renderInline(cell)}</td>`).join("")}</tr>`);
    }
    out.push("      </tbody>");
    out.push("    </table>");
    tableBuffer = [];
  };
  const closeAll = () => {
    flushParagraph();
    flushList();
    flushTable();
  };

  while (index < lines.length) {
    const line = lines[index];
    if (line.startsWith("```")) {
      if (inFence) {
        out.push(
          `    <pre><code class="language-${escapeAttribute(fenceLanguage)}">${fenceBuffer.join("\n")}</code></pre>`,
        );
        fenceBuffer.length = 0;
        inFence = false;
      } else {
        closeAll();
        inFence = true;
        fenceLanguage = line.slice(3).trim();
      }
      index += 1;
      continue;
    }
    if (inFence) {
      fenceBuffer.push(escapeHtml(line));
      index += 1;
      continue;
    }
    if (/^#{1,4} /.test(line)) {
      closeAll();
      const level = line.match(/^#+/u)[0].length;
      const text = line.slice(level + 1);
      out.push(`    <h${level} id="${escapeAttribute(slug(text))}">${renderInline(text)}</h${level}>`);
      index += 1;
      continue;
    }
    if (/^\|/.test(line)) {
      flushParagraph();
      flushList();
      tableBuffer.push(line.trim());
      index += 1;
      continue;
    }
    if (/^- /.test(line)) {
      flushParagraph();
      flushTable();
      if (!listOpen) {
        out.push("    <ul>");
        listOpen = true;
      }
      out.push(`      <li>${renderInline(line.slice(2))}</li>`);
      index += 1;
      continue;
    }
    if (line.trim() === "") {
      closeAll();
      index += 1;
      continue;
    }
    flushList();
    flushTable();
    paragraph.push(line.trim());
    index += 1;
  }
  closeAll();
  if (inFence) {
    // Unterminated fence: emit what was collected rather than dropping it.
    out.push(`    <pre><code>${fenceBuffer.join("\n")}</code></pre>`);
  }
  return out.join("\n");
}

export function slug(text) {
  return text
    .toLowerCase()
    .replace(/[^a-z0-9]+/gu, "-")
    .replace(/^-+|-+$/gu, "");
}

function pageHtml({ title, version, body, navigation, versionedPath, versions, currentVersion }) {
  const sections = navigation.sections
    .map((section) => {
      const links = section.pages
        .map((page) => {
          const active = page.path === versionedPath ? ' class="active"' : "";
          return `            <li><a${active} href="/docs/${PRODUCT_ID}/v${version}/${page.path.replace(/\.md$/u, ".html")}">${escapeHtml(page.title)}</a></li>`;
        })
        .join("\n");
      return `          <li>\n            <span class="section">${escapeHtml(section.title)}</span>\n            <ul>\n${links}\n            </ul>\n          </li>`;
    })
    .join("\n");
  const versionOptions = versions
    .map((candidate) => {
      const selected = candidate === currentVersion ? " selected" : "";
      return `            <option value="/docs/${PRODUCT_ID}/v${candidate}/"${selected}>v${candidate}</option>`;
    })
    .join("\n");
  return `<!doctype html>
<html lang="en">
  <head>
    <meta charset="utf-8">
    <meta name="viewport" content="width=device-width, initial-scale=1">
    <title>${escapeHtml(title)} — MCP Eval v${version}</title>
    <link rel="stylesheet" href="/docs/style.css">
  </head>
  <body>
    <header>
      <div class="brand"><a href="/docs/${PRODUCT_ID}/v${currentVersion}/introduction/overview.html">MCP Eval</a>
        <span class="version">v${version}</span>
        <select onchange="window.location.href = this.value" aria-label="Documentation version">
${versionOptions}
        </select>
      </div>
    </header>
    <nav aria-label="Documentation sections">
      <ul>
${sections}
      </ul>
    </nav>
    <main>
${body}
    </main>
    <footer>
      <p>Deterministic, share-safe documentation for <a href="https://github.com/${"cavi-ai/mcp-eval"}">mcp-eval</a> v${version}.</p>
    </footer>
  </body>
</html>
`;
}

function versionIndexHtml({ version, navigation, versions, currentVersion }) {
  const first = navigationPaths(navigation)
    .find((page) => page.endsWith(".md")) ?? "introduction/overview.md";
  return `<!doctype html>
<html lang="en">
  <head>
    <meta charset="utf-8">
    <meta http-equiv="refresh" content="0; url=/docs/${PRODUCT_ID}/v${version}/${first.replace(/\.md$/u, ".html")}">
    <title>MCP Eval v${version}</title>
    <link rel="canonical" href="/docs/${PRODUCT_ID}/v${version}/">
  </head>
  <body>
    <p>Redirecting to the <a href="/docs/${PRODUCT_ID}/v${version}/${first.replace(/\.md$/u, ".html")}">MCP Eval v${version} overview</a>.</p>
  </body>
</html>
`;
}

function notFoundHtml({ versions, currentVersion }) {
  const versionLinks = versions
    .map((candidate) => `        <li><a href="/docs/${PRODUCT_ID}/v${candidate}/introduction/overview.html">v${candidate}</a></li>`)
    .join("\n");
  return `<!doctype html>
<html lang="en">
  <head>
    <meta charset="utf-8">
    <title>Not found — MCP Eval docs</title>
    <link rel="stylesheet" href="/docs/style.css">
  </head>
  <body>
    <main>
      <h1>Page not found</h1>
      <p>Pick a version to browse:</p>
      <ul>
${versionLinks}
      </ul>
    </main>
  </body>
</html>
`;
}

const STYLESHEET = `:root { --ink: #16202a; --muted: #5b6b7a; --accent: #2563eb; --rule: #dde5ee; --panel: #f6f9fd; }
* { box-sizing: border-box; }
body { margin: 0; font: 16px/1.65 ui-sans-serif, system-ui, sans-serif; color: var(--ink); }
header { border-bottom: 1px solid var(--rule); background: var(--panel); }
.brand { display: flex; align-items: center; gap: 0.75rem; padding: 0.75rem 1.5rem; font-weight: 600; }
.brand a { color: inherit; text-decoration: none; }
.brand .version { color: var(--muted); font-weight: 400; }
select { font: inherit; }
nav { position: fixed; top: 53px; bottom: 0; left: 0; width: 260px; overflow-y: auto; border-right: 1px solid var(--rule); padding: 1rem; }
nav ul { list-style: none; margin: 0; padding: 0; }
nav .section { display: block; font-weight: 600; margin: 1rem 0 0.25rem; }
nav a { color: var(--accent); text-decoration: none; display: block; padding: 0.15rem 0.25rem; border-radius: 4px; }
nav a.active { background: var(--panel); font-weight: 600; }
main { margin-left: 260px; max-width: 52rem; padding: 2rem 2.5rem; }
pre { background: var(--panel); border: 1px solid var(--rule); border-radius: 8px; padding: 1rem; overflow-x: auto; }
code { font: 0.92em/1.5 ui-monospace, SFMono-Regular, Menlo, monospace; background: var(--panel); padding: 0.1em 0.35em; border-radius: 4px; }
pre code { background: none; padding: 0; }
table { border-collapse: collapse; width: 100%; margin: 1rem 0; }
th, td { border: 1px solid var(--rule); padding: 0.4rem 0.6rem; text-align: left; }
th { background: var(--panel); }
h1, h2, h3, h4 { line-height: 1.25; }
footer { margin-left: 260px; padding: 1rem 2.5rem; color: var(--muted); border-top: 1px solid var(--rule); }
a { color: var(--accent); }
`;

const REDIRECT_INDEX = `<!doctype html>
<html lang="en">
  <head>
    <meta charset="utf-8">
    <meta http-equiv="refresh" content="0; url=/docs/${PRODUCT_ID}/">
    <link rel="canonical" href="/docs/${PRODUCT_ID}/">
    <title>MCP Eval documentation</title>
  </head>
  <body>
    <p>Redirecting to <a href="/docs/${PRODUCT_ID}/">MCP Eval documentation</a>.</p>
  </body>
</html>
`;

export async function renderSite(input = {}) {
  const docsRoot = path.resolve(input.docsRoot ?? path.join(REPO_ROOT, `docs/${PRODUCT_ID}`));
  const outputRoot = path.resolve(input.output ?? path.join(REPO_ROOT, "docs/site"));
  if (!docsRoot.endsWith("docs") && !docsRoot.includes(path.join("docs", PRODUCT_ID))) {
    throw new Error("site rendering requires the versioned docs root");
  }
  // Version directories are the immediate children of docsRoot named
  // v<semver> containing a manifest.json — the deterministic identity of a
  // versioned build — sorted by semver.
  const versionDirs = [];
  const entries = await (await import("node:fs/promises")).readdir(docsRoot, { withFileTypes: true });
  for (const entry of entries) {
    if (!entry.isDirectory() || !entry.name.startsWith("v")) continue;
    const manifest = path.join(docsRoot, entry.name, "manifest.json");
    if (await stat(manifest).then(() => true).catch(() => false)) {
      versionDirs.push(entry.name.slice(1));
    }
  }
  if (!versionDirs.length) {
    throw new Error(`no versioned documentation found under ${docsRoot}`);
  }
  versionDirs.sort((left, right) => {
    const [lMajor, lMinor, lPatch] = left.split(".").map(Number);
    const [rMajor, rMinor, rPatch] = right.split(".").map(Number);
    return lMajor - rMajor || lMinor - rMinor || lPatch - rPatch;
  });
  const versions = versionDirs;
  const currentVersion = versions[versions.length - 1];

  await rm(outputRoot, { recursive: true, force: true });
  const written = [];
  for (const version of versions) {
    const root = path.join(docsRoot, `v${version}`);
    const manifest = JSON.parse(await readFile(path.join(root, "manifest.json"), "utf8"));
    if (manifest.version !== version) {
      throw new Error(`manifest version ${manifest.version} does not match directory v${version}`);
    }
    const navigation = JSON.parse(await readFile(path.join(root, "navigation.json"), "utf8"));
    for (const page of navigationPaths(navigation)) {
      const body = await readFile(path.join(root, page), "utf8");
      const title = page
        .split("/")
        .pop()
        .replace(/\.md$/u, "")
        .split("-")
        .map((word) => word.charAt(0).toUpperCase() + word.slice(1))
        .join(" ");
      const html = pageHtml({
        title,
        version,
        body: renderMarkdown(body),
        navigation,
        versionedPath: page,
        versions,
        currentVersion,
      });
      const target = path.join(outputRoot, "docs", PRODUCT_ID, `v${version}`, page.replace(/\.md$/u, ".html"));
      await mkdir(path.dirname(target), { recursive: true });
      await writeFile(target, html);
      written.push(path.relative(outputRoot, target).split(path.sep).join("/"));
    }
    const indexTarget = path.join(outputRoot, "docs", PRODUCT_ID, `v${version}`, "index.html");
    await writeFile(
      indexTarget,
      versionIndexHtml({ version, navigation, versions, currentVersion }),
    );
    written.push(path.relative(outputRoot, indexTarget).split(path.sep).join("/"));
  }
  // Style: one shared stylesheet.
  const styleTarget = path.join(outputRoot, "docs", "style.css");
  await mkdir(path.dirname(styleTarget), { recursive: true });
  await writeFile(styleTarget, STYLESHEET);
  written.push("docs/style.css");
  // Stable alias: /docs/index.html redirects to the current version.
  const aliasTarget = path.join(outputRoot, "docs", "index.html");
  await mkdir(path.dirname(aliasTarget), { recursive: true });
  await writeFile(
    aliasTarget,
    `<!doctype html>
<html lang="en">
  <head>
    <meta charset="utf-8">
    <meta http-equiv="refresh" content="0; url=/docs/${PRODUCT_ID}/v${currentVersion}/introduction/overview.html">
    <link rel="canonical" href="/docs/${PRODUCT_ID}/v${currentVersion}/introduction/overview.html">
    <title>MCP Eval documentation</title>
  </head>
  <body>
    <p>Redirecting to <a href="/docs/${PRODUCT_ID}/v${currentVersion}/introduction/overview.html">MCP Eval v${currentVersion} documentation</a>.</p>
  </body>
</html>
`,
  );
  written.push("docs/index.html");
  // Site root index for Pages' document root.
  await writeFile(path.join(outputRoot, "index.html"), REDIRECT_INDEX);
  written.push("index.html");
  // 404 page listing versions.
  await writeFile(
    path.join(outputRoot, "404.html"),
    notFoundHtml({ versions, currentVersion }),
  );
  written.push("404.html");
  written.sort(comparePortablePaths);
  return { outputRoot, versions, currentVersion, written };
}

if (process.argv[1] && import.meta.filename === process.argv[1]) {
  const values = parseOptions(process.argv.slice(2));
  renderSite({
    ...(values["docs-root"] ? { docsRoot: path.resolve(values["docs-root"]) } : {}),
    ...(values.output ? { output: path.resolve(values.output) } : {}),
  })
    .then(({ outputRoot, versions, currentVersion }) => {
      process.stdout.write(
        `rendered ${versions.join(", ")} -> ${outputRoot} (latest ${currentVersion})\n`,
      );
    })
    .catch((error) => {
      process.stderr.write(`${error instanceof Error ? error.message : String(error)}\n`);
      process.exitCode = 1;
    });
}