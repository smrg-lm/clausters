#!/usr/bin/env bash
# Build the application crate alone so workspace feature unification cannot
# turn notation on in the configuration that is meant to prove its absence.
set -euo pipefail
apps_root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
probe_root="$(mktemp -d "${TMPDIR:-/tmp}/clausters-apps-features.XXXXXX")"
trap 'rm -rf -- "$probe_root"' EXIT

python3 - "$apps_root" "$probe_root" <<'PY'
from pathlib import Path
import re
import sys
import tomllib

apps, probe = map(Path, sys.argv[1:])
workspace = tomllib.loads((apps.parent.parent / "Cargo.toml").read_text())
version = workspace["workspace"]["package"]["version"]
manifest = (apps / "Cargo.toml").read_text().replace(
    "version.workspace = true", f'version = "{version}"')
manifest = re.sub(r'path = "([^"]+)"',
    lambda match: f'path = "{(apps / match[1]).resolve()}"', manifest)
manifest += f'\n[lib]\npath = "{apps / "src/lib.rs"}"\n\n[workspace]\n'
(probe / "Cargo.toml").write_text(manifest)
# Preserve the checkout's dependency versions rather than resolving a new set.
(probe / "Cargo.lock").write_bytes((apps.parent.parent / "Cargo.lock").read_bytes())
PY

export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$apps_root/../../target}"
for app_features in none notation; do
    printf '\n== isolated applications: %s\n' "$app_features"
    feature_args=(--no-default-features)
    if [[ "$app_features" == notation ]]; then
        feature_args+=(--features notation)
    fi
    cargo test --offline --manifest-path "$probe_root/Cargo.toml" "${feature_args[@]}"
    cargo clippy --offline --manifest-path "$probe_root/Cargo.toml" \
        --all-targets "${feature_args[@]}" -- -D warnings
    RUSTDOCFLAGS=-Dwarnings cargo doc --offline --no-deps \
        --manifest-path "$probe_root/Cargo.toml" "${feature_args[@]}"
done
