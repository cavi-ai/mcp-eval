#!/usr/bin/env bash
# Runs the probe battery for the composite action from INPUT_* variables,
# writes the job summary and step outputs, and exits with the probe's code
# (or the baseline diff's when the probe passed and the diff failed).
set -euo pipefail

usage() {
  echo "::error::mcpeval action: $*" >&2
  exit 2
}

# `mcpeval report` exits with the rendered report's verdict (0, 1, or 3).
render() {
  local status=0
  mcpeval report "$report" "$@" || status=$?
  case $status in
    0 | 1 | 3) ;;
    *) exit "$status" ;;
  esac
}

manifest=${INPUT_MANIFEST:-mcp-eval.manifest.json}
report=${INPUT_REPORT_PATH:-mcpeval.report.json}

command_args=()
trimmed=${INPUT_COMMAND:-}
trimmed=${trimmed#"${trimmed%%[![:space:]]*}"}
if [ "${trimmed:0:1}" = "[" ]; then
  parsed=$(mktemp)
  jq -j 'if type == "array" and length > 0 and all(.[]; type == "string")
         then .[] + "\u0000" else error("not a non-empty array of strings") end' \
    <<<"$trimmed" >"$parsed" 2>/dev/null \
    || usage "command starts with '[' but is not a JSON array of strings"
  while IFS= read -r -d '' arg; do
    command_args+=("$arg")
  done <"$parsed"
elif [ -n "$trimmed" ]; then
  read -r -d '' -a command_args <<<"$trimmed" || true
fi

has_command=false
[ "${#command_args[@]}" -eq 0 ] || has_command=true
has_url=false
[ -z "${INPUT_URL:-}" ] || has_url=true
[ "$has_command" != "$has_url" ] || usage "set exactly one of the command and url inputs"

args=(probe --server "$INPUT_SERVER" --manifest "$manifest" --format json)
[ "${INPUT_ALLOW_MUTATION:-false}" != true ] || args+=(--allow-mutation)
if [ "$has_url" = true ]; then
  args+=(--url "$INPUT_URL")
  [ "${INPUT_ALLOW_REMOTE_HTTP:-false}" != true ] || args+=(--allow-remote-http)
else
  args+=(-- "${command_args[@]}")
fi

# The pinned release may predate `diff` and `report --manifest`; fail with a
# clear message instead of a bare usage error from the probe.
if [ -n "${INPUT_BASELINE:-}" ] && ! mcpeval diff --help >/dev/null 2>&1; then
  usage "$(mcpeval --version) does not support baseline; set the version input to 0.3.0 or later"
fi
if [ "${INPUT_SARIF:-false}" = true ] && ! mcpeval report --help 2>&1 | grep -q -- --manifest; then
  usage "$(mcpeval --version) does not support sarif; set the version input to 0.3.0 or later"
fi

case "$report" in
  *.json) markdown="${report%.json}.md" ;;
  *) markdown="$report.md" ;;
esac

mkdir -p "$(dirname "$report")"
code=0
mcpeval "${args[@]}" >"$report" || code=$?

passed=false
{
  echo "report=$report"
  echo "exit-code=$code"
  if [ -s "$report" ]; then
    passed=$(jq -r '.passed' "$report")
    echo "readiness=$(jq -r '.readiness.score' "$report")"
  fi
  echo "passed=$passed"
} >>"$GITHUB_OUTPUT"

if [ -s "$report" ]; then
  render --format markdown >"$markdown"
  cat "$markdown" >>"$GITHUB_STEP_SUMMARY"
  echo "markdown=$markdown" >>"$GITHUB_OUTPUT"
fi

diff_code=0
if [ -n "${INPUT_BASELINE:-}" ] && [ -s "$report" ]; then
  diff_args=(diff "$INPUT_BASELINE" "$report" --format markdown --fail-on-regression)
  [ "${INPUT_FAIL_ON_CHANGE:-false}" != true ] || diff_args+=(--fail-on-change)
  mcpeval "${diff_args[@]}" | tee -a "$markdown" >>"$GITHUB_STEP_SUMMARY" || diff_code=$?
  echo "diff-exit-code=$diff_code" >>"$GITHUB_OUTPUT"
fi

if [ "${INPUT_SARIF:-false}" = true ] && [ -s "$report" ]; then
  render --format sarif --manifest "$manifest" >mcpeval.sarif
  echo "sarif=mcpeval.sarif" >>"$GITHUB_OUTPUT"
fi

if [ "$code" -eq 0 ] && [ "$diff_code" -ne 0 ]; then
  exit "$diff_code"
fi
exit "$code"
