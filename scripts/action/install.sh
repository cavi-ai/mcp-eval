#!/usr/bin/env bash
# Installs the mcpeval release pinned by distribution/release.json (or by the
# requested release's own release.json) after verifying its checksum
# companion, pinned SHA-256, and size.
set -euo pipefail

fail() {
  echo "mcpeval install: $*" >&2
  exit 1
}

if command -v mcpeval >/dev/null 2>&1; then
  echo "using preinstalled $(mcpeval --version)"
  exit 0
fi

case "${RUNNER_OS:-}" in
  Linux) platform=linux ;;
  macOS) platform=darwin ;;
  Windows) platform=win32 ;;
  *) fail "unsupported runner OS '${RUNNER_OS:-}'" ;;
esac
case "${RUNNER_ARCH:-}" in
  X64) arch=x64 ;;
  ARM64) arch=arm64 ;;
  *) fail "unsupported runner architecture '${RUNNER_ARCH:-}'" ;;
esac
key="$platform-$arch"

manifest="$ACTION_PATH/distribution/release.json"
repository=$(jq -r '.repository' "$manifest")
base_url="${MCPEVAL_RELEASE_BASE_URL:-https://github.com/$repository/releases/download}"
work=$(mktemp -d "$RUNNER_TEMP/mcpeval.XXXXXX")

if [ -n "${MCPEVAL_VERSION:-}" ]; then
  requested="v${MCPEVAL_VERSION#v}"
  curl -fsSL --retry 3 -o "$work/release.json" "$base_url/$requested/release.json" \
    || fail "could not download $base_url/$requested/release.json"
  manifest="$work/release.json"
  [ "$(jq -r '.tag' "$manifest" 2>/dev/null)" = "$requested" ] \
    || fail "$base_url/$requested/release.json does not describe $requested"
fi

tag=$(jq -r '.tag' "$manifest")
asset=$(jq -ce --arg key "$key" '.assets[$key]' "$manifest") \
  || fail "release $tag has no $key asset"
archive=$(jq -r '.archive' <<<"$asset")
sha256=$(jq -r '.sha256' <<<"$asset")
size=$(jq -r '.size' <<<"$asset")
url="$base_url/$tag/$archive"

curl -fsSL --retry 3 -o "$work/$archive.sha256" "$url.sha256" \
  || fail "could not download $url.sha256"
curl -fsSL --retry 3 -o "$work/$archive" "$url" \
  || fail "could not download $url"

companion=$(cat "$work/$archive.sha256"; printf x)
companion=${companion%x}
companion=${companion%$'\n'}
companion=${companion%$'\r'}
line=$'^([a-f0-9]{64})  ([^\r\n]+)$'
if ! [[ $companion =~ $line ]] || [ "${BASH_REMATCH[2]}" != "$archive" ]; then
  fail "invalid checksum companion for $archive"
fi
[ "${BASH_REMATCH[1]}" = "$sha256" ] \
  || fail "checksum companion does not match the pinned SHA-256 for $archive"

received=$(wc -c <"$work/$archive" | tr -d '[:space:]')
[ "$received" = "$size" ] \
  || fail "size mismatch for $archive: expected $size, received $received"

if command -v sha256sum >/dev/null 2>&1; then
  digest=$(sha256sum <"$work/$archive")
else
  digest=$(shasum -a 256 <"$work/$archive")
fi
[ "${digest%% *}" = "$sha256" ] || fail "archive SHA-256 mismatch for $archive"

bin_dir="$work/bin"
mkdir "$bin_dir"
if [[ $archive == *.zip ]]; then
  # Git Bash's GNU tar cannot read zip archives; Windows ships bsdtar.
  zip_tar=tar
  [ -z "${SYSTEMROOT:-}" ] || zip_tar="$SYSTEMROOT/System32/tar.exe"
  binary=mcpeval.exe
  "$zip_tar" -xf "$work/$archive" -C "$bin_dir" mcpeval.exe mcpeval-demo.exe \
    || fail "could not extract $archive"
else
  binary=mcpeval
  tar -xzf "$work/$archive" -C "$bin_dir" mcpeval mcpeval-demo \
    || fail "could not extract $archive"
fi

path_entry=$bin_dir
if command -v cygpath >/dev/null 2>&1; then
  path_entry=$(cygpath -w "$bin_dir")
fi
echo "$path_entry" >>"$GITHUB_PATH"
echo "installed $("$bin_dir/$binary" --version) ($key) from $url"
