"""Responders: OscFunc / MidiFunc (port of ``sc3/base/responders.py``).

The **input** half of the client. Until now the client was output-only — it
built OSC/MIDI and sent it to the server. Responders add the receive path and
the client's role as a general MIDI/OSC hub, mirroring sclang's
``OSCFunc``/``MIDIFunc``: receive OSC and MIDI from *any* application, match and
dispatch to a callback, and let that callback emit OSC/MIDI onward — to the
Clausters server or to other apps.

A responder registers a self-filtering handler with a **receiver** (the
transport + demux thread): `clausters.base.OscReceiver` for OSC,
`clausters.base.MidiReceiver` for MIDI. Pass one explicitly, or rely on the
lazily-created module defaults (`default_osc_receiver` / `default_midi_receiver`
— opt-in convenience, the one bit of process-wide state here, in the spirit of
``main.default_clock``).

Callbacks run on the receiver's thread (or, if the receiver has a clock, on the
clock thread): keep them quick and non-blocking — the golden rule. To *sequence*
in response to an event, schedule a routine on a clock (non-blocking) instead of
looping inside the callback.

```python
from clausters.responders import OscFunc, MidiFunc

# Relay an incoming /play to the server as a /s_new.
OscFunc(lambda msg, t, src: server.synth("default", {"freq": msg[1]}), "/play")

# Drive the server from a MIDI keyboard: note on -> /s_new, note off -> free.
notes = {}
def on(m, src):
    notes[m["note"]] = server.synth("default", {"freq": 440 * 2 ** ((m["note"] - 69) / 12)})
MidiFunc(on, "note_on")
```
"""

from .base._oscinterface import OscReceiver
from .base._midiinterface import MidiReceiver

__all__ = [
    "OscFunc",
    "MidiFunc",
    "oscfunc",
    "midifunc",
    "default_osc_receiver",
    "default_midi_receiver",
    "set_default_osc_receiver",
    "set_default_midi_receiver",
]


# ---- module-default receivers (opt-in convenience) ----

_default_osc = None
_default_midi = None


def default_osc_receiver() -> OscReceiver:
    """The lazily-created, started default `OscReceiver` (ephemeral UDP port).
    Created on first use so importing this module opens no socket. Override it
    with `set_default_osc_receiver` (e.g. to bind a fixed port external apps can
    target, or to attach a clock)."""
    global _default_osc
    if _default_osc is None:
        _default_osc = OscReceiver().start()
    return _default_osc


def set_default_osc_receiver(receiver: OscReceiver) -> OscReceiver:
    """Install ``receiver`` as the module default returned by
    `default_osc_receiver`. Start it yourself first."""
    global _default_osc
    _default_osc = receiver
    return receiver


def default_midi_receiver() -> MidiReceiver:
    """The lazily-created, started default `MidiReceiver` (a virtual input
    port). Created on first use, so importing this module opens no MIDI port and
    needs no ``live``-built ``clausters-midi``. Override with
    `set_default_midi_receiver`."""
    global _default_midi
    if _default_midi is None:
        _default_midi = MidiReceiver().start()
    return _default_midi


def set_default_midi_receiver(receiver: MidiReceiver) -> MidiReceiver:
    """Install ``receiver`` as the module default returned by
    `default_midi_receiver`. Start it yourself first."""
    global _default_midi
    _default_midi = receiver
    return receiver


def _matches_template(template, value) -> bool:
    """One ``arg_template`` slot vs an incoming value: a callable is a predicate,
    ``None`` matches anything, else it is compared for equality."""
    if callable(template):
        return bool(template(value))
    return template is None or template == value


# ---- OSC ----


