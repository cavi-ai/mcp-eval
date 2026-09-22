import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import path from "node:path";
import test from "node:test";

const ROOT = path.resolve(import.meta.dirname, "../..");
const SOURCE = path.join(ROOT, "docs/mcp-eval/source");

function section(markdown, title) {
  const marker = `## ${title}\n`;
  const start = markdown.indexOf(marker);
  assert.notEqual(start, -1, `missing section: ${title}`);
  const remainder = markdown.slice(start + marker.length);
  const end = remainder.search(/^## /mu);
  return end === -1 ? remainder : remainder.slice(0, end);
}

function listProbeNames(markdown) {
  return [...markdown.matchAll(/^- `([^`]+)`(?:[ \t].*)?$/gmu)].map((match) => match[1]);
}

function headingProbeNames(markdown) {
  return [...markdown.matchAll(/^### `([^`]+)`$/gmu)].map((match) => match[1]);
}

test("release identity follows the Cargo package and binary contracts", async () => {
  const {
    CLI_BINARY,
    DOCUMENTED_VERSION,
    PRODUCT_ID,
    RELEASE_REPOSITORY,
    resolveReleaseIdentity,
  } = await import("./lib.mjs");
  const release = resolveReleaseIdentity({
    version: DOCUMENTED_VERSION,
    tag: `v${DOCUMENTED_VERSION}`,
    commit: "0123456789abcdef0123456789abcdef01234567",
    sourceDateEpoch: 1700000000,
  });
  assert.deepEqual(
    {
      binary: CLI_BINARY,
      repository: RELEASE_REPOSITORY,
      slug: PRODUCT_ID,
      tag: release.tag,
      version: DOCUMENTED_VERSION,
    },
    {
      binary: "mcpeval",
      repository: "cavi-ai/mcp-eval",
      slug: "mcp-eval",
      tag: `v${DOCUMENTED_VERSION}`,
      version: DOCUMENTED_VERSION,
    },
  );
});

test("navigation references every official source page exactly once", async () => {
  const navigation = JSON.parse(await readFile(path.join(SOURCE, "navigation.json"), "utf8"));
  const paths = navigation.sections.flatMap((section) => section.pages.map((page) => page.path));
  assert.equal(navigation.title, "MCP Eval");
  assert.equal(paths.length, 14);
  assert.equal(new Set(paths).size, paths.length);
  for (const relative of paths) {
    assert.ok(await readFile(path.join(SOURCE, "pages", relative), "utf8"));
  }
});

test("official docs publish exactly five headline dimensions and label supplemental probes separately", async () => {
  const expectedHeadline = [
    "discovery-cost",
    "schema-guessability",
    "error-honesty",
    "state-recovery",
    "contention",
  ];
  const expectedSupplemental = [
    "token-cost",
    "degradation-over-n",
    "instruction-fidelity",
    "latency-budget",
    "pagination",
    "payload-bounds",
    "surface-listing",
    "output-schema",
    "cancellation",
    "protocol-negotiation",
    "sampling",
    "elicitation",
    "resource-subscription",
    "completion",
  ];
  const overview = await readFile(path.join(SOURCE, "pages/introduction/overview.md"), "utf8");
  const reference = await readFile(path.join(SOURCE, "pages/reference/evaluation-dimensions.md"), "utf8");
  assert.deepEqual(listProbeNames(section(overview, "Headline evaluation dimensions")), expectedHeadline);
  assert.deepEqual(listProbeNames(section(overview, "Supplemental probes")), expectedSupplemental);
  assert.deepEqual(headingProbeNames(section(reference, "Headline evaluation dimensions")), expectedHeadline);
  assert.deepEqual(headingProbeNames(section(reference, "Supplemental probes")), expectedSupplemental);
});

test("official docs require manual annotation review before sharing store records", async () => {
  const security = await readFile(
    path.join(SOURCE, "pages/security/privacy-and-authorization.md"),
    "utf8",
  );
  const findings = await readFile(
    path.join(SOURCE, "pages/guides/findings-and-verification.md"),
    "utf8",
  );
  const troubleshooting = await readFile(
    path.join(SOURCE, "pages/guides/troubleshooting.md"),
    "utf8",
  );
  const text = [security, findings, troubleshooting].join("\n");
  assert.doesNotMatch(text, /Only `<MCPEVAL_HOME>\/store\/` is safe to share/u);
  assert.match(security, /manually review or remove every annotation note/u);
  assert.match(findings, /Never put credentials, private paths, customer identifiers, or raw payload fragments in `--note`/u);
  assert.match(troubleshooting, /`doctor` prints a non-failing review warning/u);
});

test("official docs cover recovery and authorization contracts", async () => {
  const navigation = JSON.parse(await readFile(path.join(SOURCE, "navigation.json"), "utf8"));
  const pages = await Promise.all(
    navigation.sections.flatMap((section) => section.pages).map((page) => (
      readFile(path.join(SOURCE, "pages", page.path), "utf8")
    )),
  );
  const text = pages.join("\n");
  for (const phrase of [
    "discovery-cost",
    "schema-guessability",
    "error-honesty",
    "state-recovery",
    "contention",
    "token-cost",
    "latency-budget",
    "pagination",
    "payload-bounds",
    "surface-listing",
    "output-schema",
    "read-only by default",
    "--allow-mutation",
    "--format json",
    "--format markdown",
    "declared sandbox",
    "MCPEVAL_HTTP_AUTHORIZATION",
    "--confirm-read-only",
    "mcpeval-demo",
    "readiness",
    "mcpeval.probe-report/v1",
    "cavi-ai/mcp-eval@main",
    "mcpeval explain",
    "run_probe",
    "scaffold",
    "mcpeval share",
    "--price-per-mtok",
    "per session",
    "--format sarif",
    "mcpeval report",
    "--print-config",
    "--allow-spawn",
    "State of MCP servers",
    "corpus",
    "notifications/cancelled",
    "Remediation",
    "--brief",
    "corpus median",
    "scripts/corpus/collect.sh",
  ]) {
    assert.ok(text.includes(phrase), phrase);
  }
});

test("documentation publishes from the release chain and is gated in CI", async () => {
  const workflow = await readFile(path.join(ROOT, ".github/workflows/publish-docs.yml"), "utf8");
  for (const phrase of [
    "release:",
    "types: [published]",
    "workflow_call:",
    "workflow_dispatch:",
    "dry_run:",
    "node scripts/docs/build.mjs",
    "node scripts/docs/verify.mjs",
    "node scripts/docs/release-artifact.mjs",
    "mcp-eval-docs-${TAG}.tar.gz",
    "$DIRECTORY/$ARTIFACT.sha256",
    "if: ${{ env.DRY_RUN != 'true' }}",
    "CONSUMER_DISPATCH_TOKEN",
  ]) {
    assert.ok(workflow.includes(phrase), phrase);
  }
  assert.ok(workflow.indexOf("node scripts/docs/build.mjs") < workflow.indexOf("node scripts/docs/verify.mjs"));
  assert.ok(workflow.indexOf("node scripts/docs/verify.mjs") < workflow.indexOf("gh api --method POST"));

  // A release created with GITHUB_TOKEN fires no `release` event, so the tag
  // path must call this workflow directly once the release exists.
  const releaseBinaries = await readFile(path.join(ROOT, ".github/workflows/release-binaries.yml"), "utf8");
  assert.ok(releaseBinaries.includes("uses: ./.github/workflows/publish-docs.yml"));
  assert.ok(releaseBinaries.indexOf("gh release create") < releaseBinaries.indexOf("uses: ./.github/workflows/publish-docs.yml"));

  // The Cargo and documentation gates run on every push and pull request.
  const ci = await readFile(path.join(ROOT, ".github/workflows/ci.yml"), "utf8");
  for (const phrase of [
    "cargo fmt --all --check",
    "cargo clippy --all-targets",
    "cargo test --all-targets --locked",
    "npm run docs:build",
    "npm run docs:verify",
    "npm run docs:test",
  ]) {
    assert.ok(ci.includes(phrase), phrase);
  }
});

test("the documentation site renders every released version and deploys via Pages", async () => {
  const workflow = await readFile(path.join(ROOT, ".github/workflows/publish-docs.yml"), "utf8");
  for (const phrase of [
    "actions/deploy-pages@v4",
    "actions/upload-pages-artifact@v3",
    "actions/configure-pages@v5",
    "node scripts/docs/render-site.mjs",
    "needs: docs",
    "id-token: write",
    "pages: write",
  ]) {
    assert.ok(workflow.includes(phrase), phrase);
  }
  // Every version materializes from its own tag and verifies before the
  // site renders it.
  assert.ok(workflow.indexOf("git rev-list -n 1") < workflow.indexOf("node scripts/docs/build.mjs"));
  assert.ok(
    workflow.indexOf("node scripts/docs/verify.mjs") < workflow.indexOf("node scripts/docs/render-site.mjs"),
    "the site must render only after verification",
  );
});
