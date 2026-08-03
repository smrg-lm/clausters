"""`CommInterface`: OSC over a notebook kernel's comm.

The carrier that lets the ordinary Python client drive a host living in a
notebook cell. It is an `clausters.base.OscInterface` like the TCP, UDP and
WebSocket ones, so `clausters.gui.GuiHost` and `clausters.defs.Server` take it
through the same seam and neither learns that a browser is involved: bytes go
in, bytes come back, and everything above — the GuiDef builders, the def
specs, the verbs — is untouched.

Three things make it different from a socket, and each one is a property of
where it runs rather than a design choice:

**The front end is not there yet.** A cell's output renders after its code
finishes, so the packets a cell sends have no recipient while it runs. Outbound
GUI packets therefore go through `clausters_jupyter.journal.Journal`, which
forwards and remembers; a mount replays it. See that module — it is also what
makes a reloaded page rebuild itself.

**Sends can come from any thread.** A routine runs on the clock's own thread
(`clausters.base.clock`), and that thread does not own the kernel's sockets, so
a send from anywhere but the kernel's thread is handed to the kernel's event
loop with ``call_soon_threadsafe`` (the link's job — see
`clausters_jupyter.widget`) and left there. It is a queue, not a pump: nothing
blocks, and the pace is the routine's own yields.

**A reply cannot arrive while a cell runs.** Not slowly — at all. ipykernel
serializes the shell channel::

    # kernelbase.py, on the path of every shell message
    # Whilst executing a shell message, do not accept any other shell messages
    # on the same subshell, so that cells are run sequentially.
    async with asyncio_lock:
        await self.dispatch_shell(msg, subshell_id=subshell_id)

A ``comm_msg`` is a shell message, so it waits behind the running cell's
``execute_request``. Awaiting does not help (the lock is held across the
await), and ipykernel 7 removed ``do_one_iteration``, so there is no loop to
pump either. The only mechanism that would change this is a subshell (JEP 91),
which the *front end* opts into per message and which Colab and VS Code do not
support.

`recv` therefore **refuses instead of hanging**: asked from the cell thread to
*wait*, it raises `RoundTripInCell`, whose message says to split the work
across two cells. Two cases are not that and keep working: a zero timeout,
which polls what already arrived (this is what
`clausters.gui.GuiHost.pump` does, and it is how a notebook reads its widgets
back), and a wait from any other thread — a routine, a responder — since those
do not hold the kernel's lock and their replies arrive whenever it is next
idle.

This only ever bites the in-page backend. With a native server the client talks
to it over an ordinary socket, and the comm carries `/gui_*` alone, whose one
round trip is `clausters.gui.GuiHost.query`.
"""

import queue
import threading

from clausters.base import OscInterface, _osclib

from .journal import Journal

__all__ = ["CommInterface", "RoundTripInCell", "GUI_CHANNEL", "SERVER_CHANNEL"]

#: The two fronts one comm carries. Every payload is tagged with one of these,
#: because a notebook has a single channel to the page and two peers behind it.
GUI_CHANNEL = "gui"
SERVER_CHANNEL = "server"


class RoundTripInCell(RuntimeError):
    """A reply was awaited from the cell that is running.

    Raised by `CommInterface.recv` rather than blocking until the timeout,
    because in a notebook the reply cannot arrive until the cell ends. The way
    through is to split the work: send in one cell, use the answer in the next.
    """


