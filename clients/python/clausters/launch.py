"""Launching the server and GUI as child processes, torn down automatically.

Live coding wants the whole system reachable from one interpreter: a *separate*
audio server process (so it survives a client restart, is shared, and keeps the
audio thread out of Python) and the visual server (`clausters-gui`) beside it —
without opening three terminals or spelling out a shared-memory path. This module
starts and owns those processes from Python:

- `ServerProcess` — spawns ``clausters --shm <auto> [server flags]`` and waits
  until it answers, choosing the shared-memory segment for you.
- `GuiProcess` — spawns ``clausters-gui --server <addr> --shm <same segment>``,
  wired to the server by construction.

Both are context managers, both register cleanup so **the child dies when this
interpreter exits** (a normal exit, an unhandled exception, or an abandoned
handle garbage-collected), and both sit under the ergonomic entry points:
`clausters.Session.live` (which boots a server if none is up) and
`clausters.Session.gui`, and the object-level `clausters.defs.Server.boot` /
`clausters.gui.GuiHost.boot`. Reach for those first; use these when you want to
own the raw processes explicitly.

The shared-memory segment is a Unix concept here (the GUI host maps it for
zero-message meters/scopes); on Windows `default_shm_path` returns ``None`` and
the processes fall back to the network paths.
"""

import atexit
import itertools
import os
import socket
import subprocess
import tempfile
import time
import weakref

from . import _cli
from .base._oscinterface import OscUdpInterface
from .errors import ServerError

__all__ = ["ServerProcess", "GuiProcess", "default_shm_path", "server_is_up", "DEFAULT_PORT"]

#: The audio server's fixed UDP port (``osc::server::DEFAULT_PORT``). The server
#: binary always binds this; it is not a CLI option.
DEFAULT_PORT = 57110

#: The GUI host's default UDP port (``clausters.gui.host.DEFAULT_PORT``).
GUI_DEFAULT_PORT = 57210

_shm_counter = itertools.count()


def default_shm_path() -> "str | None":
    """A fresh shared-memory segment path for a spawned server, or ``None`` where
    shared memory does not apply (Windows).

    On Linux the tmpfs mount ``/dev/shm`` is preferred (RAM-backed, no disk
    writes); otherwise a file under the system temp directory. The name carries
    this process's pid and a counter, so several servers in one session get
    distinct segments and stale files never collide."""
    if os.name == "nt":
        return None
    base = "/dev/shm" if os.path.isdir("/dev/shm") and os.access("/dev/shm", os.W_OK) \
        else tempfile.gettempdir()
    return os.path.join(base, f"clausters_{os.getpid()}_{next(_shm_counter)}")


def server_is_up(host: str = "127.0.0.1", port: int = DEFAULT_PORT,
                 timeout: float = 0.3) -> bool:
    """Whether an audio server already answers ``/server_status`` at ``host:port``.

    A quick UDP probe used to decide *boot-or-attach*: `Session.live` (and
    `clausters.defs.Server.boot`) attach to a running server if one replies, and
    start one only when none does — so the same call works whether or not a
    server is already up."""
    osc = OscUdpInterface().start()
    try:
        osc.send_msg((host, port), "/server_status")
        return osc.recv(timeout) is not None
    finally:
        osc.close()


def _verbosity_flags(verbose: int) -> "list[str]":
    """CLI verbosity flags shared by both binaries: ``-v`` repeated to raise the
    level, ``-q`` to lower it. ``0`` adds nothing (the default warn level)."""
    if verbose > 0:
        return ["-v"] * verbose
    if verbose < 0:
        return ["-q"] * (-verbose)
    return []


# Linux: the libc handle for PR_SET_PDEATHSIG, resolved once at import (the
# preexec hook runs between fork and exec, where importing is not safe).
_libc = None
if os.name == "posix" and os.uname().sysname == "Linux":  # pragma: no branch
    try:
        import ctypes

        _libc = ctypes.CDLL(None, use_errno=True)
    except OSError:  # pragma: no cover - no usable libc
        _libc = None


def _die_with_parent():
    """`Popen` preexec hook (Linux): have the kernel SIGTERM the child when
    this interpreter dies — *however* it dies. The atexit/finalizer teardown
    covers clean exits, but not a SIGKILL, a closed terminal's SIGHUP or a
    crashed kernel: without this a stale ``clausters``/``clausters-gui`` could
    survive and squat the port for the next session."""
    if _libc is not None:
        PR_SET_PDEATHSIG = 1
        _libc.prctl(PR_SET_PDEATHSIG, 15, 0, 0, 0)  # SIGTERM


