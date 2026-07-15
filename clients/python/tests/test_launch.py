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
    code = ("import time; from clausters.launch import GuiProcess; "
            "p = GuiProcess(port=57931).start(); "
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
