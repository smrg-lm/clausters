"""`ClaustersWidget`: the cell's end of the wire.

One widget is one canvas — one ``window``-rooted GuiDef drawn in one output
area — and its whole job is to move bytes. It does not know what a GuiDef is,
what a `/gui_set` means or which backend is running; it carries tagged OSC
packets in both directions and hands the front end its assets on mount. Every
decision about *what* to send belongs upstream, in the client that already
knows.

It is the ``link`` `clausters_jupyter.carrier.CommInterface` talks to, and the
two are deliberately split: the carrier is testable with a fake link and no
kernel (`clients/jupyter/tests/test_carrier.py`), and the widget is the only
file that imports anywidget.

**Two channels on one comm.** A notebook has a single channel to the page and
two peers behind it — the GUI host and, for the in-page backend, the engine —
so every payload is tagged `~clausters_jupyter.carrier.GUI_CHANNEL` or
`~clausters_jupyter.carrier.SERVER_CHANNEL`. The tag rides as the message's
``ch`` field; the OSC packet itself rides as a binary buffer, never
base64-encoded into JSON.

**Sends are marshalled onto the kernel's loop.** A routine sends from the
clock's own thread, which does not own the kernel's sockets. Tornado's
``IOLoop.add_callback`` is thread-safe and is what the kernel's own machinery
uses, so a send from anywhere else is queued there and returns immediately. The
pace is the routine's yields; nothing blocks and nothing is pumped.
"""

import pathlib
import threading

import anywidget
import traitlets

from . import assets
from .carrier import GUI_CHANNEL, SERVER_CHANNEL

__all__ = ["ClaustersWidget"]

_STATIC = pathlib.Path(__file__).parent / "static"