def _terminate(proc: "subprocess.Popen"):
    """Stop a child politely (SIGTERM / ``terminate``), then forcibly (SIGKILL /
    ``kill``) if it does not exit promptly. Idempotent and never raises."""
    if proc is None or proc.poll() is not None:
        return
    try:
        proc.terminate()
    except OSError:
        return
    try:
        proc.wait(timeout=3.0)
    except subprocess.TimeoutExpired:
        try:
            proc.kill()
            proc.wait(timeout=2.0)
        except (OSError, subprocess.TimeoutExpired):
            pass


class _Process:
    """Shared machinery for the two launched processes: spawn, own, and tear down
    on close / interpreter exit. Subclasses supply the argv and the readiness
    check."""

    #: what to call the process in messages.
    kind = "process"

    def __init__(self):
        self.proc: "subprocess.Popen | None" = None
        self._finalizer: "weakref.finalize | None" = None
        self._atexit = None

    # -- subclass hooks --

    def _argv(self) -> "list[str]":  # pragma: no cover - abstract
        raise NotImplementedError

    def _wait_ready(self, deadline: float):  # pragma: no cover - abstract
        raise NotImplementedError

    # -- lifecycle --

    def start(self):
        """Spawn the process and block until it answers (or a `ServerError` on
        timeout). Idempotent: a second call while running is a no-op."""
        if self.proc is not None and self.proc.poll() is None:
            return self
        self._probe_port_free()
        argv = self._argv()
        try:
            # On Linux the child is bound to this interpreter's life by the
            # kernel (`_die_with_parent`); elsewhere only the atexit teardown
            # applies. Both server and GUI host go through here.
            preexec = _die_with_parent if _libc is not None else None
            self.proc = subprocess.Popen(argv, preexec_fn=preexec)
        except OSError as e:
            raise ServerError(f"could not launch {self.kind}: {e}") from e
        # Die with this interpreter even on an abandoned handle: a finalizer for
        # the GC path and an atexit for a clean exit (belt and suspenders; both
        # route through the idempotent `_terminate`).
        self._finalizer = weakref.finalize(self, _terminate, self.proc)
        self._atexit = lambda: _terminate(self.proc)
        atexit.register(self._atexit)
        try:
            self._wait_ready(time.monotonic() + self.ready_timeout)
        except Exception:
            self.close()
            raise
        return self

    #: seconds to wait for the process to answer before giving up.
    ready_timeout = 10.0

    def _probe_port_free(self):
        """Refuse to spawn over a port something else already owns.

        The readiness poll (`_wait_ready`) only checks that *something*
        answers on the port — if a stale process from an earlier session is
        still bound there, the fresh child cannot bind, yet the poll gets the
        stale one's reply and adopts it silently, so every later message goes
        to the old binary. A quick UDP bind probe turns that into a clear
        error instead."""
        try:
            probe = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
            try:
                probe.bind((self.host, self.port))
            finally:
                probe.close()
        except OSError as e:
            raise ServerError(
                f"port {self.port} is already in use — likely a stale "
                f"{self.kind} from an earlier session; close that process "
                f"(or attach to it instead of booting)") from e

    def _died_early(self):
        code = self.proc.poll() if self.proc else None
        if code is not None:
            raise ServerError(f"{self.kind} exited early (code {code})")

    def close(self):
        """Stop the process (if running) and drop the exit hooks. Idempotent;
        called automatically on context-manager exit and interpreter shutdown."""
        if self._atexit is not None:
            try:
                atexit.unregister(self._atexit)
            except Exception:
                pass
            self._atexit = None
        if self._finalizer is not None:
            self._finalizer.detach()
            self._finalizer = None
        _terminate(self.proc)
        self.proc = None

    def __enter__(self):
        return self.start()

    def __exit__(self, *exc):
        self.close()


