#!/usr/bin/env bash
# Keeps one pull-request comment per server holding the rendered report and
# baseline diff: created on the first run, edited in place on later runs.
# A comment that cannot be written is a warning, never the gate's verdict.
set -euo pipefail

# GitHub rejects comment bodies over 65,536 characters.
limit=60000
marker="<!-- mcpeval:${INPUT_SERVER} -->"
body=$(mktemp)
{
  echo "$marker"
  if [ "$(wc -c <"$MARKDOWN")" -gt "$limit" ]; then
    head -c "$limit" "$MARKDOWN"
    printf '\n\n_Truncated; the full report is in the job summary._\n'
  else
    cat "$MARKDOWN"
  fi
} >"$body"

warn() {
  echo "::warning::mcpeval action: could not comment on the pull request; the job needs pull-requests: write"
  exit 0
}

comments="repos/$GITHUB_REPOSITORY/issues/$PR_NUMBER/comments"
existing=$(gh api --paginate "$comments" \
  --jq ".[] | select(.body | startswith(\"$marker\")) | .id" 2>/dev/null) || warn
existing=${existing%%$'\n'*}
if [ -n "$existing" ]; then
  gh api --method PATCH "repos/$GITHUB_REPOSITORY/issues/comments/$existing" \
    -F "body=@$body" >/dev/null 2>&1 || warn
else
  gh api --method POST "$comments" -F "body=@$body" >/dev/null 2>&1 || warn
fi
