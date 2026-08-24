"""The process launchers' guard rails (nothing here spawns a binary)."""

import socket

import pytest

from clausters.errors import ServerError
from clausters.launch import GuiProcess


def test_start_refuses_a_port_already_in_use():
    # A stale host on the port used to be adopted silently: the fresh child
    # could not bind, but the readiness poll got the stale one's reply. The
    # bind probe turns that into a clear error before anything is spawned.
    hold = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    hold.bind(("127.0.0.1", 0))
    port = hold.getsockname()[1]
    try:
        with pytest.raises(ServerError, match="already in use"):
            GuiProcess(port=port).start()
    finally:
        hold.close()


def test_a_launched_host_carries_the_typeface_it_was_given():
    # The launch-time half of `GuiHost.font`: a face that should be in place
    # before the first window opens is the host's own flag, not a message.
    assert "--font" not in GuiProcess()._argv()
    argv = GuiProcess(font="/usr/share/fonts/x.ttf")._argv()
    assert argv[argv.index("--font") + 1] == "/usr/share/fonts/x.ttf"


@pytest.mark.skipif(not __import__("sys").platform.startswith("linux"),
                    reason="PR_SET_PDEATHSIG is Linux-only")
def test_the_child_dies_with_a_killed_interpreter():
    # atexit covers a clean exit; the kernel death signal must cover the rest
    # (SIGKILL, a closed terminal) so no stale host squats the port.
    import os
    import signal
    import subprocess
    import sys
    import time

    from clausters import _cli

    try:
        _cli.gui_path()
    except Exception as e:  # pragma: no cover - source tree always bundles it
        pytest.skip(f"no clausters-gui binary: {e}")
    # --headless: the host must come up on a machine with no display (CI).
    code = ("import time; from clausters.launch import GuiProcess; "
            "p = GuiProcess(port=57931, extra_args=['--headless']).start(); "
            "print(p.proc.pid, flush=True); time.sleep(30)")
    child = subprocess.Popen([sys.executable, "-c", code],
                             stdout=subprocess.PIPE, text=True)
    try:
        gui_pid = int(child.stdout.readline())
        os.kill(child.pid, signal.SIGKILL)
        child.wait(timeout=5)
        for _ in range(50):
            try:
                os.kill(gui_pid, 0)
            except ProcessLookupError:
                return  # the host went down with the interpreter
            time.sleep(0.1)
        os.kill(gui_pid, signal.SIGKILL)
        pytest.fail("clausters-gui outlived the killed interpreter")
    finally:
        if child.poll() is None:
            child.kill()


def test_the_process_is_told_which_port_to_bind():
    # The handle's address is the one the process binds, which is what lets
    # several servers share a machine. Argv only — nothing is spawned.
    from clausters.launch import ServerProcess

    argv = ServerProcess(port=57130)._argv()
    assert argv[1:3] == ["--port", "57130"]
    assert ServerProcess()._argv()[1:3] == ["--port", "57110"]


def test_boot_refuses_a_handle_pointing_at_another_machine():
    # Booting starts a process *here*; a handle aimed elsewhere names a server
    # no boot of ours can produce. The port half is no longer pinned — that was
    # a lock on a binary that took no port flag.
    from clausters.defs import Server

    with pytest.raises(ValueError, match="this machine"):
        Server(host="192.168.1.9", transport="udp").boot(adopt_default=False)


def test_attach_raises_where_nobody_answers():
    # A bare `Server(...)` reaches nothing and says nothing; `attach` is the
    # verb that verifies, so a wrong address fails here instead of silently
    # dropping every later message into a UDP void.
    from clausters.defs import Server

    free = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    free.bind(("127.0.0.1", 0))
    port = free.getsockname()[1]
    free.close()
    server = Server(port=port, transport="udp")
    try:
        with pytest.raises(ServerError, match="no server answers"):
            server.attach(timeout=0.2)
    finally:
        server.interface.close()
