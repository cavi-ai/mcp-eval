## What changed

-

## Why

-

## Verification

- [ ] `cargo test --all-targets --locked`
- [ ] `cargo clippy --all-targets -- -D warnings` clean
- [ ] `cargo fmt --all --check` clean
- [ ] `python3 scripts/check-version-agreement.py` clean, if a version changed
- [ ] `npm run test:distribution && npm run verify:distribution`, if distribution touched
- [ ] `npm run docs:build && npm run docs:verify && npm run docs:test`, if docs touched
- [ ] Documentation release workflow dry-run output reviewed, if release delivery changed

## Contract impact

- [ ] No report or manifest schema change
- [ ] `mcpeval.probe-report/v1` or manifest `version` handling updated
