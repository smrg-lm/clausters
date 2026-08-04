"""`Bridge`: one carrier, many cells.

A `clausters.gui.GuiHost` is one client with one carrier, but a notebook draws
each window in its own output area, so the packets of one connection have to be
split across several widgets and their events merged back. That is this class,
and it is the whole of the multi-window story.

**Routing is by root, and the journal already knows it.** Every widget owns one
``window``-rooted def; `clausters_jupyter.journal.Journal` attributes each
packet to the root whose subtree it edits, which is exactly the question "which
cell does this belong to". So `Bridge` asks the journal rather than parsing
anything itself.

**A packet for a window nobody is showing is not lost, it is remembered.** The
verbs open a window and return it; the widget appears only when the window is
*displayed*, which may be later, in another cell, or never. Until then the
journal holds the tree, and displaying it replays. Nothing has to be sent
twice, and nothing is dropped for arriving early — the same mechanism that
already covered a cell's own output rendering after its code ran.

**Events come back through whichever widget carried them.** The page drains one
host into whatever comm is listening, so an event may arrive through a widget
other than the one that owns the widget it came from. It does not matter: they
all feed the one `CommInterface`, which is what the `GuiHost` reads.
"""

import threading
import uuid
import weakref

from .carrier import GUI_CHANNEL, CommInterface

__all__ = ["Bridge"]


