import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import path from "node:path";
import test from "node:test";

const ROOT = path.resolve(import.meta.dirname, "../..");
const SOURCE = path.join(ROOT, "docs/mcp-eval/source");
const CORPUS_REL = "data/readiness-corpus.json";

async function corpus() {
  return JSON.parse(await readFile(path.join(ROOT, CORPUS_REL), "utf8"));
}

function headlineCounts(observations) {
  const perfect = observations.filter((observation) => observation.score === 100).length;
  return { total: observations.length, perfect, below: observations.length - perfect };
}

test("readiness corpus follows its schema contract", async () => {
  const document = await corpus();
  assert.equal(document.schema, "mcpeval.readiness-corpus/v1");
  assert.ok(Array.isArray(document.observations));
  assert.ok(document.observations.length >= 2, "corpus needs a distribution");
  const names = document.observations.map((observation) => observation.server);
  assert.ok(names.every((name) => typeof name === "string" && /^[a-zA-Z0-9][a-zA-Z0-9._-]*$/u.test(name)));
  assert.ok(names.every((name, index) => names.indexOf(name) === index), "duplicate server observation");
  for (const observation of document.observations) {
    assert.ok(Number.isInteger(observation.score) && observation.score >= 0 && observation.score <= 100);
  }
});

test("README corpus claims match the checked-in readiness corpus", async () => {
  const readme = await readFile(path.join(ROOT, "README.md"), "utf8");
  const { total } = headlineCounts((await corpus()).observations);
  const countPhrases = [...readme.matchAll(/(\d+) popular public servers/gmu)].map((match) => Number(match[1]));
  assert.ok(countPhrases.length >= 1, "README should state a corpus size");
  for (const count of countPhrases) {
    assert.equal(count, total, `README corpus count ${count} != ${CORPUS_REL} count ${total}`);
  }
});

test("state-of-mcp-servers page matches the checked-in readiness corpus", async () => {
  const page = await readFile(
    path.join(SOURCE, "pages/guides/state-of-mcp-servers.md"),
    "utf8",
  );
  const observations = (await corpus()).observations;
  const { total, perfect, below } = headlineCounts(observations);

  const sizeClaims = [...page.matchAll(/\*\*(\d+) public MCP servers?\*\*/gmu)].map((match) => Number(match[1]));
  assert.ok(sizeClaims.length >= 1, "page should state the corpus size");
  for (const claim of sizeClaims) {
    assert.equal(claim, total, `page corpus size ${claim} != ${CORPUS_REL} count ${total}`);
  }

  const perfectClaims = [...page.matchAll(/Readiness 100\/100 \| (\d+)/gmu)].map((match) => Number(match[1]));
  assert.ok(perfectClaims.length >= 1, "page should table the 100/100 count");
  for (const claim of perfectClaims) {
    assert.equal(claim, perfect, `page 100/100 count ${claim} != corpus count ${perfect}`);
  }

  const belowClaims = [...page.matchAll(/Readiness below 100 \| (\d+)/gmu)].map((match) => Number(match[1]));
  assert.ok(belowClaims.length >= 1, "page should table the below-100 count");
  for (const claim of belowClaims) {
    assert.equal(claim, below, `page below-100 count ${claim} != corpus count ${below}`);
  }

  for (const claim of page.matchAll(/(\d+) of (\d+) servers score 100\/100/gmu)) {
    assert.equal(Number(claim[2]), total, `page denominator ${claim[2]} != corpus count ${total}`);
    assert.equal(Number(claim[1]), perfect, `page numerator ${claim[1]} != corpus count ${perfect}`);
  }

  // A named sub-100 server must actually be sub-100 in the corpus, and vice
  // versa: every sub-100 observation is named on the page.
  const belowServers = observations.filter((observation) => observation.score !== 100);
  for (const observation of belowServers) {
    assert.ok(
      page.includes(`\`${observation.server}\``),
      `sub-100 server ${observation.server} is not discussed on the page`,
    );
  }
});

test("CHANGELOG corpus claims do not contradict the current corpus", async () => {
  const changelog = await readFile(path.join(ROOT, "CHANGELOG.md"), "utf8");
  const { total } = headlineCounts((await corpus()).observations);
  // A claim in an unreleased section describes the checked-in corpus and
  // must match it. Once released, a section becomes historical: the
  // corpus it names is the one that shipped, so release sections are
  // exempt even when the corpus has since grown.
  const sections = [
    ...changelog.matchAll(/^## (Unreleased|\d+\.\d+\.\d+)(?: - \d{4}-\d{2}-\d{2})?$/gmu),
  ];
  assert.ok(sections.length >= 2, "changelog has no release section");
  const unreleased = changelog.slice(
    sections[0].index,
    sections[0][1] === "Unreleased" ? sections[1]?.index : sections[0].index,
  );
  const claims = [...unreleased.matchAll(/holds (\d+) public servers/gmu)].map(
    (match) => Number(match[1]),
  );
  for (const claim of claims) {
    assert.equal(claim, total, `changelog corpus claim ${claim} != ${CORPUS_REL} count ${total}`);
  }
});

test("docs probe tables cover every probe kind the CLI accepts", async () => {
  // ProbeKind::as_str in src/manifest.rs is the canonical label set: the
  // match arms inside `pub fn as_str` under `impl ProbeKind`.
  const source = await readFile(path.join(ROOT, "src/manifest.rs"), "utf8");
  const impl = source.slice(source.indexOf("impl ProbeKind"), source.indexOf("impl ProbeKind") + 2000);
  const body = impl.slice(impl.indexOf("pub fn as_str"));
  const labels = [...body.matchAll(/Self::[A-Za-z]+ => "([a-z0-9-]+)",/gmu)].map((match) => match[1]);
  assert.equal(labels.length, 19, `unexpected probe-kind count in src/manifest.rs: ${labels.length}`);

  const readme = await readFile(path.join(ROOT, "README.md"), "utf8");
  for (const label of labels) {
    assert.ok(readme.includes(`\`${label}\``), `README probe table is missing ${label}`);
  }

  const reference = await readFile(path.join(SOURCE, "pages/reference/evaluation-dimensions.md"), "utf8");
  for (const label of labels) {
    assert.ok(reference.includes(`\`${label}\``), `evaluation-dimensions reference is missing ${label}`);
  }
});