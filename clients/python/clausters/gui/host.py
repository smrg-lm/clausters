"""`GuiHost`: the client object that drives a ``clausters-gui`` host.

The GUI host is a *sibling OSC front* of the audio server: it speaks the same
OSC encoding over the same transports, only the vocabulary is ``/gui_*`` instead
of the audio commands. So `GuiHost` reuses the existing OSC interfaces
(`clausters.base.OscTcpInterface` by default — the host listens on TCP at the
same port, so a ``/gui_def`` tree is not bounded by a UDP datagram —
`clausters.base.OscUdpInterface` with ``transport="udp"``) pointed at the
host's port rather than the server's, and builds messages with the existing
encoder — there is no parallel wire code here. Keep the split: building the
GuiDef tree (see `clausters.gui.guidef`) is host-agnostic; only this object
talks to the host.

This is the request/reply face used at the skeleton milestone: ``define`` sends
a whole tree, ``set``/``free`` mutate it, and ``query`` round-trips a widget's
state back through ``/gui_info``. Event streams (``/gui_event``/``/gui_closed``)
flow through the responder model (`clausters.responders.OscFunc`) and are wired
up as the interactive widgets land.
"""

import itertools

from ..base import _osclib
from ..base._oscinterface import OscTcpInterface, OscUdpInterface
from .guidef import to_json

__all__ = ["GuiHost", "DEFAULT_PORT"]

#: The GUI host's default port, UDP and TCP alike (the host's
#: ``transport::DEFAULT_PORT``), clear of the audio server's family
#: (UDP/TCP 57110, WebSocket 57120).
DEFAULT_PORT = 57210


