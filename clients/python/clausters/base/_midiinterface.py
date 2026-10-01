"""MIDI destinations and interfaces (port of ``sc3/base/_midiinterface.py``).

The same RT/NRT seam as the OSC side, for MIDI. A `MidiServer` is
the double-dispatch counterpart of the OSC ``Server``: a clock + routine plays
the *same* ``Pbind`` through it, and which **interface** it holds decides the
rendering -- `MidiNrtInterface` accumulates a `MidiScore` (in
beats) that writes a `.mid`/clip file offline, `MidiRtInterface` sends
the notes out a virtual OS port live, through the ``clausters-midi`` crate.

A *MIDI message* is raw status/data bytes (``bytes`` or an iterable of ints).
MIDI carries no timetags: timing comes from the clock at emit time.

The **input** side -- a virtual port other apps/devices route into, decoded into
message dicts and demuxed to `clausters.responders.MidiFunc` responders -- lives
in `MidiReceiver` at the bottom, the MIDI counterpart of
`clausters.base._oscinterface.OscReceiver`.
"""

import threading

from .. import _native
from .moment import Moment


# Channel-voice status nibbles -> (message type name, data-field names). A
# parsed message is a dict ``{'type', 'channel', <fields...>}`` in the style of
# mido / sc3's responder layer, so `MidiFunc` matches on ``type``.
_CV_TYPES = {
    0x80: ("note_off", ("note", "velocity")),
    0x90: ("note_on", ("note", "velocity")),
    0xA0: ("polytouch", ("note", "value")),
    0xB0: ("control_change", ("control", "value")),
    0xC0: ("program_change", ("program",)),
    0xD0: ("aftertouch", ("value",)),
    0xE0: ("pitchwheel", ("pitch",)),
}


def parse_midi(message) -> dict | None:
    """Decode raw channel-voice bytes into a message dict (``{'type',
    'channel', ...}``), or ``None`` for a non-channel-voice / malformed message.

    ``pitchwheel`` combines the two 7-bit data bytes into a single 14-bit
    ``pitch`` (0..16383, centre 8192); every other field is a raw 7-bit value.
    """
    b = bytes(message)
    if not b or b[0] < 0x80:
        return None
    kind = _CV_TYPES.get(b[0] & 0xF0)
    if kind is None:
        return None
    name, fields = kind
    d1 = b[1] if len(b) > 1 else 0
    d2 = b[2] if len(b) > 2 else 0
    msg = {"type": name, "channel": b[0] & 0x0F}
    if name == "pitchwheel":
        msg["pitch"] = (d1 & 0x7F) | ((d2 & 0x7F) << 7)
    elif len(fields) == 1:
        msg[fields[0]] = d1
    else:
        msg[fields[0]], msg[fields[1]] = d1, d2
    return msg


class MidiScore:
    """Accumulated MIDI events ordered by **beat**. Beats are clock-agnostic;
    the PPQ chosen at write time maps them to file ticks."""

    def __init__(self):
        self.events = []  # (beat, bytes)

    def add(self, beat, message):
        self.events.append((float(beat), bytes(message)))

    def sorted(self):
        # Stable sort keeps same-beat order (a note-off before a re-trigger).
        return sorted(self.events, key=lambda e: e[0])

    def _ticked(self, ppq):
        return [(round(beat * ppq), msg) for beat, msg in self.sorted()]

    def to_smf(self, ppq: int) -> bytes:
        """Standard MIDI File (`.mid`) bytes via the `clausters-midi` crate."""
        from .. import _midi

        return _midi.write_smf(self._ticked(ppq), ppq)

    def to_clip(self, ppq: int) -> bytes:
        """MIDI 2.0 Clip File (SMF2CLIP) bytes -- note velocities at 16-bit
        resolution -- via the `clausters-midi` crate."""
        from .. import _midi

        return _midi.write_clip(self._ticked(ppq), ppq)


class MidiNrtInterface:
    """Non-real-time MIDI: accumulate ``(beat, message)`` into a
    `MidiScore` to write offline."""

    is_realtime = False

    def __init__(self):
        self.score = MidiScore()

    def emit(self, beat, message):
        self.score.add(beat, message)

    def close(self):
        pass