class CommInterface(OscInterface):
    """OSC over the kernel comm of a `clausters_jupyter.widget.ClaustersWidget`.

    Args:
        link: the widget, or anything with ``send_packet(channel, payload)`` and
            ``subscribe(channel, callback)``. Kept as the one thing this class
            knows about the front end.
        channel: `GUI_CHANNEL` or `SERVER_CHANNEL` — which peer in the page
            this interface's packets are for.
        journal: replay journal for outbound packets, or ``None`` for a
            carrier whose traffic is not reconstructible (the audio channel:
            see `clausters_jupyter.journal`).
    """

    time_mode = "unix"
    #: A comm message frames its own payload, so a packet is not bounded by a
    #: datagram and a bulk round trip may use the server's whole frame ceiling.
    stream = True
    #: Nothing may block for a reply here — see `recv` and the module docstring.
    #: What this buys is that `clausters.defs._wire.send_def` skips its
    #: confirmation instead of raising: sending a def is the first thing any
    #: notebook does, the comm keeps order, and the wait was only ever a
    #: confirmation.
    awaitable = False

    def __init__(self, link, channel: str = GUI_CHANNEL, journal=None):
        self.link = link
        self.channel = channel
        self.journal = journal if journal is not None else (
            Journal() if channel == GUI_CHANNEL else None)
        #: Inbound packets, filled from the kernel's thread by `_inbound`.
        self._replies: queue.Queue = queue.Queue()
        #: The thread that runs cell code, learned at construction — this class
        #: is built while a cell is executing, which is the only time a
        #: notebook ever runs user code.
        self._cell_thread = threading.current_thread()
        self._closed = False
        link.subscribe(channel, self._inbound)

    # ---- OscInterface ----

    def boot(self):
        """Start the peer this carrier talks to — what `clausters.defs.Server.boot`
        calls on the instance it is called from.

        The engine is in the browser, so starting it is not launching a process
        but giving it somewhere to run: the page executes nothing until some
        cell has an output, since the front end is served as a widget's module.
        So this puts the engine's own cell on screen (`Bridge.start_audio_cell`)
        — an empty box that draws nothing and only has to exist — and from then
        on a synth sounds when it is created, as it does anywhere else.

        Called twice it does nothing the second time, and it does nothing at all
        on the GUI channel, whose peer is the host rather than the engine.
        """
        if self.channel == GUI_CHANNEL:
            return
        self.link.start_audio_cell()


    def send_msg(self, target, addr, *args):
        self._send(_osclib.message(addr, *args))

    def send_bundle(self, target, when, *messages):
        packets = [_osclib.message(*m) for m in messages]
        self._send(_osclib.bundle_at(when, *packets))

    def recv(self, timeout):
        """One reply packet, or ``None`` on timeout.

        Raises `RoundTripInCell` when a cell asks to *wait* for a reply that
        cannot arrive until it ends — see the module docstring.

        A zero timeout is not that: it is a poll of what has already arrived,
        which is exactly what `clausters.gui.GuiHost.pump` does to dispatch the
        events a previous cell's interactions left waiting. That works, and is
        in fact the ordinary way a notebook reads its widgets back.
        """
        if timeout and threading.current_thread() is self._cell_thread:
            raise RoundTripInCell(
                "a reply cannot arrive while this cell is running: ipykernel "
                "holds the shell channel until the cell ends, so the front "
                "end's answer is queued behind it. Split the work across two "
                "cells - send in one, read the answer in the next - or use "
                "backend='native', whose server is reached over a socket "
                "rather than the comm."
            )
        try:
            return self._replies.get(block=bool(timeout), timeout=timeout or None)
        except queue.Empty:
            return None

    def close(self):
        self._closed = True
        self.link.unsubscribe(self.channel, self._inbound)

    stop = close

    # ---- the front end ----

    def replay(self):
        """The packets that rebuild this channel's state on a fresh front end.
        Called by the widget on mount; empty for an unjournalled channel."""
        return self.journal.replay() if self.journal is not None else []

    def _inbound(self, payload: bytes):
        """One packet from the page. Runs on the kernel's thread."""
        self._replies.put(payload)

    def _send(self, packet: bytes):
        if self._closed:
            raise RuntimeError("send on a closed CommInterface")
        # The route is read *before* recording, because recording is what
        # forgets it: a `/gui_free` drops its whole entry, so asking afterwards
        # returns nobody and the packet that closes a window is the one packet
        # never delivered. Every other address survives either order.
        root = None
        if self.journal is not None:
            root = self.journal.root_of(packet)
            self.journal.record(packet)
        self.link.send_packet(self.channel, packet, root)
