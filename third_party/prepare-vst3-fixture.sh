#!/usr/bin/env bash
# Restore the external gain example and apply the project's acceptance checks.
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
version=0.3.0
hash=e31e3fe7a6f028ded8c5a9984cf283319e6dc63572c9b71da1d4ee9d71d3b6bf
patch_hash="$(sha256sum "$here/vst3-fixture.patch" | cut -d ' ' -f1)"
pin="$hash:$patch_hash"
if [[ -f "$here/vst3-fixture/.clausters-pin" ]] && [[ $(cat "$here/vst3-fixture/.clausters-pin") == "$pin" ]]; then
    exit 0
fi
archive="${VST3_ARCHIVE:-$here/vst3-$version.crate}"
cleanup() { if [[ -z ${VST3_ARCHIVE:-} ]]; then rm -f "$archive"; fi; }
trap cleanup EXIT
if [[ ! -f "$archive" ]]; then
    curl -fL --retry 3 "https://static.crates.io/crates/vst3/vst3-$version.crate" -o "$archive"
fi
printf '%s  %s\n' "$hash" "$archive" | sha256sum --check --status
mkdir -p "$here/vst3-fixture"
tar -xOf "$archive" "vst3-$version/examples/gain.rs" > "$here/vst3-fixture/fixture.rs"
tar -xOf "$archive" "vst3-$version/LICENSE-MIT" > "$here/vst3-fixture/LICENSE-MIT"
patch --silent -d "$here/vst3-fixture" -p1 < "$here/vst3-fixture.patch"
printf '%s\n' "$pin" > "$here/vst3-fixture/.clausters-pin"
