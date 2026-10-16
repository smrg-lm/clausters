#!/usr/bin/env python3
"""Look at an example with nobody at the screen: drive it through a headless
GUI host and leave a picture of every window after every step.

    scripts/look.py EXAMPLE [STEP ...]

EXAMPLE is a Python example (``clients/python/examples/...``). It runs the
way it runs in a notebook -- its interactive branch, which opens its windows
and returns -- against a GUI host that shows none: each window is drawn
offscreen, so this works while somebody uses the machine and on one with no
display. Then each STEP is taken in order:

* a **name** the example defines is called with no arguments (``tidy``,
  ``again``): the functions an example offers for its cells;
* a **number** waits that many seconds (``0.5``), for what moves on its own --
  a transport, a meter.

After the example opens and after every step, every window open is captured
(``GuiHost.capture``, which waits for the edits before it), into the example
tree's ``out/look/<example>/`` as ``<NN>-<step>.png`` -- or
``<NN>-<step>-<window>.png`` when more than one window is open. The directory
is emptied first, so what is there is this run's.

Run it with the client importable, after ``scripts/refresh-bin.sh``: the
bundled binaries are what an example launches. A window is drawn at the size
it declares, at a scale of one, so a picture is the same picture on any
machine -- within two levels a channel between a card and a software adapter
(``docs/decisions.md``, "A capture is the same picture on any adapter").

The host is made headless through the project layer of the configuration: the
example runs in a directory of its own whose ``clausters.toml`` says
``[gui] headless = true``, which overrides the user's file and nothing else.
"""

import argparse
import os
import runpy
import shutil
import sys
import tempfile
import time
from pathlib import Path


def out_dir(example: Path) -> Path:
    """The example tree's ``out/look/<example>/``: beside the nearest
    ``examples`` directory above the example."""
    for parent in example.parents:
        if parent.name == "examples":
            return parent / "out" / "look" / example.stem
    return example.parent / "out" / "look" / example.stem


def main() -> int:
    parser = argparse.ArgumentParser(
        description=__doc__.split("\n\n")[0],
        formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("example", type=Path)
    parser.add_argument("steps", nargs="*")
    args = parser.parse_args()
    example = args.example.resolve()
    if not example.is_file():
        parser.error(f"{args.example}: no such example")
    pictures = out_dir(example)
    shutil.rmtree(pictures, ignore_errors=True)
    pictures.mkdir(parents=True)

    # The project layer: a directory of its own whose config shows no window.
    workdir = Path(tempfile.mkdtemp(prefix="clausters-look-"))
    (workdir / "clausters.toml").write_text("[gui]\nheadless = true\n")
    os.chdir(workdir)
    # The interactive branch: an example opens its windows and returns
    # rather than waiting for them to close.
    sys.ps1 = "look> "
    sys.path.insert(0, str(example.parent))

    from clausters.gui import ambient_host

    namespace = {}
    taken = 0
    written = []

    def look(step: str) -> None:
        nonlocal taken
        host = ambient_host()
        windows = host.windows if host is not None else []
        if not windows:
            print(f"{step}: no window is open")
        for window in windows:
            suffix = f"-{window}" if len(windows) > 1 else ""
            path = pictures / f"{taken:02d}-{step}{suffix}.png"
            written.append(host.capture(window, str(path)))
        taken += 1

    try:
        namespace = runpy.run_path(str(example), run_name="__main__")
        look("opened")
        for step in args.steps:
            try:
                seconds = float(step)
            except ValueError:
                action = namespace.get(step)
                if not callable(action):
                    raise SystemExit(f"{example.name} defines no {step}() to call")
                action()
                look(step)
            else:
                time.sleep(seconds)
                look(f"after-{step}s")
    finally:
        session = namespace.get("session")
        if session is not None and hasattr(session, "close"):
            session.close()
        elif ambient_host() is not None:
            ambient_host().stop()
        shutil.rmtree(workdir, ignore_errors=True)
    for path in written:
        print(os.path.relpath(path, Path(__file__).resolve().parent.parent))
    return 0


if __name__ == "__main__":
    sys.exit(main())
