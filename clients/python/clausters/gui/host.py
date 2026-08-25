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

import json
from dataclasses import dataclass, field

from ..base import _osclib
from ..base._oscinterface import OscTcpInterface, OscUdpInterface
from .guidef import to_json, view as _view
from .handle import WindowHandle
from .ids import GuiIdAllocator
from ..errors import ReplyTimeout

__all__ = ["GuiHost", "WidgetInfo", "DEFAULT_PORT"]


@dataclass
class WidgetInfo:
    """What a widget **is now**, as `GuiHost.query` reports it: its ``type`` and
    the props it carries.

    A record like the server's (`clausters.defs.info`), and for the same reason:
    two fields addressed by name read better than a pair unpacked by position,
    and the web client answers with exactly this shape. Printing follows the
    same rule too -- ``repr`` names every field, ``str`` is the readable line.

    An empty ``type`` means the host has no such widget: it still answers, the
    way the server replies even on a miss.
    """

    type: str
    props: dict = field(default_factory=dict)

    def __str__(self) -> str:
        if not self.type:
            return "(no such widget)"
        shown = " ".join(
            f"{k}={v:g}" if isinstance(v, (int, float)) and not isinstance(v, bool)
            else f"{k}={v}"
            for k, v in self.props.items())
        return f"{self.type}{' ' + shown if shown else ''}"

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

    ``share`` takes one slice of the widget-id space instead of all of it, for
    a host with more than one client naming widgets on it — the same
    arrangement, and the same arithmetic, as the audio
    `clausters.defs.Server`'s (`clausters.base.IdShare`).

    ``interface`` supplies an already-built `clausters.base.OscInterface`
    instead, and then ``transport`` is not consulted — the same seam
    `clausters.defs.Server` has, for a carrier this module does not know
    about (a host reached over a carrier of the caller's own, a test double). The
    interface only has to speak the `clausters.base.OscInterface` protocol;
    ``host``/``port`` are still recorded as `target` for the sake of anything
    that reports where this client points.
    """

    def __init__(self, host: str = "127.0.0.1", port: int = DEFAULT_PORT,
                 transport: str = "tcp", interface=None, share=None):
        self.target = (host, port)
        #: whether this handle built its own carrier — a supplied ``interface``
        #: may reach a host over something that does not answer a UDP probe, so
        #: `attach` does not verify one (the audio `clausters.defs.Server` draws
        #: the same line).
        self._own_carrier = interface is None
        if interface is not None:
            self._osc = interface
        elif transport == "tcp":
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
        self._alloc = GuiIdAllocator(share=share)
        #: id -> its child ids, for every widget this client defined — the
        #: subtree `free` walks to return the whole branch's ids to the pool.
        self._children: dict[int, list[int]] = {}
        #: id -> the `clausters.gui.guidef.Source` objects that widget draws, so
        #: a freed widget stops being one of the live ends a `Source.set`
        #: pushes to.
        self._sources: dict[int, list] = {}
        #: window id -> the handle handed out for it, so a redraw can
        #: refresh it in place instead of orphaning the caller's copy.
        self._handles: dict = {}
        #: id -> event handler (a `WidgetHandle.on_event`) and window id ->
        #: closed handler (a `WindowHandle.on_closed`), dispatched by `pump`.
        #: The stamp of the last ``/gui_event`` seen -- what `ack` answers. The
        #: host numbers every edit it emits so an owner's reply can name which
        #: one it is about; zero means nothing has arrived yet.
        self.last_seq = 0
        #: The document version the last ``/gui_event`` was made against -- what
        #: the host had been told when the hand let go. Zero means the host
        #: cannot say, which is what an owner that never reports a version
        #: leaves it with, and which an owner reads as *apply unchecked*.
        self.last_version = 0
        self._on_event: dict = {}
        #: widget id -> {tag: callback} for the interface events (`on_press`,
        #: `on_release`, `on_click`). Kept apart from `_on_event` because they
        #: are a different vocabulary, not a filter over the same one, and
        #: because both may be registered on one widget at once.
        self._on_interface: dict = {}
        self._on_closed: dict = {}
        #: the ``clausters-gui`` process this host started and owns (`boot`), if
        #: any; stopped by `stop`. ``None`` when connected to a host it did not
        #: start.
        self._process = None

    def boot(self, server: "str | None" = None, *, shm: "str | None" = None,
             verbose: int = 0, data_dir=None, extra_args=(),
             ready_timeout: float = 10.0, adopt_ambient: bool = True) -> "GuiHost":
        """Start the ``clausters-gui`` process **this handle is for**, connect
        to it, and return ``self``.

        A `GuiHost` is a handle: constructing one runs nothing, and this is the
        verb that brings up what it points at. Unlike the audio server's, this
        handle's address does not move — the port is the one given to the
        constructor (57210 by default) and the process is told to use it — so
        booting only launches, waits, and starts the connection.

        Pair it with `stop`, which closes the connection and stops a process
        this booted. `clausters.Session.gui` does all of it wired to the
        session's server; this is the way to it without one.

        Args:
            server: the audio server address as ``"host:port"``, or ``None`` for
                a host with no client leg.
            shm: the audio server's shared-memory segment path to map (Unix
                only), or ``None`` to skip it.
            verbose: host log verbosity, like `clausters.defs.Server.boot`.
            data_dir: the host's ``--data-dir`` for its GuiDef store.
            extra_args: extra host CLI tokens.
            ready_timeout: seconds to wait for the host to answer.
            adopt_ambient: make this the **ambient** host when none is
                registered (`clausters.gui.set_ambient_host`), so ``view.open()``,
                `clausters.plot` and `clausters.scope` land here instead of
                booting a second host. First-wins, exactly as
                `clausters.defs.Server.boot`'s ``adopt_default``: an ambient host
                already registered is not displaced.

        Returns: ``self``, so ``GuiHost().boot()`` reads as one expression.
        """
        from ..launch import GuiProcess

        self._process = GuiProcess(
            server=server, shm=shm, port=self.target[1], verbose=verbose,
            data_dir=data_dir, extra_args=extra_args,
            ready_timeout=ready_timeout).start()
        self.start()
        if adopt_ambient:
            self._adopt_ambient()
        return self

    def _adopt_ambient(self):
        """Register as the ambient host when none is, first-wins — the mirror of
        `clausters.defs.Server.boot`'s ``adopt_default``."""
        from . import ambient_host, set_ambient_host

        if ambient_host() is None:
            set_ambient_host(self)

    def attach(self, *, timeout: float = 0.3, adopt_ambient: bool = True) -> "GuiHost":
        """Connect this handle to a host **already running** at its address, and
        return ``self``.

        The other half of `boot`, for the host nobody here started: one left
        behind by a script that ended, one launched from a terminal, one another
        process owns. Ownership is the difference and it runs through the pair —
        this handle did not start the process, so `stop` closes the connection
        and leaves the host standing, windows and all.

        Unlike a bare ``GuiHost(...).start()``, this **verifies**: a handle
        pointing where nobody answers raises here, rather than sending every
        later ``/gui_def`` into a void that reports nothing back.

        The probe goes over UDP whatever carrier this handle then talks over
        (`clausters.launch.gui_is_up`), so it says the host's front is bound,
        not that its TCP leg is up — a host started with ``--no-tcp`` answers the
        probe and then refuses a ``transport="tcp"`` connection.

        Args:
            timeout: seconds to wait for the ``/gui_query`` probe.
            adopt_ambient: make this the ambient host when none is registered,
                exactly as `boot` does.

        Returns: ``self``, so ``GuiHost(port=…).attach()`` reads as one
        expression.
        """
        from ..errors import ServerError
        from ..launch import gui_is_up

        if self._own_carrier and not gui_is_up(*self.target, timeout=timeout):
            raise ServerError(
                f"no GUI host answers at {self.target[0]}:{self.target[1]} — "
                "`boot()` one there, or point this handle where one is running")
        self.start()
        if adopt_ambient:
            self._adopt_ambient()
        return self

    def start(self) -> "GuiHost":
        self._osc.start()
        return self

    def stop(self):
        """Close the connection and, if this host `boot`-ed a ``clausters-gui``
        process, stop it too. A host that was the ambient one stops being it."""
        from . import ambient_host, set_ambient_host

        if ambient_host() is self:
            set_ambient_host(None)
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
        inside ``tree`` are assigned too, in the copy that is sent — the tree
        itself is untouched, so it can be opened again (see `define`). The returned
        `clausters.gui.handle.WindowHandle` **is** the window id (an ``int``) and
        also resolves the tree's ``name``d widgets: ``win["cutoff"].set(…)``.
        Editing the open window is `set`; closing it is `close`. Any trailing
        ``blobs`` ride along exactly as in `define`.

        **Any root opens.** A view with no parent is a window, so a root that is
        not one — a `clausters.gui.guidef.layout`, a lone `clausters.gui.guidef.knob`
        — is framed here in a window that **hugs** it: the frame is the client's,
        adds nothing but the OS window the wire needs (only a ``window``-rooted
        def becomes one), and is invisible to the handle, which goes on resolving
        the tree's names. Reach for `clausters.gui.guidef.view` when the window's
        own properties matter — a title, a size, a theme — since those are
        properties of a root nobody frames."""
        if tree.get("type") != "window":
            tree = _view(tree, hug=True)
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
        ``id=None``) get a fresh host-unique one here, in the **copy that is
        sent** — the caller's tree is left as it was written. So one tree opens
        as many times as you like, each instance with its own ids, and the way
        to a widget is its name through the returned handle rather than an id
        read back out of the document. Ids you did pick are kept verbatim; they
        share one recycling namespace across every window on this host
        (allocation starts at 1000, so hand ids below 1000 never collide with
        assigned ones) — and a hand-picked id on a subtree used **twice** in one
        tree is used twice, which the host answers by skipping the second.

        Any widget given a ``name`` is bound in the returned handle:
        ``define``/`open` walk the tree once, and ``win["cutoff"]`` resolves to
        that widget's `clausters.gui.handle.WidgetHandle`. Re-defining an
        existing id **redefines** it (the old subtree's ids return to the pool
        first, mirroring the host freeing the old subtree).

        **A redraw keeps a named widget's callback, and refreshes the handle you
        already hold** rather than handing back a second one. Both follow from
        what a name is for: the redefined tree gets fresh ids from the pool, so
        a handler kept under the old id would be orphaned (or, worse, fire for
        whatever widget inherited that number), and a `clausters.gui.handle.
        WindowHandle` captured before the redraw would resolve every name to an
        id that no longer means it. A callback belongs to the widget the name
        points at, not to the id it happened to have -- which is what lets an
        editor redraw its window (`clausters.gui.Editor.load`) without silently
        killing the transport bar beside it."""
        id = int(id)
        previous = self._handles.get(id)
        inherited: dict = {}
        inherited_hand: dict = {}
        root_handler = self._on_event.get(id)
        root_hand = self._on_interface.get(id)
        if id in self._children:
            if previous is not None:
                inherited = {name: self._on_event[wid]
                             for name, wid in previous._names.items()
                             if wid in self._on_event}
                # The interface handlers travel by name for the same reason:
                # `on_click` is a callback of the widget the name points at, and
                # a redrawn window that dropped them would look like a button
                # that stopped working.
                inherited_hand = {name: self._on_interface[wid]
                                  for name, wid in previous._names.items()
                                  if wid in self._on_interface}
            self._recycle_subtree(id, keep_root=True)
        names: dict = {}
        controls: dict = {}
        doc = self._stamp(tree, id, names, controls)
        if root_handler is not None:
            self._on_event[id] = root_handler
        if root_hand is not None:
            self._on_interface[id] = root_hand
        for name, func in inherited.items():
            wid = names.get(name)
            if wid is not None:
                self._on_event[wid] = func
        for name, table in inherited_hand.items():
            wid = names.get(name)
            if wid is not None:
                self._on_interface[wid] = table
        self._osc.send_msg(self.target, "/gui_def", id, to_json(doc), *blobs)
        if previous is not None:
            # Refreshed **in place**: one window is one handle, so every
            # reference the caller kept goes on resolving names correctly.
            previous._names.clear()
            previous._names.update(names)
            previous._controls.clear()
            previous._controls.update(controls)
            return previous
        handle = WindowHandle(self, id, names, controls)
        self._handles[id] = handle
        return handle

    def load(self, name: str):
        """``/gui_load <name>`` — instantiate a **persisted** GuiDef by name, the
        host replaying it as its saved ``/gui_def`` (it must have been started
        with a ``--data-dir``).

        The tree is the host's, not this client's: it carries the ids it was
        saved with, so nothing is allocated here and no `clausters.gui.handle.
        WindowHandle` comes back — address its widgets with `set` / `free` by
        the ids the def declares.
        """
        self._osc.send_msg(self.target, "/gui_load", name)

    def font(self, face: bytes):
        """``/gui_font <blob>`` — draw text with this typeface from now on.

        ``face`` is a raw TrueType/OpenType file (the host's rasterizer does not
        decompress WOFF2). A face is a property of the **host**, not of a
        window, so the call carries no id and every window it has open — and
        every one it opens later — draws with it.

        Loading one **relayouts nothing**: the size table never followed the
        typeface, so the same tree comes up the same size before and after and a
        face may be handed over at any point. What changes is that ``text_size``
        becomes continuous rather than quantized to half-steps of the cell,
        which a bitmap glyph's own pixels require.

        A host built without a rasterizer logs and keeps drawing with its
        embedded bitmap face — which is what it also does with bytes it cannot
        read. Neither is an error here: the bitmap face is the floor every build
        draws on. The launch-time spelling is `clausters.launch.GuiProcess`'s
        ``font=`` (the host's ``--font``), for a face that should be in place
        before the first window opens.
        """
        self._osc.send_msg(self.target, "/gui_font", bytes(face))

    def _stamp(self, node: dict, node_id: int, names: dict, controls: dict) -> dict:
        """A **copy** of ``node`` with a fresh id on every id-less descendant:
        the document ``/gui_def`` is sent, plus ``name -> id`` collected into
        ``names`` and each id's children recorded (the subtree `free` recycles),
        and ``controls`` collecting ``widget id -> def control name`` for every
        widget built from a control (what
        `clausters.gui.handle.WindowHandle.bind` wires).

        The caller's tree is never written into. That is what makes a `View` a
        definition rather than an instance: the same tree opens twice, and the
        *same* sub-view nested twice in one tree gets two id runs, instead of
        both branches sharing one and the host skipping the second (its
        ``"widget id already in use"``). Ids identify a live widget, so they
        belong to what `open` hands back, not to the document.

        The root carries no id in the tree — it is the ``/gui_def`` argument —
        so its id is passed in. A **duplicate name is refused** here, as it is
        when a `clausters.gui.guidef.View` is built: the name is how the handle
        addresses a widget, and a silent last-wins would leave the shadowed one
        drawing and unreachable.
        """
        name = node.get("name")
        if isinstance(name, str) and name:
            if name in names:
                raise ValueError(
                    f"duplicate widget name {name!r} in one tree — the handle "
                    "addresses a widget by name, so two widgets cannot share "
                    "one (the second would shadow the first, which would still "
                    "draw and be unreachable)")
            names[name] = node_id
        held_control = getattr(node, "_control", None)
        if held_control is not None:
            controls[node_id] = held_control.name
        for held in getattr(node, "_sources", ()):
            held._live.append((self, node_id))
            self._sources.setdefault(node_id, []).append(held)
        out = dict(node)
        child_ids: list[int] = []
        children = node.get("children")
        if children:
            stamped = []
            for child in children:
                cid = int(child["id"]) if "id" in child else self.alloc_id()
                child_ids.append(cid)
                sub = self._stamp(child, cid, names, controls)
                sub["id"] = cid
                stamped.append(sub)
            out["children"] = stamped
        self._children[node_id] = child_ids
        return out

    def _recycle_subtree(self, id: int, *, keep_root: bool):
        """Return ``id``'s subtree ids to the pool and forget its child map and
        event handlers. With ``keep_root`` the root id stays allocated (a
        redefine reuses it); a hand-picked id below the base was never allocated,
        so the pool ignores it."""
        for cid in self._children.pop(id, ()):
            self._recycle_subtree(cid, keep_root=False)
        for held in self._sources.pop(id, ()):
            held._live[:] = [end for end in held._live if end != (self, id)]
        self._on_event.pop(id, None)
        self._on_interface.pop(id, None)
        if keep_root:
            return
        self._on_closed.pop(id, None)
        self._handles.pop(id, None)
        self._alloc.free(id)

    def set(self, id: int, **props):
        """``/gui_set <id> <k> <v> ...`` — update one live widget.

        Property types are preserved: a Python ``int`` rides as an OSC int, a
        ``float`` as an OSC float. A **structural** value — an ``axes`` pair, a
        ``theme`` table, a list of ``points`` or ``notes`` — has no OSC type at
        all, so it rides as its JSON string; pass the object and it is
        serialized here, or pass the string yourself and it goes through
        untouched.

        A ``bool`` rides as ``1``/``0``, which is what a flag prop is on the
        wire: OSC's own boolean tags carry no argument, so a flag has always
        been an int there, and the builders emit one. Writing
        ``set(fills=False)`` is what a reader of ``fills=True`` in a `guidef`
        builder will type, so it means the same thing here.
        """
        args = []
        for k, v in props.items():
            if isinstance(v, bool):
                v = int(v)
            elif isinstance(v, (dict, list, tuple)):
                v = json.dumps(list(v) if isinstance(v, tuple) else v)
            args += [k, v]
        self._osc.send_msg(self.target, "/gui_set", id, *args)

    def ack(self, seq: int, doc_version: int = 0, generations=(), reason=None):
        """``/gui_ack <seq> <docVersion> [<source> <generation>…] [<reason>]`` —
        answer the edits this host emitted, up to ``seq``.

        The reply ``/gui_event`` never had. Without it the host cannot tell an
        edit the owner **refused** from one it took, so it goes on drawing what
        the hand did -- and cannot tell which of two gestures in flight an answer
        belongs to.

        There is no success flag, because there is nothing to branch on: the
        values the owner decided ride as ordinary `set` calls **in the same
        bundle** (see `push`), and *applied*, *applied transformed* and
        *refused* are the same message -- a refusal is simply the previous value
        pushed back. Send it **always**, including when nothing changed.

        Args:
            seq: the last stamp processed. Monotonic, so one number retires
                every edit at or below it and a lost acknowledgement is
                harmless.
            doc_version: the document's version after applying.
            generations: ``(source, generation)`` pairs for samples whose
                *content* changed -- the only thing that can tell a reader its
                copy is stale, since a destructive edit leaves the identity put.
            reason: why an edit was refused or transformed. Informational.
        """
        args = [int(seq), int(doc_version)]
        for source, generation in generations:
            args += [int(source), int(generation)]
        if reason is not None:
            args.append(str(reason))
        self._osc.send_msg(self.target, "/gui_ack", *args)

    def push(self, seq: int, *sets, doc_version: int = 0, generations=(),
             reason=None):
        """The state the owner decided, plus the acknowledgement, as **one
        bundle**.

        Args:
            seq: the stamp being answered (see `ack`).
            sets: ``(id, props)`` pairs, each becoming a ``/gui_set``.
            doc_version, generations, reason: as in `ack`.

        The acknowledgement goes **last**, after the values, and the whole thing
        is one packet: the host processes a bundle's messages in order as a
        unit, so it never sees a stamp retire an edit before the state that
        edit produced has arrived.
        """
        messages = []
        for id, props in sets:
            args = []
            for k, v in props.items():
                if isinstance(v, (dict, list, tuple)):
                    v = json.dumps(list(v) if isinstance(v, tuple) else v)
                args += [k, v]
            messages.append(("/gui_set", int(id), *args))
        ack = [int(seq), int(doc_version)]
        for source, generation in generations:
            ack += [int(source), int(generation)]
        if reason is not None:
            ack.append(str(reason))
        messages.append(("/gui_ack", *ack))
        self._osc.send_bundle(self.target, 0.0, *messages)

    def focus(self, id: int, on: bool = True):
        """``/gui_set <id> focus 1`` — point the keyboard at this widget
        (``on=False`` gives the focus up).

        The focused widget is the only one keys reach, and there is one focus
        per host. The user moves it by clicking or with Tab; this is the
        script's way, for a field that should be ready to type into the moment
        its window opens. A widget that reads no keyboard refuses it, and the
        move is reported back as a ``"focus"`` event on both the widget that
        gained it and the one that lost it.
        """
        self.set(id, focus=int(bool(on)))

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

    def bind_widget(self, id: int, target: int, prop: str):
        """``/gui_bind <id> "widget" <target> <prop>`` — apply this widget's value
        to **another widget's property**, with no round-trip through this script.

        On every change the host sets ``prop`` on widget ``target`` exactly as a
        `set` would — ``bind_widget(picker, pages, "index")`` makes a menu flip a
        ``stack``'s page, a slider drive a plot's ``max``, a curve write another
        curve's ``points`` (an edit-back payload rides as the JSON string the
        prop already takes). A bound widget stops emitting ``/gui_event``;
        `unbind` restores it.

        **A binding fires an apply, never another binding**: the target's own
        binding does not fire from it, so two widgets bound to each other settle
        instead of cascading. Nothing detects a cycle, because the chain is one
        hop by construction.
        """
        self._osc.send_msg(self.target, "/gui_bind", id, "widget", int(target), prop)

    def unbind(self, id: int):
        """``/gui_bind <id>`` (no target) — remove a widget's binding, so its value
        flows back to this script as ``/gui_event`` again."""
        self._osc.send_msg(self.target, "/gui_bind", id)

    def query(self, id: int, timeout: float = 1.0):
        """``/gui_query <id>`` -> the ``/gui_info`` reply as a `WidgetInfo`.

        What the widget **is now**: the props it was defined with, with every
        edit the user has made since laid over them — a dragged control's value,
        a moved clip's ``offset``/``dur``, an edited curve's ``points``. So this
        is how a script reads back what a gesture did without listening for the
        event that announced it.

        Scalars only, since the reply is flat OSC arguments: a structural prop
        nothing edits is not reported, while an edited one comes back as the
        JSON **string** its own ``set`` accepts, so what you read is what you
        could write.

        Raises `clausters.errors.ReplyTimeout` when the host does not answer,
        as every other query here does — a host that is up answers off its own
        event loop. An empty ``type`` (``""``) means the host has no such
        widget: it still answers, the way the server replies even on a miss.
        """
        self._osc.send_msg(self.target, "/gui_query", id)
        data = self._osc.recv(timeout)
        if data is None:
            raise ReplyTimeout(f"no /gui_info for widget {id} within {timeout}s")
        addr, args = _osclib.decode(data)
        if addr != "/gui_info" or not args:
            raise ReplyTimeout(f"no /gui_info for widget {id} within {timeout}s")
        # args = [id, type, k, v, k, v, ...]
        kind = args[1] if len(args) > 1 else ""
        props = {args[i]: args[i + 1] for i in range(2, len(args) - 1, 2)}
        return WidgetInfo(str(kind), props)

    # ---- event routing to the handle callbacks ----

    def _set_event_handler(self, id: int, func):
        """Register (or, with ``func=None``, clear) a `clausters.gui.handle.
        WidgetHandle.on_event` callback for widget ``id``."""
        if func is None:
            self._on_event.pop(id, None)
        else:
            self._on_event[id] = func

    #: The interface events a widget reports -- what the **hand** did, as
    #: against what the widget is worth. They arrive as a one-string payload and
    #: are the only such payload, which is what tells them from a value.
    INTERFACE_EVENTS = ("press", "release", "click")

    def _set_interface_handler(self, id: int, tag: str, func):
        """Register (or clear) a `clausters.gui.handle.WidgetHandle.on_press` /
        ``on_release`` / ``on_click`` callback for widget ``id``."""
        table = self._on_interface.setdefault(id, {})
        if func is None:
            table.pop(tag, None)
            if not table:
                self._on_interface.pop(id, None)
        else:
            table[tag] = func

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
            # ``<id> <seq> <version> <payload…>``: the stamp and the version the
            # edit was made against are the second and third arguments of every
            # event, before any tag, so one rule reads them all. A callback is
            # handed the payload -- these two are the host's bookkeeping, and
            # `ack` is what answers them.
            self.last_seq = int(args[1]) if len(args) > 1 else 0
            self.last_version = int(args[2]) if len(args) > 2 else 0
            wid = int(args[0])
            payload = args[3:]
            ran = False
            # The interface events first, and they are *also* handed to
            # `on_event`: that verb is the raw stream, so a script that reads
            # everything keeps reading everything.
            if len(payload) == 1 and payload[0] in self.INTERFACE_EVENTS:
                hand = self._on_interface.get(wid, {}).get(payload[0])
                if hand is not None:
                    hand()
                    ran = True
            func = self._on_event.get(wid)
            if func is not None:
                func(*payload)
                ran = True
            if ran:
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
