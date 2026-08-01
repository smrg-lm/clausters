"""The ``clausters`` console-script: run the bundled standalone server.

``pip install clausters`` puts a ``clausters`` command on the environment's
``PATH``; it locates the native server binary shipped inside the wheel
(``clausters/_bin/``) and execs it, forwarding every argument. So ``clausters
--tcp`` / ``clausters --shm /dev/shm/seg`` / ``clausters --nrt score out.wav``
behave exactly like the cargo-built binary.

This is the **separate** server — a real process you can point UDP/TCP clients,
``ShmClient`` or several machines at. The **in-process embedded** server needs
no command at all: `Session.embedded` opens one (`clausters.ipc.Clausters`
is the handle it owns).

Lookup precedence mirrors `clausters._libpath`: an explicit ``CLAUSTERS_BIN``
override, the binary bundled in the wheel, then a source checkout's workspace
``target/{release,debug}/``.
"""

import os
import stat
import sys

from . import _libpath

#: server-binary file names across platforms (POSIX / Windows).
_BIN_NAMES = ("clausters", "clausters.exe")
#: GUI-host binary file names across platforms.
_GUI_BIN_NAMES = ("clausters-gui", "clausters-gui.exe")


def server_path() -> str:
    """Absolute path to the standalone server binary, or raise `SystemExit`
    with a build hint if it cannot be found."""
    candidates = [os.environ.get("CLAUSTERS_BIN")]
    candidates += _libpath.bundled_bin_candidates(_BIN_NAMES)
    candidates += _libpath.workspace_candidates(_BIN_NAMES)
    for c in candidates:
        if c and os.path.exists(c):
            return c
    raise SystemExit(
        "clausters: standalone server binary not found. A wheel bundles it; in "
        "a source checkout build it with `cargo build --release --bin clausters` "
        "or point CLAUSTERS_BIN at it."
    )


def gui_path() -> str:
    """Absolute path to the ``clausters-gui`` visual-server binary, or raise
    `SystemExit` with a build hint if it cannot be found.

    The binary is bundled in the wheel alongside the server, so an installed
    package launches the GUI out of the box. Precedence mirrors `server_path`:

    1. the ``CLAUSTERS_GUI_BIN`` override,
    2. the binary bundled in the wheel (``clausters/_bin/``),
    3. a source checkout's ``clients/gui/target/{release,debug}/`` (the GUI crate
       is an independent workspace, so its ``target`` is separate).
    """
    candidates = [os.environ.get("CLAUSTERS_GUI_BIN")]
    candidates += _libpath.bundled_bin_candidates(_GUI_BIN_NAMES)
    candidates += _libpath.gui_workspace_candidates(_GUI_BIN_NAMES)
    for c in candidates:
        if c and os.path.exists(c):
            return c
    raise SystemExit(
        "clausters-gui: visual-server binary not found. A wheel bundles it; in "
        "a source checkout build it with `cargo build --release --bin "
        "clausters-gui` from clients/gui, or point CLAUSTERS_GUI_BIN at it."
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
    """Run the bundled server, forwarding ``argv``. On POSIX this *replaces* the
    Python process (``os.execv``); on Windows it spawns and returns the exit
    code."""
    argv = sys.argv[1:] if argv is None else list(argv)
    path = server_path()
    _ensure_executable(path)
    if os.name == "nt":
        import subprocess

        return subprocess.call([path, *argv])
    os.execv(path, [path, *argv])  # never returns on success
    return 0  # pragma: no cover