class Bridge:
    """The link a `CommInterface` sends through, fanning out to the widgets.

    Args:
        widget_factory: ``factory()`` -> a new
            `clausters_jupyter.widget.ClaustersWidget`, called once per window
            actually displayed.
        engine: whether the widgets this makes boot the in-page audio engine.
            The bridge does not use it — it is the one place that can answer
            "is there an engine in this page", which `clausters_jupyter.audio`
            asks before offering a cell that would connect nothing.
    """

    def __init__(self, widget_factory, engine: bool = True):
        self._factory = widget_factory
        #: This notebook's id, handed to every widget. JupyterLab is a
        #: single-page application, so two notebooks open in one tab share a
        #: ``globalThis`` while allocating widget and node ids from the same
        #: base: without a key per kernel, the second one's `/gui_def 1000`
        #: redefines the first one's window and its `/synth_new 1000` collides
        #: with the first one's node.
        self.session = uuid.uuid4().hex
        #: Whether this notebook's audio runs in the page (``backend="page"``).
        self.has_engine = engine
        #: root (window) id -> the widget showing it.
        self._widgets: dict = {}
        #: channel -> [callback], the carriers listening (the GuiHost's).
        self._subs: dict = {}
        #: The gui carrier, set by `carrier` — its journal is the routing table.
        self._gui: "CommInterface | None" = None
        #: The widget dedicated to the audio leg, made by `audio_widget`. The
        #: engine lives in the page, so its packets need *a* cell open; a
        #: notebook that only plays has no window to ride along with.
        self._audio = None
        #: The widgets whose module in the page has announced itself. A widget
        #: object exists from the moment its window is displayed, but nothing
        #: in the page is listening until it mounts and registers its handler,
        #: so "a widget exists" is not "the page can receive" -- posting into
        #: that gap is posting into nowhere.
        #: A `weakref.WeakSet`: an entry must not keep a disposed widget
        #: alive, and must not survive it either -- an `id()` of a collected
        #: object is reused, and would mark a fresh widget ready before it is.
        self._ready: "weakref.WeakSet" = weakref.WeakSet()
        #: Audio packets sent before any cell was showing one, in order.
        #: **A queue, not a journal**: these were never delivered, which is a
        #: different thing from having been delivered and being replayable. A
        #: `/synth_new` replayed onto a reloaded page would start a second
        #: voice; this one has not started a first. It is drained once, by the
        #: first widget to appear, and never refilled — after that the engine
        #: is reachable and a send goes straight out.
        self._pending: list = []

    def carrier(self, channel: str = GUI_CHANNEL) -> CommInterface:
        """The `CommInterface` a `clausters.gui.GuiHost` or
        `clausters.defs.Server` takes as its ``interface``."""
        iface = CommInterface(self, channel)
        if channel == GUI_CHANNEL:
            self._gui = iface
        return iface

    def widget_for(self, window_id: int):
        """The widget showing ``window_id``, making and priming it on first
        ask. This is what a formatter calls when a window is displayed."""
        widget = self._widgets.get(window_id)
        if widget is None:
            widget = self._factory()
            widget.bridge = self
            # A window that asked for a height gets it: a canvas has no
            # intrinsic size, so without this every cell -- a two-lane scope, a
            # one-line plot, a multitrack -- comes out at the same default. The
            # width still follows the output area, which is the one dimension a
            # notebook really owns.
            height = self._height_of(window_id)
            if height is not None:
                widget.height = height
            self._widgets[window_id] = widget
        return widget

    def widget_ready(self, widget):
        """One widget's module has mounted in the page and is listening.

        Called by `clausters_jupyter.widget.ClaustersWidget` on the front end's
        ``ready``, which is the first moment anything sent to that cell can
        arrive. It is also what drains the audio queue: the opening move of
        every notebook is to send a def and start a synth *before* displaying
        anything, so those packets are waiting for exactly this.
        """
        self._ready.add(widget)
        self._drain_audio(widget)

    def widget_gone(self, widget):
        """One widget's view is gone from the page, and its cell is listening
        no longer.

        The mirror of `widget_ready`, and it has to exist because a comm
        outlives the view on it: re-running a cell clears its output and
        disposes the view, while the model stays alive and the kernel goes on
        sending into it -- and a message with no `render` at the other end is
        dropped by the front end without a trace. What that looked like is a
        notebook run a second time, from the top, in silence: every packet
        addressed to the audio cell of the *first* run.

        The audio cell is dropped rather than merely un-readied, since it is
        memoized (`audio_widget`): forgetting it is what lets `showing` answer
        no again, so the next thing with audio to send puts a live cell up.
        """
        self._ready.discard(widget)
        if widget is self._audio:
            self._audio = None

    def showing(self) -> bool:
        """Whether any cell is showing a widget of this bridge.

        Until one is, nothing of the browser half has happened — no comm, no
        wasm, no engine — which is what makes an auto-wired session cheap to
        throw away (`clausters_jupyter.session.notebook`).
        """
        return bool(self._widgets) or self._audio is not None

    def replay_for(self, widget) -> list:
        """The packets that rebuild, on a fresh front end, the window this
        widget shows — not the whole session's."""
        if self._gui is None or self._gui.journal is None:
            return []
        for window_id, known in self._widgets.items():
            if known is widget:
                return self._gui.journal.replay_root(window_id)
        return []

    def forget(self, widget):
        """Drop a widget (its output was cleared, its window freed)."""
        for window_id, known in list(self._widgets.items()):
            if known is widget:
                del self._widgets[window_id]

    # ---- the link `CommInterface` uses ----

    def send_packet(self, channel: str, payload: bytes, root=None):
        """Route one outbound packet to the widget showing its window.

        A packet whose window is not displayed goes nowhere and needs to go
        nowhere: the journal has it, and displaying the window replays.

        ``root`` is the window the carrier already resolved — it has to, since
        recording a `/gui_free` is what forgets the route. ``None`` asks here.
        """
        if channel != GUI_CHANNEL:
            self._send_audio(channel, payload)
            return
        if root is None:
            root = self._root_of(payload)
        widget = self._widgets.get(root) if root is not None else None
        if widget is None:
            return
        widget.send_packet(channel, payload)
        # A freed window's cell is emptied by the front end, so the widget it
        # held is gone too -- keeping the entry would route a later window's
        # packets, if it reused the id, to a canvas nobody can see.
        if root is not None and _frees(payload, root):
            del self._widgets[root]

    def audio_widget(self):
        """The cell that carries the audio leg, made on first ask.

        `clausters_jupyter.audio` displays it. It draws nothing — the engine
        has nothing to show — but it has to exist and be on screen, because
        the engine runs *in the page* and a browser starts no audio without a
        gesture in it.
        """
        if self._audio is None:
            self._audio = self._factory()
            self._audio.bridge = self
        return self._audio

    def _send_audio(self, channel: str, payload: bytes):
        """The audio leg: its own cell if there is one, else any mounted one.

        Any will do — the engine is one per notebook, not one per widget, so
        the packet reaches the same engine whichever cell carries it. What it
        may *not* ride is a cell whose module has not mounted yet: the comm is
        open from the moment the widget exists, but the handler at the other
        end is registered by `render`, and a message that arrives before it is
        dropped by the front end without a trace.

        So the packet waits in `_pending` until some cell is genuinely
        listening, which is what makes the ordinary opening — send a def,
        start a synth, *then* display something — work at all.
        """
        widget = self._audio if self._audio in self._ready else None
        if widget is None:
            widget = next((w for w in self._widgets.values()
                           if w in self._ready), None)
        if widget is None:
            self._pending.append((channel, payload))
            self.start_audio_cell()
            return
        widget.send_packet(channel, payload)

    def start_audio_cell(self, asked: bool = False):
        """Put the engine's own cell on screen, because something needs it.

        **A synth sounds when it is created**, and nothing about a window
        should come into that. The engine, though, runs in the *page*, and the
        page runs nothing at all until some cell has an output: anywidget
        serves the module as a widget's, so with no widget displayed there is
        no comm, no wasm and no `AudioContext` — the packet has nowhere to go
        and waits in `_pending`.

        The way out is not to make the caller display something. It is to
        display the one cell whose only job is to exist — the same cell
        `clausters_jupyter.audio` hands over by hand — the first time the
        notebook has audio to send. So the cell that starts a synth is the
        cell that starts sounding, which is what the client's semantics say
        everywhere else.

        Only from the kernel's own thread and only inside a cell: `display`
        writes to whatever cell is executing, and a routine sending from the
        clock thread has none. There the packet waits, and the next displayed
        anything delivers it.

        ``asked`` is the difference between a caller saying ``server.boot()``
        and this package deciding a packet needs somewhere to go. The guard
        below is for the second: it keeps "Run All" from putting an empty box
        under every cell that starts a synth right after opening a window. A
        caller who *asks* gets a cell, because the one state where the guard is
        wrong is the one where asking matters — a notebook being run a second
        time, whose cells the kernel has not yet been told are gone. Those
        messages queue behind the running cell (ipykernel holds the shell),
        so during the re-run of the first cell this still believes the *first*
        run's cells are on screen.
        """
        # `showing` and not "nothing is ready": a window already displayed
        # whose module has not announced itself yet needs no second cell -- its
        # own will deliver this the moment it mounts. Asking the narrower
        # question would put an empty box under every cell that starts a synth
        # right after opening a window, which "Run All" does every time.
        if not asked and self.showing():
            return
        display = _cell_display()
        if display is None:
            return
        if asked:
            # An explicit ask gets a *live* cell, not the memo of one whose
            # view may already be gone.
            self._audio = None
        display(self.audio_widget())

    def _drain_audio(self, widget):
        """Hand the first widget to appear whatever the audio leg could not
        send yet. Called once, by whichever cell got there first."""
        pending, self._pending = self._pending, []
        for channel, payload in pending:
            widget.send_packet(channel, payload)

    def subscribe(self, channel: str, callback):
        self._subs.setdefault(channel, []).append(callback)

    def unsubscribe(self, channel: str, callback):
        subs = self._subs.get(channel)
        if subs and callback in subs:
            subs.remove(callback)

    def inbound(self, channel: str, payload: bytes):
        """One packet up from any widget, into the one carrier."""
        for callback in list(self._subs.get(channel, ())):
            callback(payload)

    def _height_of(self, window_id: int):
        if self._gui is None or self._gui.journal is None:
            return None
        return self._gui.journal.height_of(window_id)

    def _root_of(self, packet: bytes):
        if self._gui is None or self._gui.journal is None:
            return None
        return self._gui.journal.root_of(packet)


def _cell_display():
    """IPython's `display`, but only when a cell is running on this thread.

    Two conditions, and both matter. Outside a kernel (a test, a plain REPL)
    there is no cell to draw in at all. Off the kernel's thread — a routine on
    the clock's — `display` would attribute its output to whichever cell
    happens to be executing, which is nobody's intent; those packets wait
    instead, and the notebook's next output delivers them.
    """
    if threading.current_thread() is not threading.main_thread():
        return None
    try:
        from IPython import get_ipython
        from IPython.display import display
    except ImportError:
        return None
    return display if get_ipython() is not None else None


def _frees(packet: bytes, root: int) -> bool:
    """Whether ``packet`` is the ``/gui_free`` of ``root`` itself, as opposed
    to one of a widget inside it (which leaves the window standing)."""
    from clausters.base import _osclib

    try:
        addr, args = _osclib.decode(packet)
    except Exception:
        return False
    return addr == "/gui_free" and bool(args) and args[0] == root
