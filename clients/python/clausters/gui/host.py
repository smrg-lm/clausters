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

from ..base import _osclib
from ..base._oscinterface import OscTcpInterface, OscUdpInterface
from .guidef import to_json
from .handle import WindowHandle
from .ids import GuiIdAllocator

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
        #: window ids opened through `open` and not yet `close`d.
        self._open: set[int] = set()
        #: the one widget-id namespace for this host client — recycling, so a
        #: freed subtree's ids return to the pool (the GUI sibling of the audio
        #: server's `NodeIdAllocator`). Windows and widgets share it.
        self._alloc = GuiIdAllocator()
        #: id -> its child ids, for every widget this client defined — the
        #: subtree `free` walks to return the whole branch's ids to the pool.
        self._children: dict[int, list[int]] = {}
        #: id -> event handler (a `WidgetHandle.on_event`) and window id ->
        #: closed handler (a `WindowHandle.on_closed`), dispatched by `pump`.
        self._on_event: dict = {}
        self._on_closed: dict = {}
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
        windows and widgets share the host's one recycling id namespace, so a
        widget id must not repeat across windows (`open` draws its window ids
        from the same pool). A freed subtree's ids return to the pool."""
        return self._alloc.alloc()

    def open(self, tree: dict, *blobs: bytes, id: "int | None" = None) -> WindowHandle:
        """Open a window from a ``window``-rooted GuiDef and return its handle.

        A thin, id-managing wrapper over `define`: with ``id=None`` an id is
        assigned for you (and remembered so `close` / `close_all` can free it);
        pass an explicit ``id`` to name the root yourself. Id-less **widgets**
        inside ``tree`` are assigned too, in place — see `define`. The returned
        `clausters.gui.handle.WindowHandle` **is** the window id (an ``int``) and
        also resolves the tree's ``name``d widgets: ``win["cutoff"].set(…)``.
        Editing the open window is `set`; closing it is `close`. Any trailing
        ``blobs`` ride along exactly as in `define`."""
        if id is None:
            id = self.alloc_id()
        handle = self.define(id, tree, *blobs)
        self._open.add(int(id))
        return handle

    def close(self, id: int):
        """Close a window opened with `open` (or any widget subtree): ``/gui_free``
        frees the subtree and, for a ``window`` root, its OS window, and its ids
        return to the pool. The counterpart to `open`; `set` edits a window in
        between."""
        self.free(id)
        self._open.discard(int(id))

    def close_all(self):
        """Close every window still open through `open`. Handy at the end of a
        live session before dropping the host."""
        for id in list(self._open):
            self.close(id)

    def __enter__(self) -> "GuiHost":
        return self.start()

    def __exit__(self, *exc):
        self.stop()

    def define(self, id: int, tree: dict, *blobs: bytes) -> WindowHandle:
        """``/gui_def <id> <json> [blob…]`` — build a whole widget tree in one
        message, returning its `clausters.gui.handle.WindowHandle`. Any trailing
        ``blobs`` (e.g. waveform samples from
        `clausters.gui.guidef.samples_to_blob`) ride alongside the JSON and are
        referenced by index from a widget's ``blob`` property.

        Widgets built **without an id** (`clausters.gui.guidef` builders take
        ``id=None``) get a fresh host-unique one here, **written into the
        caller's dict in place** — so after ``define``/`open` the widget you
        kept a reference to reads back as ``widget["id"]``, ready for `set` /
        `bind`. Ids you did pick are kept verbatim; they share one recycling
        namespace across every window on this host (allocation starts at 1000,
        so hand ids below 1000 never collide with assigned ones).

        Any widget given a ``name`` is bound in the returned handle:
        ``define``/`open` walk the tree once, and ``win["cutoff"]`` resolves to
        that widget's `clausters.gui.handle.WidgetHandle`. Re-defining an
        existing id **redefines** it (the old subtree's ids return to the pool
        first, mirroring the host freeing the old subtree)."""
        id = int(id)
        if id in self._children:
            self._recycle_subtree(id, keep_root=True)
        names: dict = {}
        self._register(tree, id, names)
        self._osc.send_msg(self.target, "/gui_def", id, to_json(tree), *blobs)
        return WindowHandle(self, id, names)

    def _register(self, node: dict, node_id: int, names: dict):
        """Walk ``node`` (whose id is ``node_id``): assign a fresh id to every
        id-less descendant **in place**, record each id's children (the subtree
        `free` recycles), and collect ``name -> id``. The root carries no id in
        the tree — it is the ``/gui_def`` argument — so its id is passed in."""
        name = node.get("name")
        if isinstance(name, str) and name:
            names[name] = node_id
        child_ids: list[int] = []
        for child in node.get("children", ()):
            if "id" not in child:
                child["id"] = self.alloc_id()
            cid = child["id"]
            child_ids.append(cid)
            self._register(child, cid, names)
        self._children[node_id] = child_ids

    def _recycle_subtree(self, id: int, *, keep_root: bool):
        """Return ``id``'s subtree ids to the pool and forget its child map and
        event handlers. With ``keep_root`` the root id stays allocated (a
        redefine reuses it); a hand-picked id below the base was never allocated,
        so the pool ignores it."""
        for cid in self._children.pop(id, ()):
            self._recycle_subtree(cid, keep_root=False)
        self._on_event.pop(id, None)
        if keep_root:
            return
        self._on_closed.pop(id, None)
        self._alloc.free(id)

    def set(self, id: int, **props):
        """``/gui_set <id> <k> <v> ...`` — update one live widget. Property types
        are preserved: a Python ``int`` rides as an OSC int, a ``float`` as an
        OSC float."""
        args = []
        for k, v in props.items():
            args += [k, v]
        self._osc.send_msg(self.target, "/gui_set", id, *args)

    def free(self, id: int):
        """``/gui_free <id>`` — free a widget and its subtree, returning its ids
        to the pool (the client-side mirror of the host freeing the subtree)."""
        self._osc.send_msg(self.target, "/gui_free", id)
        self._recycle_subtree(int(id), keep_root=False)

    def bind(self, id: int, address: str, *prefix):
        """``/gui_bind <id> "server" <address> <prefix…>`` — forward this widget's
        value **straight to the audio server**, bypassing this script.

        On every change the host sends ``address`` (an OSC path like ``/node_set``
        or ``/bus_set``) with the fixed ``prefix`` arguments followed by the
        widget's value — e.g. ``bind(10, "/node_set", node_id, "freq")`` makes knob
        10 send ``/node_set <node_id> freq <value>`` to the server itself, so the
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

    # ---- event routing to the handle callbacks ----

    def _set_event_handler(self, id: int, func):
        """Register (or, with ``func=None``, clear) a `clausters.gui.handle.
        WidgetHandle.on_event` callback for widget ``id``."""
        if func is None:
            self._on_event.pop(id, None)
        else:
            self._on_event[id] = func

    def _set_closed_handler(self, id: int, func):
        """Register (or clear) a `clausters.gui.handle.WindowHandle.on_closed`
        callback for window ``id``."""
        if func is None:
            self._on_closed.pop(id, None)
        else:
            self._on_closed[id] = func

    def dispatch(self, addr, args) -> bool:
        """Route one inbound message to the handle callback registered for its id
        (`clausters.gui.handle.WidgetHandle.on_event` for ``/gui_event``,
        `WindowHandle.on_closed` for ``/gui_closed``). A ``/gui_closed`` also
        drops the window from the open set. Returns whether a callback ran."""
        if addr == "/gui_event" and args:
            func = self._on_event.get(int(args[0]))
            if func is not None:
                func(*args[1:])
                return True
        elif addr == "/gui_closed" and args:
            wid = int(args[0])
            self._open.discard(wid)
            func = self._on_closed.get(wid)
            if func is not None:
                func()
                return True
        return False

    def pump(self, timeout: float = 0.0) -> int:
        """Drain the host's pending messages, routing each to the handle
        callbacks registered with `clausters.gui.handle.WidgetHandle.on_event` /
        `WindowHandle.on_closed`. Returns how many were dispatched. The
        event-driven counterpart to `poll` (the raw primitive): call it from the
        script's loop — **never** the clock thread, which a routine must not
        block."""
        n = 0
        while (msg := self.poll(timeout)) is not None:
            if self.dispatch(*msg):
                n += 1
            timeout = 0.0  # only the first wait blocks
        return n

    def poll(self, timeout: float = 0.0):
        """One inbound message as ``(addr, args)``, or ``None`` within ``timeout``.

        The receive side of the protocol: the host pushes ``/gui_event`` (a widget
        was interacted with) and ``/gui_closed`` (a window was closed) back to the
        script that built the window. Drive an interactive panel by polling this
        in a loop, wrap it with a `clausters.responders.OscFunc`-style dispatch,
        or — for the handle callbacks — `pump` it.
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