class OscFunc:
    """Responder for incoming OSC messages.

    Registers ``func`` to fire when a message matching ``path`` arrives. The
    callback is called ``func(msg, time, src)`` — ``msg`` the message as a list
    ``[addr, arg1, …]``, ``time`` the bundle's Unix time (or ``None`` for an
    immediate / bare message), ``src`` the ``(host, port)`` of the sender.

    Args:
        func: the callback ``func(msg, time, src)``.
        path: the OSC address to match (a leading ``/`` is added if missing).
        src: optional ``(host, port)`` — respond only to that sender. A port of
            ``None`` matches any port from that host.
        arg_template: optional list matched against the message arguments by
            position; an entry is a literal (compared equal), a predicate
            callable, or ``None`` (matches anything). Shorter than the message
            is fine — only the listed positions are checked.
        recv: the `clausters.base.OscReceiver` to register with; defaults to
            `default_osc_receiver`.

    The responder is enabled on creation. Call `free` (or `disable`) when done.
    """

    def __init__(self, func, path, *, src=None, arg_template=None, recv=None):
        self.func = func
        self.path = path if path.startswith("/") else "/" + path
        self.src = src
        self.arg_template = arg_template
        self.recv = recv if recv is not None else default_osc_receiver()
        self.enabled = False
        self.enable()

    def _handler(self, addr, args, time, src):
        if addr != self.path:
            return
        if self.src is not None:
            host, port = self.src
            if src[0] != host or (port is not None and src[1] != port):
                return
        if self.arg_template is not None:
            for tmpl, val in zip(self.arg_template, args):
                if not _matches_template(tmpl, val):
                    return
        self.func([addr, *args], time, src)

    def enable(self):
        """Start responding (registers the handler with the receiver)."""
        if not self.enabled:
            self.recv.add(self._handler)
            self.enabled = True
        return self

    def disable(self):
        """Stop responding without discarding the object (re-`enable`-able)."""
        if self.enabled:
            self.recv.remove(self._handler)
            self.enabled = False
        return self

    def free(self):
        """Disable permanently; call when finished with this responder."""
        self.disable()

    def one_shot(self):
        """Free the responder after its first match — a one-time action."""
        inner = self.func

        def once(msg, time, src):
            self.free()
            inner(msg, time, src)

        self.func = once
        return self

    def __repr__(self):
        return f"OscFunc({self.path!r}, src={self.src}, arg_template={self.arg_template})"


# ---- MIDI ----


class MidiFunc:
    """Responder for incoming MIDI messages.

    Registers ``func`` to fire on channel-voice messages of a given type. The
    callback is called ``func(message, src)`` — ``message`` a dict
    (``{'type', 'channel', …}``, see `clausters.base._midiinterface.parse_midi`)
    and ``src`` the port name.

    Args:
        func: the callback ``func(message, src)``.
        midi_msg: a message type (``'note_on'``) or a list of types
            (``['note_on', 'note_off']``).
        chan: optional channel (0..15) to restrict to.
        arg_template: optional ``{field: matcher}`` dict matched against the
            message fields; a matcher is a literal, a predicate callable, or
            ``None`` (matches anything).
        recv: the `clausters.base.MidiReceiver` to register with; defaults to
            `default_midi_receiver`.

    Enabled on creation; `free` (or `disable`) when done.
    """

    def __init__(self, func, midi_msg, *, chan=None, arg_template=None, recv=None):
        self.func = func
        self.types = [midi_msg] if isinstance(midi_msg, str) else list(midi_msg)
        self.chan = chan
        self.arg_template = arg_template
        self.recv = recv if recv is not None else default_midi_receiver()
        self.enabled = False
        self.enable()

    def _handler(self, message, src):
        if message["type"] not in self.types:
            return
        if self.chan is not None and message["channel"] != self.chan:
            return
        if self.arg_template is not None:
            for field, tmpl in self.arg_template.items():
                if not _matches_template(tmpl, message.get(field)):
                    return
        self.func(message, src)

    def enable(self):
        """Start responding (registers the handler with the receiver)."""
        if not self.enabled:
            self.recv.add(self._handler)
            self.enabled = True
        return self

    def disable(self):
        """Stop responding without discarding the object (re-`enable`-able)."""
        if self.enabled:
            self.recv.remove(self._handler)
            self.enabled = False
        return self

    def free(self):
        """Disable permanently; call when finished with this responder."""
        self.disable()

    def one_shot(self):
        """Free the responder after its first match — a one-time action."""
        inner = self.func

        def once(message, src):
            self.free()
            inner(message, src)

        self.func = once
        return self

    def __repr__(self):
        return f"MidiFunc({self.types!r}, chan={self.chan}, arg_template={self.arg_template})"


# ---- decorator syntax ----


def oscfunc(path, **kwargs):
    """Decorator building an `OscFunc` over a callback.

    ```python
    @oscfunc("/play")
    def resp(msg, time, src):
        print(msg, time, src)
    ```
    """
    if not isinstance(path, str):
        raise TypeError("oscfunc needs the OSC address path as a string")
    return lambda func: OscFunc(func, path, **kwargs)


def midifunc(midi_msg, **kwargs):
    """Decorator building a `MidiFunc` over a callback.

    ```python
    @midifunc(["note_on", "note_off"])
    def resp(message, src):
        print(message, src)
    ```
    """
    if not isinstance(midi_msg, (str, list)):
        raise TypeError("midifunc needs a MIDI type string or list of them")
    return lambda func: MidiFunc(func, midi_msg, **kwargs)
