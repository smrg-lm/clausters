"""The ``clausters-editor`` console script: run the bundled application.

It locates the binary and runs it with the interpreter this package is
installed in as the session's default: until an environment is chosen in the
editor, the session imports what this environment has. The engraver the
binary links, bundled beside it (``_libs/``), is named to it with its data.

Lookup precedence: the ``CLAUSTERS_EDITOR_BIN`` override, the binary bundled in
the wheel (``clausters_editor/bin/``), then a source checkout's
``target/release/``.
"""

import os
import stat
import sys

_HERE = os.path.dirname(os.path.abspath(__file__))
_NAME = "clausters-editor.exe" if os.name == "nt" else "clausters-editor"
#: The repository's root, for a checkout's build (wheel/clausters_editor/ is
#: four folders below it).
_ROOT = os.path.join(_HERE, "..", "..", "..", "..")


def binary_path() -> str:
    """Absolute path to the application binary, or raise `SystemExit`."""
    candidates = [
        os.environ.get("CLAUSTERS_EDITOR_BIN"),
        os.path.join(_HERE, "bin", _NAME),
        os.path.join(_ROOT, "target", "release", _NAME),
    ]
    for c in candidates:
        if c and os.path.exists(c):
            return os.path.abspath(c)
    raise SystemExit(
        "clausters-editor: the application binary was not found. A wheel "
        "bundles it; in a source checkout build it with `cargo build -p "
        "clausters-editor --release`, or point CLAUSTERS_EDITOR_BIN at it."
    )


def _ensure_executable(path: str):
    """Best-effort: add the executable bit if missing (a wheel may unpack the
    bundled binary without it). Ignored if the location is read-only."""
    try:
        mode = os.stat(path).st_mode
        if not mode & stat.S_IXUSR:
            os.chmod(path, (mode | 0o555) & 0o7777)
    except OSError:
        pass


def main(argv=None) -> int:
    """Run the application, forwarding ``argv``. On POSIX this *replaces* the
    Python process (``os.execv``); on Windows it spawns and returns the exit
    code."""
    argv = sys.argv[1:] if argv is None else list(argv)
    path = binary_path()
    _ensure_executable(path)
    os.environ.setdefault("CLAUSTERS_EDITOR_DEFAULT_PYTHON", sys.executable)
    # the engraver's data, where the wheel bundles it beside its library
    engraver = os.path.join(_HERE, "_libs", "libverovio.so")
    if os.path.exists(engraver):
        os.environ.setdefault("CLAUSTERS_VEROVIO", engraver)
    if os.name == "nt":
        import subprocess

        return subprocess.call([path, *argv])
    os.execv(path, [path, *argv])  # never returns on success
    return 0  # pragma: no cover
