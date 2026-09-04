import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

// The npm distribution publishes through npm trusted publishing (OIDC), the
// same strategy as @cavi-ai/api-client and @cavi-ai/bobby-browser. No npm token
// is stored in this repository: the GitHub OIDC token (id-token: write) is
// exchanged by the npm CLI for a short-lived publish grant. These assertions
// keep the workflow from silently regressing to a stored-token publish, which
// is how the first attempt failed (empty NODE_AUTH_TOKEN).
const workflow = await readFile(
  new URL("../../.github/workflows/publish-distributions.yml", import.meta.url),
  "utf8",
);

test("npm publishes via tokenless OIDC trusted publishing", () => {
  assert.match(workflow, /id-token:\s*write/, "the npm job must request the OIDC id-token");
  assert.match(
    workflow,
    /npm publish[^\n]*--provenance/,
    "publish must attach provenance",
  );
});

test("no npm auth token is stored or referenced", () => {
  assert.doesNotMatch(
    workflow,
    /NPM_BOOTSTRAP_TOKEN|NPM_TOKEN|NODE_AUTH_TOKEN/,
    "npm publishing is OIDC only; a stored npm token must not return",
  );
});
