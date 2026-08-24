#!/usr/bin/env bash
# Write the package version everywhere it lives, from the one place it is
# decided.
#
# The number lives in six files and only one of them is a decision: the root
# `[workspace.package].version`, which the eight Cargo crates inherit. The other
# five cannot inherit it — `clients/gui` is its own workspace, `pyproject.toml`
# and `package.json` are not Cargo's, and the two lockfiles record what their
# manifest said — so this writes them, and `tests/versions.rs` fails when any of
# them disagrees. (`clients/web/package-lock.json` had been two minors behind
# for exactly that reason.)
#
# This answers *where*, never *which*: the pre-1.0 breaking tier, when either
# ABI counter moves and the one-way linkage between them are the
# `release-versioning` skill's, and the number is bumped during development, not
# at release time.
#
# Usage:
#   scripts/set-version.sh            # print what every file says
#   scripts/set-version.sh 0.11.0     # write it everywhere
set -eu

root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"

read_version() {   # file, python expression over the text
    python3 - "$1" "$2" <<'PY'
import pathlib, re, sys
text = pathlib.Path(sys.argv[1]).read_text()
m = re.search(sys.argv[2], text, re.M)
print(m.group(1) if m else "?")
PY
}

WS='^\[workspace\.package\][^\[]*?^version = "([^"]+)"'
PKG='^\[package\][^\[]*?^version = "([^"]+)"'

report() {
    printf '%-34s %s\n' "Cargo.toml (workspace)" "$(read_version Cargo.toml "$WS")"
    printf '%-34s %s\n' "clients/gui/Cargo.toml" "$(read_version clients/gui/Cargo.toml "$PKG")"
    printf '%-34s %s\n' "clients/python/pyproject.toml" \
        "$(read_version clients/python/pyproject.toml '^version = "([^"]+)"')"
    printf '%-34s %s\n' "clients/web/package.json" \
        "$(read_version clients/web/package.json '^  "version": "([^"]+)"')"
    printf '%-34s %s\n' "clients/web/package-lock.json" \
        "$(read_version clients/web/package-lock.json '^  "version": "([^"]+)"')"
}

if [ $# -eq 0 ]; then
    report
    exit 0
fi

new="$1"
case "$new" in
    [0-9]*.[0-9]*.[0-9]*) ;;
    *) echo "set-version: '$new' is not a x.y.z version" >&2; exit 1 ;;
esac

python3 - "$new" <<'PY'
import pathlib, re, sys

new = sys.argv[1]

def sub(path, pattern, repl):
    p = pathlib.Path(path)
    text = p.read_text()
    out, n = re.subn(pattern, repl, text, count=1, flags=re.M)
    if n != 1:
        raise SystemExit(f"set-version: no version line in {path}")
    p.write_text(out)

# The one decision, and the three manifests that cannot inherit it.
sub("Cargo.toml",
    r'(\[workspace\.package\](?:\n(?!\[).*)*?\nversion = ")[^"]+(")',
    lambda m: m.group(1) + new + m.group(2))
sub("clients/gui/Cargo.toml",
    r'(\[package\](?:\n(?!\[).*)*?\nversion = ")[^"]+(")',
    lambda m: m.group(1) + new + m.group(2))
sub("clients/python/pyproject.toml", r'^version = "[^"]+"', f'version = "{new}"')
sub("clients/web/package.json", r'^  "version": "[^"]+"', f'  "version": "{new}"')
PY

# The lockfiles record what their manifest says; both are regenerated rather
# than edited, offline so a release does not depend on a registry being up.
cargo update -p clausters --offline >/dev/null
(cd clients/gui && cargo update -p clausters-gui --offline >/dev/null)
(cd clients/web && npm install --package-lock-only --ignore-scripts --silent >/dev/null)

echo "set-version: $new"
report
