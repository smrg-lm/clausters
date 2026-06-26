"""`GuiHost`: the client object that drives a ``clausters-gui`` host.

The GUI host is a *sibling OSC front* of the audio server: it speaks the same
OSC encoding over the same transports, only the vocabulary is ``/gui_*`` instead
of the audio commands. So `GuiHost` reuses the existing OSC interface
(`clausters.base._oscinterface.OscUdpInterface`) pointed at the host's port
rather than the server's, and builds messages with the existing encoder — there
is no parallel wire code here. Keep the split: building the GuiDef tree (see
`clausters.gui.guidef`) is host-agnostic; only this object talks to the host.

This is the request/reply face used at the skeleton milestone: ``define`` sends
a whole tree, ``set``/``free`` mutate it, and ``query`` round-trips a widget's
state back through ``/gui_info``. Event streams (``/gui_event``/``/gui_closed``)
flow through the responder model (`clausters.responders.OscFunc`) and are wired
up as the interactive widgets land.
"""

from ..base import _osclib
from ..base._oscinterface import OscUdpInterface
from .guidef import to_json

__all__ = ["GuiHost", "DEFAULT_PORT"]

#: The GUI host's default UDP port (the host's ``transport::DEFAULT_PORT``),
#: clear of the audio server's family (UDP/TCP 57110, WebSocket 57120).
DEFAULT_PORT = 57210


class GuiHost:
    """A connection to a running ``clausters-gui`` host over UDP."""

    def __init__(self, host: str = "127.0.0.1", port: int = DEFAULT_PORT):
        self.target = (host, port)
        self._osc = OscUdpInterface()

    def start(self) -> "GuiHost":
        self._osc.start()
        return self

    def stop(self):
        self._osc.close()

    def __enter__(self) -> "GuiHost":
        return self.start()

    def __exit__(self, *exc):
        self.stop()

    def define(self, id: int, tree: dict, *blobs: bytes):
        """``/gui_def <id> <json> [blob…]`` — build a whole widget tree in one
        message. Any trailing ``blobs`` (e.g. waveform samples from
        `clausters.gui.guidef.samples_to_blob`) ride alongside the JSON and are
        referenced by index from a widget's ``blob`` property."""
        self._osc.send_msg(self.target, "/gui_def", id, to_json(tree), *blobs)

    def set(self, id: int, **props):
        """``/gui_set <id> <k> <v> ...`` — update one live widget. Property types
        are preserved: a Python ``int`` rides as an OSC int, a ``float`` as an
        OSC float."""
        args = []
        for k, v in props.items():
            args += [k, v]
        self._osc.send_msg(self.target, "/gui_set", id, *args)

    def free(self, id: int):
        """``/gui_free <id>`` — destroy a widget and its subtree."""
        self._osc.send_msg(self.target, "/gui_free", id)

    def query(self, id: int, timeout: float = 1.0):
        """``/gui_query <id>`` -> the ``/gui_info`` reply as ``(type, props)``.

        Returns ``None`` on timeout. An empty ``type`` (``""``) means the host
        has no such widget — it still answers, the way the server replies even on
        a miss.
        """
        self._osc.send_msg(self.target, "/gui_query", id)
        data = self._osc.recv(timeout)
        if data is None:
            return None
        addr, args = _osclib.decode(data)
        if addr != "/gui_info" or not args:
            return None
        # args = [id, type, k, v, k, v, ...]
        kind = args[1] if len(args) > 1 else ""
        props = {args[i]: args[i + 1] for i in range(2, len(args) - 1, 2)}
        return kind, props

    def poll(self, timeout: float = 0.0):
        """One inbound message as ``(addr, args)``, or ``None`` within ``timeout``.

        The receive side of the protocol: the host pushes ``/gui_event`` (a widget
        was interacted with) and ``/gui_closed`` (a window was closed) back to the
        script that built the window. Drive an interactive panel by polling this
        in a loop, or wrap it with a `clausters.responders.OscFunc`-style dispatch.
        """
        data = self._osc.recv(timeout)
        if data is None:
            return None
        return _osclib.decode(data)

    def listen(self, duration: float, handler):
        """Polls events for ``duration`` seconds, calling ``handler(addr, args)``
        for each. A small convenience for scripts and demos; for anything richer,
        use `poll` with your own loop or the responder model."""
        import time

        deadline = time.monotonic() + duration
        while time.monotonic() < deadline:
            msg = self.poll(timeout=0.1)
            if msg is not None:
                handler(*msg)
