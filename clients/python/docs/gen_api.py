"""Generate the API reference pages of the Python client book, one per package.

Run from anywhere; it runs pydoc-markdown once per page (with
`../pydoc-markdown.yml` for the processors and the renderer, and that page's
modules on the command line) and writes `src/api/<package>.md`. The pages are
git-ignored and rebuilt by `build.sh`; `src/SUMMARY.md` names them, so the
package names below are the contract with it.

One page per package rather than one for the whole client: a single page came
to 840 KB, too long to read or to scroll, and slow to lay out in a browser.
Only the public modules are listed. The private loaders and wire helpers
(_native, _osclib, _libpath, _midi, _oscinterface, _midiinterface) are
intentionally excluded.
"""

import os
import subprocess
import sys

# Each page: its package, then the submodules it documents (relative to it).
PAGES = [
    ("clausters", ["play", "plot", "render", "scope", "session", "responders", "bundle",
                   "ipc", "errors", "data", "document", "multitrack"]),
    ("clausters.base", ["builtins", "absobject", "stream", "clock", "timebase", "netaddr",
                        "main"]),
    ("clausters.seq", ["event", "pattern", "eventstream", "timeline"]),
    ("clausters.defs", ["expr", "signals", "boxes", "asdef", "faustdef", "synthdef",
                        "graphdef", "node", "bus", "buffer", "clocksync"]),
    ("clausters.defs.ugens", ["graph", "osc", "filter", "pan", "io", "buf", "spectral",
                              "trig", "demand", "env"]),
    ("clausters.defs.server", ["options", "queries", "transport", "streams"]),
    ("clausters.form", ["element", "aggregate", "render"]),
    ("clausters.gui", ["guidef", "host", "multitrack", "playhead_sync"]),
    ("clausters.gui.editing", ["context", "domain", "echo", "editor", "view"]),
    ("clausters.gui.notation", ["engraver", "mei", "sheet", "view"]),
]


def main() -> int:
    here = os.path.dirname(os.path.abspath(__file__))
    client = os.path.dirname(here)  # clients/python: pydoc-markdown.yml and clausters/
    out = os.path.join(here, "src", "api")
    os.makedirs(out, exist_ok=True)
    for name in os.listdir(out):
        if name.endswith(".md"):
            os.remove(os.path.join(out, name))
    for package, submodules in PAGES:
        args = ["pydoc-markdown", "pydoc-markdown.yml", "-m", package]
        for sub in submodules:
            args += ["-m", f"{package}.{sub}"]
        page = subprocess.run(args, cwd=client, check=True, stdout=subprocess.PIPE, text=True).stdout
        with open(os.path.join(out, f"{package}.md"), "w", encoding="utf-8") as fp:
            fp.write(f"# {package}\n\n{page}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
