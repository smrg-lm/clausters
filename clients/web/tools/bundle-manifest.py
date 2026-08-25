#!/usr/bin/env python3
"""Writes <data-dir>/bundle.json — the manifest a browser bundle boot needs.

A native standalone bundle is a data directory the host and server read by
listing it; HTTP cannot list directories, so serving a bundle to the browser
boot (`bootBundle`, `<clausters-bundle>`, examples/panels/standalone.html) requires
this one extra file at the bundle's root. It
enumerates the persisted defs (file stems, exactly as saved) and names the
GuiDef; the optional "buffers" map (server buffer index -> audio URL relative
to the bundle) is left for hand-editing, since which files feed which buffers
is knowledge the authoring script holds, not the directory.

Usage: bundle-manifest.py <data-dir> [<gui-name>]
(gui-name defaults to the single GuiDef in the bundle; required when there are
several.)
"""

import json
import os
import sys


def stems(directory):
    if not os.path.isdir(directory):
        return []
    return sorted(f[:-5] for f in os.listdir(directory) if f.endswith(".json"))


def main(argv):
    if len(argv) < 2:
        sys.exit(__doc__.strip())
    data_dir = argv[1]
    guidefs = stems(os.path.join(data_dir, "defs", "guidefs"))
    if len(argv) > 2:
        gui = argv[2]
    elif len(guidefs) == 1:
        gui = guidefs[0]
    else:
        sys.exit(f"bundle has {len(guidefs)} GuiDefs {guidefs}; name one explicitly")

    manifest = {
        "gui": gui,
        "synthdefs": stems(os.path.join(data_dir, "defs", "synthdefs")),
        "graphdefs": stems(os.path.join(data_dir, "defs", "graphdefs")),
    }
    # Declaring the optional boot.json here spares the browser boot a probe
    # (whose 404 would land in the console). Re-run this tool after adding or
    # removing the preset.
    if os.path.exists(os.path.join(data_dir, "boot.json")):
        manifest["boot"] = True
    path = os.path.join(data_dir, "bundle.json")
    existing = {}
    if os.path.exists(path):
        with open(path) as f:
            existing = json.load(f)
    if "buffers" in existing:  # keep a hand-edited buffer map across re-runs
        manifest["buffers"] = existing["buffers"]
    with open(path, "w") as f:
        json.dump(manifest, f, indent=2)
        f.write("\n")
    print(f"wrote {path}: {manifest}")


if __name__ == "__main__":
    main(sys.argv)
