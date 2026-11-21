import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

// The npm distribution publishes through npm trusted publishing (OIDC). No npm
// token is stored in this repository: the GitHub OIDC token (id-token: write) is
// exchanged by the npm CLI for a short-lived publish grant. These assertions
// keep the workflow from silently regressing to a stored-token publish, which
// is how the first attempt failed (empty NODE_AUTH_TOKEN).
const workflow = await readFile(
  new URL("../../.github/workflows/publish.yml", import.meta.url),
  "utf8",
);

test("npm publishes via tokenless OIDC trusted publishing", () => {
  assert.match(workflow, /id-token:\s*write/, "the npm job must request the OIDC id-token");
  // The trusted publisher on npmjs.com names this environment; it is part of
  // the OIDC subject claim, so dropping or renaming it stops publishing.
  assert.match(workflow, /^\s*environment:\s*production\s*$/m, "the npm job must run in the production environment");
  assert.match(
    workflow,
    /npm publish[^\n]*--provenance/,
    "publish must attach provenance",
  );
});

// crates.io trusted publishing: the job's OIDC token is exchanged by
// rust-lang/crates-io-auth-action for a 30-minute publish token.
const crates = await readFile(
  new URL("../../.github/workflows/publish-crates.yml", import.meta.url),
  "utf8",
);

test("crates.io publishes via tokenless OIDC trusted publishing", () => {
  assert.match(crates, /id-token:\s*write/, "the crates job must request the OIDC id-token");
  assert.match(crates, /^\s*environment:\s*production\s*$/m, "the crates job must run in the production environment");
  assert.match(crates, /uses: rust-lang\/crates-io-auth-action@v1\n\s+id: auth/);
  assert.match(crates, /CARGO_REGISTRY_TOKEN: \$\{\{ steps\.auth\.outputs\.token \}\}/);
  assert.doesNotMatch(crates, /secrets\.CARGO_REGISTRY_TOKEN/, "a stored crates.io token must not return");
});

test("a manual crates run publishes the tag it names and rehearses without one", () => {
  assert.match(crates, /workflow_dispatch:\n\s+inputs:\n\s+tag:/);
  assert.match(crates, /ref: \$\{\{ env\.TAG \|\| github\.ref \}\}/);
  assert.match(crates, /- name: Publish\n\s+if: env\.TAG != ''/);
});

test("no npm auth token is stored or referenced", () => {
  assert.doesNotMatch(
    workflow,
    /NPM_BOOTSTRAP_TOKEN|NPM_TOKEN|NODE_AUTH_TOKEN/,
    "npm publishing is OIDC only; a stored npm token must not return",
  );
});
