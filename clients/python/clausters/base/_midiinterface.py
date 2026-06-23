"""MIDI destinations and interfaces (port of ``sc3/base/_midiinterface.py``).

The same RT/NRT seam as the OSC side, for MIDI. A `MidiServer` is
the double-dispatch counterpart of the OSC ``Server``: a clock + routine plays
the *same* ``Pbind`` through it, and which **interface** it holds decides the
realization — `MidiNrtInterface` accumulates a `MidiScore` (in
beats) that writes a `.mid`/clip file offline, `MidiRtInterface` sends
the notes out a virtual OS port live, through the ``clausters-midi`` crate.

A *MIDI message* is raw status/data bytes (``bytes`` or an iterable of ints).
MIDI carries no timetags: timing comes from the clock at emit time.

The **input** side — a virtual port other apps/devices route into, decoded into
message dicts and demuxed to `clausters.responders.MidiFunc` responders — lives
in `MidiReceiver` at the bottom, the MIDI counterpart of
`clausters.base._oscinterface.OscReceiver`.
"""

import threading

from .main import main


# Channel-voice status nibbles -> (message type name, data-field names). A
# parsed message is a dict ``{'type', 'channel', <fields…>}`` in the style of
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
    'channel', …}``), or ``None`` for a non-channel-voice / malformed message.

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
        """MIDI 2.0 Clip File (SMF2CLIP) bytes — note velocities at 16-bit
        resolution — via the `clausters-midi` crate."""
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
    message is sent at its beat — the current one now, future ones (the note
    off) scheduled on the clock — best-effort, no timetags."""

    is_realtime = True

    def __init__(self, port: str = "clausters"):
        from .. import _midi

        self._midi = _midi
        self._handle = _midi.output_open(port)
        self.port = port

    def emit(self, beat, message):
        tt = main.current_tt
        now = getattr(tt, "_logical_beat", None)
        clock = getattr(tt, "clock", None)
        if now is not None and clock is not None and beat > now + 1e-9:
            msg = bytes(message)
            clock.sched_abs(beat, lambda: self._send(msg))
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
    """A MIDI destination for event patterns — the double-dispatch
    counterpart of the OSC `Server`. A
    `Pbind` played on a clock with this as the
    destination realizes each `Event` as a note
    on/off pair, handed to the held interface (NRT score or live port). Note
    number from `event.midinote()`, velocity from `amp` (0..1 → 0..127)."""

    def __init__(self, interface=None, channel: int = 0, ppq: int = 480):
        self.interface = interface if interface is not None else MidiNrtInterface()
        self.channel = channel & 0x0F
        self.ppq = ppq

    @property
    def score(self):
        """The accumulated `MidiScore` (NRT interface only)."""
        return getattr(self.interface, "score", None)

    def play_event(self, event):
        if event.get("type") == "rest":
            return None
        beat = getattr(main.current_tt, "_logical_beat", 0.0) or 0.0
        note = int(round(event.midinote())) & 0x7F
        amp = max(0.0, min(1.0, float(event.get("amp", 0.0))))
        velocity = int(round(amp * 127)) & 0x7F
        ch = self.channel
        self.interface.emit(beat, bytes((0x90 | ch, note, velocity)))
        self.interface.emit(beat + event.sustain(), bytes((0x80 | ch, note, 0)))
        return None

    def send_message(self, message):
        """Emit a raw MIDI message at the running routine's logical beat — the
        MIDI counterpart of ``Server.send_bundle`` for a raw OSC message, used by
        `clausters.seq.timeline.MidiEvent`."""
        beat = getattr(main.current_tt, "_logical_beat", 0.0) or 0.0
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
    """A virtual MIDI **input** port that demuxes to registered handlers — the
    MIDI counterpart of `clausters.base._oscinterface.OscReceiver`, and the
    transport under `clausters.responders.MidiFunc`.

    It opens a virtual port through the ``clausters-midi`` crate's ``live``
    feature (midir / ALSA seq on Linux) that other apps and devices route into,
    runs a background thread that polls the crate for raw messages, decodes each
    with `parse_midi`, and calls every registered handler with ``(message,
    src)`` — ``message`` a dict (``{'type', 'channel', …}``), ``src`` the port
    name. Same dispatch threading as `OscReceiver`: inline on the poll thread by
    default, or via ``clock.sched`` when a ``clock`` is given. The golden rule
    holds — a handler must not block its thread.
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