class GuiHost:
    """A connection to a running ``clausters-gui`` host.

    ``transport`` picks the carrier: ``"tcp"`` (default — reliable, and a
    ``/gui_def`` tree with its blobs can be as large as the host's frame
    ceiling) or ``"udp"`` (each message must fit a datagram; for constrained
    setups or a host started with ``--no-tcp``).
    """

    def __init__(self, host: str = "127.0.0.1", port: int = DEFAULT_PORT,
                 transport: str = "tcp"):
        self.target = (host, port)
        if transport == "tcp":
            self._osc = OscTcpInterface(host, port)
        elif transport == "udp":
            self._osc = OscUdpInterface()
        else:
            raise ValueError(f"unknown transport {transport!r} (tcp or udp)")
        #: window ids opened through `open` and not yet `close`d (auto-assigned
        #: ids start here, so they never clash with explicit small ids you pass).
        self._open: set[int] = set()
        self._ids = itertools.count(1000)
        #: the ``clausters-gui`` process this host started and owns (`boot`), if
        #: any; stopped by `stop`. ``None`` when connected to a host it did not
        #: start.
        self._process = None

    @classmethod
    def boot(cls, server: "str | None" = None, *, shm: "str | None" = None,
             port: "int | None" = None, transport: str = "tcp",
             verbose: int = 0, data_dir=None,
             extra_args=(), ready_timeout: float = 10.0) -> "GuiHost":
        """Start a ``clausters-gui`` visual-server process and return a `GuiHost`
        connected to and owning it.

        The launcher's ergonomic non-`Session` entry point for the GUI: it spawns
        the host binary (its client leg pointed at ``server`` and, when given,
        mapping the audio server's ``shm`` segment), waits until it answers, and
        hands back a started `GuiHost` whose `stop` also stops the process (as
        does interpreter exit). Pass a `clausters.defs.Server`'s address and its
        ``shm``, or let `clausters.Session.gui` wire those for you.

        Args:
            server: the audio server address as ``"host:port"``, or ``None`` for
                a host with no client leg.
            shm: the audio server's shared-memory segment path to map (Unix
                only), or ``None`` to skip it.
            port: the GUI host's own port (UDP and TCP alike); ``None`` uses
                the default (57210).
            transport: the carrier this `GuiHost` talks over — ``"tcp"``
                (default) or ``"udp"``.
            verbose: host log verbosity, like `clausters.defs.Server.boot`.
            data_dir: the host's ``--data-dir`` for its GuiDef store.
            extra_args: extra host CLI tokens.
            ready_timeout: seconds to wait for the host to answer.

        Returns:
            A started, process-owning `GuiHost`.
        """
        from ..launch import GUI_DEFAULT_PORT, GuiProcess

        port = GUI_DEFAULT_PORT if port is None else port
        proc = GuiProcess(server=server, shm=shm, port=port, verbose=verbose,
                          data_dir=data_dir, extra_args=extra_args,
                          ready_timeout=ready_timeout).start()
        host = cls("127.0.0.1", port, transport=transport).start()
        host._process = proc
        return host

    def start(self) -> "GuiHost":
        self._osc.start()
        return self

    def stop(self):
        """Close the connection and, if this host `boot`-ed a ``clausters-gui``
        process, stop it too."""
        self._osc.close()
        if self._process is not None:
            self._process.close()
            self._process = None

    # ---- windows: open / close (the tree is a `window`-rooted GuiDef) ----

    def alloc_id(self) -> int:
        """A fresh id, unique across everything this host client names —
        windows and widgets share the host's one id namespace, so a widget id
        must not repeat across windows (`open` draws its window ids from the
        same counter)."""
        return next(self._ids)

    def open(self, tree: dict, *blobs: bytes, id: "int | None" = None) -> int:
        """Open a window from a ``window``-rooted GuiDef and return its id.

        A thin, id-managing wrapper over `define`: with ``id=None`` an id is
        assigned for you (and remembered so `close` / `close_all` can free it);
        pass an explicit ``id`` to name the root yourself (e.g. to `set` its
        children by their own ids later). Id-less **widgets** inside ``tree``
        are assigned too, in place — see `define`. Editing the open window is
        `set`; closing it is `close`. Any trailing ``blobs`` ride along exactly
        as in `define`."""
        if id is None:
            id = next(self._ids)
        self.define(id, tree, *blobs)
        self._open.add(id)
        return id

    def close(self, id: int):
        """Close a window opened with `open` (or any widget subtree): ``/gui_free``
        frees the subtree and, for a ``window`` root, its OS window. The
        counterpart to `open`; `set` edits a window in between."""
        self.free(id)
        self._open.discard(id)

    def close_all(self):
        """Close every window still open through `open`. Handy at the end of a
        live session before dropping the host."""
        for id in list(self._open):
            self.close(id)

    def __enter__(self) -> "GuiHost":
        return self.start()

    def __exit__(self, *exc):
        self.stop()

    def define(self, id: int, tree: dict, *blobs: bytes):
        """``/gui_def <id> <json> [blob…]`` — build a whole widget tree in one
        message. Any trailing ``blobs`` (e.g. waveform samples from
        `clausters.gui.guidef.samples_to_blob`) ride alongside the JSON and are
        referenced by index from a widget's ``blob`` property.

        Widgets built **without an id** (`clausters.gui.guidef` builders take
        ``id=None``) get a fresh host-unique one here, **written into the
        caller's dict in place** — so after ``define``/`open` the widget you
        kept a reference to reads back as ``widget["id"]``, ready for `set` /
        `bind`. Ids you did pick are kept verbatim; they share one namespace
        across every window on this host (allocation starts at 1000, so hand
        ids below 1000 never collide with assigned ones)."""
        self._fill_ids(tree)
        self._osc.send_msg(self.target, "/gui_def", id, to_json(tree), *blobs)

    def _fill_ids(self, node: dict):
        """Assigns a fresh id to every id-less widget under ``node``, in place
        (the root itself carries no id — it is the ``/gui_def`` argument)."""
        for child in node.get("children", ()):
            if "id" not in child:
                child["id"] = self.alloc_id()
            self._fill_ids(child)

    def set(self, id: int, **props):
        """``/gui_set <id> <k> <v> ...`` — update one live widget. Property types
        are preserved: a Python ``int`` rides as an OSC int, a ``float`` as an
        OSC float."""
        args = []
        for k, v in props.items():
            args += [k, v]
        self._osc.send_msg(self.target, "/gui_set", id, *args)

    def free(self, id: int):
        """``/gui_free <id>`` — free a widget and its subtree."""
        self._osc.send_msg(self.target, "/gui_free", id)

    def bind(self, id: int, address: str, *prefix):
        """``/gui_bind <id> "server" <address> <prefix…>`` — forward this widget's
        value **straight to the audio server**, bypassing this script.

        On every change the host sends ``address`` (an OSC path like ``/n_set``
        or ``/c_set``) with the fixed ``prefix`` arguments followed by the
        widget's value — e.g. ``bind(10, "/n_set", node_id, "freq")`` makes knob
        10 send ``/n_set <node_id> freq <value>`` to the server itself, so the
        control responds with no round-trip through Python (the low-latency
        path). A bound widget stops emitting ``/gui_event``; `unbind` restores it.
        The host must have been started with ``--server`` for the value to reach
        the audio server. ``prefix`` items keep their type (an ``int`` rides as an
        OSC int, a ``str`` as a string)."""
        self._osc.send_msg(self.target, "/gui_bind", id, "server", address, *prefix)

    def unbind(self, id: int):
        """``/gui_bind <id>`` (no target) — remove a widget's binding, so its value
        flows back to this script as ``/gui_event`` again."""
        self._osc.send_msg(self.target, "/gui_bind", id)

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