class ServerProcess(_Process):
    """A separate ``clausters`` audio-server process, owned from Python.

    Spawns the standalone server binary (the wheel bundles it; a source checkout
    builds it) with a shared-memory segment chosen for you, and waits until it
    answers ``/server_status`` on its UDP port. `close` (or interpreter exit) stops it.

    Args:
        options: a `clausters.defs.ServerOptions` whose `args` size the server
            to match the client's allocators; ``None`` launches with the
            server's own defaults.
        shm: the shared-memory segment path. ``"auto"`` (the default) picks one
            with `default_shm_path`; a string forces a path; ``None`` launches
            without a segment (the meters/scopes then use the network fallback).
        verbose: server log verbosity — ``1``/``2``/``3`` for ``-v``/``-vv``/
            ``-vvv``, negative for ``-q`` (quiet).
        data_dir: ``--data-dir`` for the server's def store; ``None`` uses its
            default location.
        extra_args: extra CLI tokens appended verbatim (e.g. ``["--tcp"]``).
        binary: an explicit server-binary path; ``None`` locates it.

    The UDP port is the fixed server default (57110) — the binary does not take a
    port flag — so one machine runs one such server at a time.
    """

    kind = "clausters server"
    host = "127.0.0.1"
    port = DEFAULT_PORT

    def __init__(self, options=None, *, shm="auto", verbose: int = 0,
                 data_dir=None, extra_args=(), binary=None, ready_timeout: float = 10.0):
        super().__init__()
        self.options = options
        self.shm = default_shm_path() if shm == "auto" else shm
        self._verbose = verbose
        self._data_dir = data_dir
        self._extra = list(extra_args)
        self._binary = binary
        self.ready_timeout = ready_timeout

    def _argv(self) -> "list[str]":
        argv = [self._binary or _cli.server_path()]
        if self.options is not None:
            argv += self.options.args()
        if self.shm:
            argv += ["--shm", self.shm]
        if self._data_dir is not None:
            argv += ["--data-dir", str(self._data_dir)]
        argv += _verbosity_flags(self._verbose)
        argv += self._extra
        return argv

    def _wait_ready(self, deadline: float):
        """Poll ``/server_status`` until the server replies (its OSC front is bound and
        the engine is running)."""
        osc = OscUdpInterface().start()
        try:
            while time.monotonic() < deadline:
                self._died_early()
                osc.send_msg((self.host, self.port), "/server_status")
                if osc.recv(0.1) is not None:
                    return
                time.sleep(0.05)
        finally:
            osc.close()
        raise ServerError(
            f"{self.kind} did not answer /server_status within {self.ready_timeout:.0f}s")


class GuiProcess(_Process):
    """A ``clausters-gui`` visual-server (host) process, owned from Python.

    Spawns the GUI host binary wired to a running audio server: its client leg
    points at ``server`` and, when given, it maps the same shared-memory
    ``shm`` segment (so meters/scopes/playheads read the engine with no
    per-frame messages). `close` (or interpreter exit) stops it.

    The GUI binary is bundled in the ``clausters`` package alongside the server;
    a source checkout uses the one built under ``clients/gui/target``.
    `clausters._cli.gui_path` locates it.

    Args:
        server: the audio server address as ``"host:port"``, or ``None`` to run
            the host without a client leg (widgets that reference a server
            buffer or bind to the server then have no target).
        shm: the audio server's shared-memory segment path to map (Unix only),
            or ``None`` to skip it.
        port: the GUI host's own port (script -> host, UDP and TCP alike);
            default 57210.
        verbose: host log verbosity, like `ServerProcess`.
        data_dir: ``--data-dir`` for the GuiDef store; ``None`` uses the default.
        extra_args: extra CLI tokens appended verbatim.
        binary: an explicit host-binary path; ``None`` locates it.
    """

    kind = "clausters-gui host"
    host = "127.0.0.1"

    def __init__(self, server: "str | None" = None, *, shm: "str | None" = None,
                 port: int = GUI_DEFAULT_PORT, verbose: int = 0, data_dir=None,
                 extra_args=(), binary=None, ready_timeout: float = 10.0):
        super().__init__()
        self.server = server
        self.shm = shm
        self.port = port
        self._verbose = verbose
        self._data_dir = data_dir
        self._extra = list(extra_args)
        self._binary = binary
        self.ready_timeout = ready_timeout

    def _argv(self) -> "list[str]":
        argv = [self._binary or _cli.gui_path(), "--port", str(self.port)]
        if self.server:
            argv += ["--server", self.server]
        if self.shm:
            argv += ["--shm", self.shm]
        if self._data_dir is not None:
            argv += ["--data-dir", str(self._data_dir)]
        argv += _verbosity_flags(self._verbose)
        argv += self._extra
        return argv

    def _wait_ready(self, deadline: float):
        """Poll ``/gui_query`` until the host answers (its UDP front is bound). A
        fresh host replies ``/gui_info`` even for a missing widget id."""
        osc = OscUdpInterface().start()
        try:
            while time.monotonic() < deadline:
                self._died_early()
                osc.send_msg((self.host, self.port), "/gui_query", 0)
                if osc.recv(0.1) is not None:
                    return
                time.sleep(0.05)
        finally:
            osc.close()
        raise ServerError(
            f"{self.kind} did not answer within {self.ready_timeout:.0f}s")