class ClaustersWidget(anywidget.AnyWidget):
    """One canvas in one output area, and the byte bridge behind it.

    Args:
        engine: whether the in-page audio engine is wanted (``backend="page"``)
            — it is a megabyte and a half the native backend does not need.
        session: this notebook's id, which is what keeps two notebooks open in
            one JupyterLab tab from sharing a host (and therefore an id space).
        server_url: the ``--ws`` URL of a **native** server the host should open
            its own audio leg to; ``""`` for none. Mutually exclusive with
            ``engine`` in practice: each backend gives the host one server, and
            the host has one leg.
        width, height: the canvas' logical size. The host resizes itself to the
            element box afterwards, so these are the first frame's size rather
            than a constraint.
    """

    _esm = _STATIC / "widget.js"

    #: Whether to load the in-page engine. Read by the front end on mount.
    engine = traitlets.Bool(False).tag(sync=True)
    #: A native server's WebSocket URL for the host's audio leg, or ``""``.
    server_url = traitlets.Unicode("").tag(sync=True)
    #: Which notebook this cell belongs to — one id per
    #: `clausters_jupyter.bridge.Bridge`, so one per kernel. The front end keys
    #: its wasm host and its engine by it, because JupyterLab is a single-page
    #: application: every notebook open in that tab shares one ``globalThis``
    #: while allocating node and widget ids from the same base.
    session = traitlets.Unicode("").tag(sync=True)
    #: The canvas' logical size; the front end observes the element after
    #: mount and tells the host the real one.
    width = traitlets.Int(480).tag(sync=True)
    height = traitlets.Int(420).tag(sync=True)

    def __init__(self, *, engine: bool = True, server_url: str = "",
                 session: str = "", width: int = 480, height: int = 420,
                 **kwargs):
        super().__init__(engine=engine, server_url=server_url, session=session,
                         width=width, height=height, **kwargs)
        #: The `clausters_jupyter.bridge.Bridge` that made this widget: where
        #: inbound packets go and where the replay comes from. Set by the
        #: bridge itself, so a widget is never half-wired.
        self.bridge = None
        self._kernel_thread = threading.current_thread()
        self.on_msg(self._on_msg)
        # Start tracking views. `_view_count` is ipywidgets' own answer to "is
        # anything showing this?" -- the front end increments it when a view is
        # displayed and decrements it when one is removed, and `None`, the
        # default, means it is not tracked at all. It is the state a comm
        # cannot carry: a comm outlives every view on it, so without this the
        # kernel goes on sending into a cell whose output was cleared, and the
        # front end drops it with no `render` at the other end. (Marked
        # experimental upstream since ipywidgets 7; `jupyter_rfb` keeps a
        # synced Bool of its own for the same question. Either way the answer
        # is *state*, not a message of ours.)
        self._view_count = 0

    # ---- the bridge's end ----

    def send_packet(self, channel: str, payload: bytes):
        """Queue one OSC packet for this cell. Safe from any thread.

        Not ``send``: `ipywidgets.Widget.send` is the raw comm send and this
        must not shadow it.
        """
        self._post({"ch": channel}, [payload])

    @traitlets.observe("_view_count")
    def _views_changed(self, change):
        """The last view of this cell went away: the cell was re-run, its
        output cleared, the notebook closed.

        Only the fall to zero is acted on, and only from above it. A front end
        that never increments the count (one whose views do not emit
        ``displayed``) leaves it at zero forever, and reading that as "gone"
        would silence every notebook under it -- so the transition is what
        counts, never the value alone.
        """
        if self.bridge is None:
            return
        if change["new"] == 0 and (change["old"] or 0) > 0:
            self.bridge.widget_gone(self)

    # ---- the front end ----

    def _on_msg(self, _widget, content, buffers):
        """A message from the page: either a mount announcement or packets."""
        channel = content.get("ch")
        if channel == "ready":
            self._on_ready(content.get("have"))
            return
        if self.bridge is not None:
            for buffer in buffers:
                self.bridge.inbound(channel, bytes(buffer))

    def _on_ready(self, have):
        """The front end mounted (first render, a reloaded page, a moved
        output). Hand it the assets unless the page already has these ones,
        then replay the window it shows onto it.

        ``have`` is the digest of what the *page* has staged, or ``None``. The
        page reports rather than being told, and the comparison is by digest
        rather than by version, so a rebuilt ``dist/`` in a source checkout is
        never mistaken for the one a notebook opened earlier is holding.

        The assets are shared across notebooks even though the wasm host is
        not: same bytes, and they are the expensive part.

        This is also the first moment this cell can *receive* anything — the
        comm is open from construction, but the handler at the other end is
        registered by the front end's ``render`` — so it is what tells the
        bridge the cell is listening, and what lets the audio queue drain.
        """
        payload = assets.bundle(engine=self.engine)
        if have != assets.digest(payload):
            names = list(payload)
            self._post({"ch": "assets", "names": names,
                        "digest": assets.digest(payload)},
                       [payload[name] for name in names])
        if self.bridge is not None:
            for packet in self.bridge.replay_for(self):
                self.send_packet(GUI_CHANNEL, packet)
            self.bridge.widget_ready(self)

    def _post(self, content: dict, buffers: list):
        """`AnyWidget.send`, from whichever thread called."""
        if threading.current_thread() is self._kernel_thread:
            self.send_message(content, buffers)
            return
        loop = _kernel_loop()
        if loop is None:                      # no kernel (a test, a plain REPL)
            self.send_message(content, buffers)
        else:
            loop.add_callback(self.send_message, content, buffers)

    def send_message(self, content: dict, buffers: list):
        """The raw comm send, separated so `_post` can defer it onto the
        kernel's loop and so a test can watch it without a kernel."""
        self.send(content, buffers=buffers)


def _kernel_loop():
    """The kernel's tornado ``IOLoop``, or ``None`` outside a kernel.

    ``add_callback`` is thread-safe, which is the whole reason to reach for it:
    it is how a send from the clock thread gets onto the thread that owns the
    kernel's sockets.
    """
    try:
        from IPython import get_ipython
    except ImportError:
        return None
    shell = get_ipython()
    kernel = getattr(shell, "kernel", None)
    return getattr(kernel, "io_loop", None)


#: Channels the widget itself understands, for the front end's reference.
CHANNELS = (GUI_CHANNEL, SERVER_CHANNEL)
