# Corpus evidence action

Check an existing private corpus replay bundle and gate a workflow on its
verified replay verdict. The action runs the offline checker; it does not
install or launch an evaluator, package runner, state adapter, or server.
It uses GitHub's built-in `node24` action runtime and has no npm dependencies.
Self-hosted runners must support that runtime; see the
[GitHub action metadata reference](https://docs.github.com/en/actions/reference/workflows-and-actions/metadata-syntax#runs-for-javascript-actions).

Prepare the original corpus, targets, original reports, and replay bundle on
the runner using your own private storage policy. Then use a reviewed commit
containing this action:

```yaml
- name: Check corpus replay evidence
  id: corpus
  uses: cavi-ai/mcp-eval/actions/corpus-evidence@<reviewed-commit-sha>
  with:
    evidence: private/replay-evidence
    corpus: private/corpus.json
    targets: private/targets.json
    reports: private/original-reports
```

Replace the commit placeholder before running. Relative paths resolve from
the calling workspace; absolute paths also work. All four inputs are required.
When checking out this repository locally, use `./actions/corpus-evidence`.
The action is separate from the root probe action and is not in the npm package.

| Output | Meaning |
| --- | --- |
| `evidence-valid` | `true` when offline consistency checks pass; otherwise `false`. |
| `passed` | `true` only for intact passing evidence; otherwise `false`. |
| `exit-code` | `0` for intact passing evidence, `1` for intact failed evidence, `2` for invalid/incomplete evidence or a reporting failure. |
| `observations` | Originally observed target count; empty for invalid evidence. |
| `population` | Original target population count; empty for invalid evidence. |

Failed or invalid evidence fails the action. Outputs and a job summary are
written before the verdict exit, when runner file commands are available.
Reporting errors also fail the action; outputs may be unavailable. If using
`continue-on-error` to inspect failure outputs, preserve an explicit downstream
failure gate and treat missing outputs as failure. A passing verdict means
replay agreement under the recorded policy, not server health.

The action's outputs, summary, and diagnostics contain only fixed messages,
counts, and verdicts. They omit server labels, paths, report payloads, digests,
and arbitrary artifact metadata. The action does not upload any bundle or
report. GitHub may log workflow inputs independently of this action; use
secrets for sensitive paths and appropriate repository, runner, and storage
access controls. Do not place raw evidence in public workflow artifacts.

Offline checking establishes consistency of the supplied files, not author
authentication or proof that external checks ran. Retain trusted records of
the original inputs and verification artifact separately. Keep files immutable
during checking; the checker does not create a filesystem snapshot. See the
[corpus harness documentation](../../scripts/corpus/README.md) for the complete
evidence contract and its limitations.
