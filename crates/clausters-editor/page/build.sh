#!/usr/bin/env bash
# Builds the editor as a page into dist/: the clausters web package it loads
# (the repository's clients/web, whose GUI host draws the applications, the
# audio editor among them), the editor's application compiled for the page
# (the crate's library, for wasm32, with its wasm-bindgen glue in editor/),
# then the page around it (vite.config.ts). The web package is built here so
# the page never serves applications older than its editor; with
# CLAUSTERS_WEB naming a package of its own, that one is served as it is.
#
#   ./build.sh            (release; `debug` for a faster, larger build)
#   npm run serve         -> serves dist/ with the clausters web package
#
# Needs the wasm32 target and the wasm-bindgen CLI at the workspace's version,
# as the web client's build does (clients/web/build.sh says how).
set -euo pipefail

cd "$(dirname "$0")"
profile="${1:-release}"
flag=""
[ "$profile" = release ] && flag="--release"

if [ -z "${CLAUSTERS_WEB:-}" ]; then
    (cd ../../../clients/web && ./build.sh "$profile")
fi
# shellcheck disable=SC2086
(cd ../../.. && cargo build -p clausters-editor --lib --target wasm32-unknown-unknown $flag)
wasm-bindgen --target web --out-dir editor \
    "../../../target/wasm32-unknown-unknown/$profile/clausters_editor.wasm"
npx vite build
