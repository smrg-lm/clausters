#!/usr/bin/env bash
# Fetch pinned CLAP headers for the development bench and native host tests.
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
version=1.2.6
hash=6548b5831ad9e92551c752753e535b18fead4001165ef1df8e643efefa7133b7
if [[ -f "$here/clap/.clausters-pin" ]] && [[ $(cat "$here/clap/.clausters-pin") == "$hash" ]]; then
    exit 0
fi
archive="${CLAP_ARCHIVE:-$here/clap-$version.tar.gz}"
cleanup() { if [[ -z ${CLAP_ARCHIVE:-} ]]; then rm -f "$archive"; fi; }
trap cleanup EXIT
if [[ ! -f "$archive" ]]; then
    curl -fL --retry 3 "https://codeload.github.com/free-audio/clap/tar.gz/refs/tags/$version" -o "$archive"
fi
printf '%s  %s\n' "$hash" "$archive" | sha256sum --check --status
mkdir -p "$here/clap"
tar -xzf "$archive" --strip-components=1 -C "$here/clap"
printf '%s\n' "$hash" > "$here/clap/.clausters-pin"