class MidiRtInterface:
    """Real-time MIDI output: a virtual OS MIDI port via the
    `clausters-midi` crate's `live` feature (midir / ALSA seq on Linux). Each
    message is sent at its beat -- the current one now, future ones (the note
    off) scheduled on the clock -- best-effort, no timetags."""

    is_realtime = True

    def __init__(self, port: str = "clausters"):
        from .. import _midi

        self._midi = _midi
        self._handle = _midi.output_open(port)
        self.port = port

    def emit(self, beat, message):
        now = Moment.current()
        if now.clock is not None and beat > now.beat + 1e-9:
            msg = bytes(message)
            now.clock.sched_abs(beat, lambda: self._send(msg))
        else:
            self._send(message)

    def _send(self, message):
        if self._handle is not None:
            self._midi.output_send(self._handle, message)

    def close(self):
        if self._handle is not None:
            # Stopping the clock leaves any note-off scheduled past the stop
            # beat unsent, which would hang notes on the destination. Send an
            # "all notes off" (CC 123) on every channel before dropping the
            # port -- the standard MIDI panic, so a partial run ends silent.
            for ch in range(16):
                self._send(bytes((0xB0 | ch, 0x7B, 0)))
            self._midi.output_close(self._handle)
            self._handle = None


class MidiServer:
    """A MIDI destination for event patterns -- the double-dispatch
    counterpart of the OSC `Server`. A
    `Pbind` played on a clock with this as the
    destination renders each `Event` as a note
    on/off pair, handed to the held interface (NRT score or live port), and a
    ``"midi"`` event as the message its ``midicmd`` names. Note number from
    `event.midinote()`, velocity from `event.velocity()` -- an explicit
    ``velocity``, else the amplitude's, never 0, which is a note-off -- and the
    channel the event's own ``channel``, else this destination's.

    **MPE.** With ``zone`` (a number of members), the destination is an MPE
    zone: the lower one (master channel 0, members from 1 up) unless
    ``upper``. The RPN that declares it goes out first -- at the head of the
    score, or down the port at once -- and each note goes on a member channel
    of its own (round robin, preferring a free one, reusing the one held
    longest), preceded by its expression: the event's ``bend`` in semitones
    through ``bend_range`` (48, the zone's default), its ``press`` (the
    pressure) and its ``slide`` (the timbre), 0..1 -- the keys a server's zone
    names its voice's controls with, so one pattern plays either end. A
    dimension the event does not state goes back to its rest, so a reused
    channel does not carry the last note's."""

    def __init__(self, interface=None, channel: int = 0, ppq: int = 480,
                 zone: int | None = None, upper: bool = False, bend_range: float = 48.0):
        self.interface = interface if interface is not None else MidiNrtInterface()
        self.channel = channel & 0x0F
        self.ppq = ppq
        self.bend_range = float(bend_range)
        self._assigner = None
        self._held = []  # (off beat, channel, key) of the zone's notes
        if zone is not None:
            from .. import _midi

            self._midi = _midi
            self._assigner = _midi.MpeAssigner(zone, upper)
            for message in _midi.zone_messages(zone, upper):
                self.interface.emit(0.0, message)

    @property
    def score(self):
        """The accumulated `MidiScore` (NRT interface only)."""
        return getattr(self.interface, "score", None)

    def play_event(self, event):
        beat = Moment.current().beat
        keys = event.keys_data()
        # The messages are the core's render: a note's on and off, a "midi"
        # event's one message, nothing for a rest.
        messages = _native.event_midi(keys, self.channel)
        if self._assigner is not None and keys.get("type", "note") == "note":
            messages = self._on_member(beat, keys, messages)
        for at, message in messages:
            self.interface.emit(beat + at, message)
        return None

    def _on_member(self, beat, keys, messages):
        """A note's messages moved onto the member channel the zone assigns it,
        its expression ahead of its note-on."""
        ons = [m for _, m in messages if m[0] & 0xF0 == 0x90 and m[2] > 0]
        if not ons:
            return messages
        key = ons[0][1]
        # The notes that ended by now free their channels.
        for held in [h for h in self._held if h[0] <= beat + 1e-9]:
            self._assigner.note_off(held[1], held[2])
            self._held.remove(held)
        channel = self._assigner.note_on(key)
        if channel is None:
            return messages
        moved = [(at, bytes((m[0] & 0xF0 | channel,)) + m[1:]) for at, m in messages]
        off = max((at for at, m in moved if m[0] & 0xF0 == 0x80), default=0.0)
        self._held.append((beat + off, channel, key))
        expression = self._midi.expression_messages(
            channel, float(keys.get("bend", 0.0)), self.bend_range,
            keys.get("press"), keys.get("slide"))
        return [(0.0, m) for m in expression] + moved

    def send_message(self, message):
        """Emit a raw MIDI message at the running routine's logical beat -- the
        MIDI counterpart of ``Server.send_bundle`` for a raw OSC message."""
        beat = Moment.current().beat
        self.interface.emit(beat, bytes(message))
        return None

    def write(self, path, ppq: int | None = None, fmt: str = "smf"):
        """Write the accumulated score (NRT only) as a `.mid` (`fmt="smf"`) or a
        MIDI 2.0 clip (`fmt="clip"`)."""
        score = self.score
        if score is None:
            raise RuntimeError("write() needs a MidiServer with a MidiNrtInterface")
        ppq = ppq if ppq is not None else self.ppq
        data = score.to_clip(ppq) if fmt == "clip" else score.to_smf(ppq)
        with open(path, "wb") as f:
            f.write(data)
        return path

    def close(self):
        self.interface.close()


