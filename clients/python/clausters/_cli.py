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


#: The client-side subcommands, and what each does to a *running* server. They
#: are words, while every server flag starts with a dash, so the two namespaces
#: cannot collide and anything else is forwarded to the binary untouched.
CLIENT_COMMANDS = ("stop", "panic", "status")

CLIENT_USAGE = """\
usage: clausters <command> [--port <n>]     acts on a server already running
  stop      stop it (/server_quit)
  panic     free every node, leaving the server up (/group_deepFree)
  status    report whether one answers, and how it is configured

Anything else is passed to the server binary: `clausters --help` for its flags.
"""


def client_main(argv: "list[str]") -> int:
    """Run one of the `CLIENT_COMMANDS` against a running server.

    This is the half of the console script that does **not** launch anything —
    the way to reach a server whose client is gone (crashed, closed, or never
    Python at all) without writing a script to say one sentence to it. A stray
    server holds the audio device and may well still be sounding, and `kill`
    should not be the only way to end that."""
    from .defs import Server
    from .errors import ClaustersError, ServerError

    command, rest = argv[0], argv[1:]
    port = None
    if rest[:1] == ["--port"] and len(rest) > 1:
        try:
            port = int(rest[1])
        except ValueError:
            print(f"clausters {command}: --port takes a number, not {rest[1]!r}",
                  file=sys.stderr)
            return 2
        rest = rest[2:]
    if rest:
        print(f"clausters {command}: unexpected argument {rest[0]!r}\n\n{CLIENT_USAGE}",
              file=sys.stderr)
        return 2
    server = Server(port=port, transport="udp")
    try:
        try:
            server.attach(adopt_default=False, reconcile=False)
        except ServerError:
            # Said in the terminal's terms, not the API's: from here you start a
            # server by running one, not by calling a method.
            print(f"clausters {command}: no server answers at "
                  f"{server.target.host}:{server.target.port}", file=sys.stderr)
            return 1
        if command == "stop":
            server.quit()
            print(f"stopped the server at {server.target.host}:{server.target.port}")
        elif command == "panic":
            server.free_all()
            print(f"freed every node on {server.target.host}:{server.target.port}")
        else:
            info = server.query_info()
            print(f"a server answers at {server.target.host}:{server.target.port}: "
                  f"{info.actual_sample_rate:.0f} Hz, {info.channels} out / "
                  f"{info.input_channels} in ch, {info.audio_buses} audio buses, "
                  f"{info.control_buses} control buses")
    except ClaustersError as e:
        print(f"clausters {command}: {e}", file=sys.stderr)
        return 1
    finally:
        server.close()
    return 0


def main(argv=None) -> int:
    """Run the bundled server, forwarding ``argv``. On POSIX this *replaces* the
    Python process (``os.execv``); on Windows it spawns and returns the exit
    code.

    A leading `CLIENT_COMMANDS` word is handled here instead, against a server
    already running (`client_main`)."""
    argv = sys.argv[1:] if argv is None else list(argv)
    if argv and argv[0] in CLIENT_COMMANDS:
        return client_main(argv)
    path = server_path()
    _ensure_executable(path)
    if os.name == "nt":
        import subprocess

        return subprocess.call([path, *argv])
    os.execv(path, [path, *argv])  # never returns on success
    return 0  # pragma: no cover
