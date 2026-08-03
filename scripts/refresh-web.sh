#!/bin/sh
# Stage the built web client into the Jupyter package.
#
# The sibling of scripts/refresh-bin.sh, and it exists for the same reason: in
# this source checkout clausters-jupyter is installed editable, so the copy
# vendored inside the package wins over the workspace's, and it goes stale the
# moment a crate or a TypeScript module is rebuilt. A manual test can then
# silently exercise pre-change wasm.
#
# It builds nothing itself beyond delegating: clients/web/build.sh is the one
# builder of the web package (wasm bundles + the tsc emit), and this only
# copies what it produced.
#
#   scripts/refresh-web.sh          rebuild the web package, then stage it
#   scripts/refresh-web.sh --skip   stage what is already built
#
# The override CLAUSTERS_WEB_DIST bypasses the vendored copy entirely and is
# the better answer while iterating on the TypeScript:
#
#   export CLAUSTERS_WEB_DIST=$PWD/clients/web/dist
set -eu

root=$(cd "$(dirname "$0")/.." && pwd)
web="$root/clients/web"
pkg="$root/clients/jupyter/clausters_jupyter"

if [ "${1:-}" != "--skip" ]; then
    (cd "$web" && ./build.sh)
fi

if [ ! -f "$web/dist/index.js" ]; then
    echo "no build in $web/dist - run clients/web/build.sh first" >&2
    exit 1
fi

# The whole dist, so the package is self-contained offline; assets.py picks
# out what a given backend actually sends over the comm.
rm -rf "$pkg/_web"
mkdir -p "$pkg/_web"
cp -r "$web/dist/." "$pkg/_web/"

# The widget's own module is served by anywidget rather than sent over the
# comm, so it is staged apart, where widget.py's `_esm` points.
mkdir -p "$pkg/static"
cp "$web/dist/notebook/widget.js" "$pkg/static/widget.js"

echo "staged: $pkg/_web ($(du -sh "$pkg/_web" | cut -f1)) + static/widget.js"