class MidiReceiver:
    """A virtual MIDI **input** port that demuxes to registered handlers -- the
    MIDI counterpart of `clausters.base._oscinterface.OscReceiver`, and the
    transport under `clausters.responders.MidiFunc`.

    It opens a virtual port through the ``clausters-midi`` crate's ``live``
    feature (midir / ALSA seq on Linux) that other apps and devices route into,
    runs a background thread that polls the crate for raw messages, decodes each
    with `parse_midi`, and calls every registered handler with ``(message,
    src)`` -- ``message`` a dict (``{'type', 'channel', ...}``), ``src`` the port
    name. Same dispatch threading as `OscReceiver`: inline on the poll thread by
    default, or via ``clock.sched`` when a ``clock`` is given. The golden rule
    holds -- a handler must not block its thread.
    """

    def __init__(self, port: str = "clausters-in", clock=None, poll_interval: float = 0.002):
        self.port = port
        self.clock = clock
        self.poll_interval = poll_interval
        self._handle = None
        self._thread = None
        self._running = False
        self._handlers = []
        self._lock = threading.Lock()

    def start(self):
        if self._running:
            return self
        from .. import _midi

        self._midi = _midi
        self._handle = _midi.input_open(self.port)
        self._running = True
        self._thread = threading.Thread(target=self._loop, name="MidiReceiver", daemon=True)
        self._thread.start()
        return self

    def stop(self):
        self._running = False
        if self._thread is not None:
            self._thread.join(timeout=1.0)
            self._thread = None
        if self._handle is not None:
            self._midi.input_close(self._handle)
            self._handle = None
        return self

    close = stop

    def add(self, handler):
        """Register ``handler(message, src)``; called for every decoded
        channel-voice message. Returns ``handler`` so it can later be
        `remove`d."""
        with self._lock:
            self._handlers.append(handler)
        return handler

    def remove(self, handler):
        with self._lock:
            if handler in self._handlers:
                self._handlers.remove(handler)

    def _loop(self):
        import time

        while self._running:
            drained = False
            while self._running:
                raw = self._midi.input_poll(self._handle)
                if raw is None:
                    break
                drained = True
                msg = parse_midi(raw)
                if msg is not None:
                    self._dispatch(msg)
            if not drained:
                time.sleep(self.poll_interval)

    def _dispatch(self, msg):
        with self._lock:
            handlers = list(self._handlers)
        for handler in handlers:
            if self.clock is not None:
                self.clock.sched(0.0, lambda h=handler: h(msg, self.port))
            else:
                handler(msg, self.port)
