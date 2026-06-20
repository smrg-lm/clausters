"""MIDI destinations and interfaces (port of ``sc3/base/_midiinterface.py``).

The same RT/NRT seam as the OSC side, for MIDI (M17). A :class:`MidiServer` is
the double-dispatch counterpart of the OSC ``Server``: a clock + routine plays
the *same* ``Pbind`` through it, and which **interface** it holds decides the
realization — :class:`MidiNrtInterface` accumulates a :class:`MidiScore` (in
beats) that writes a `.mid`/clip file offline, :class:`MidiRtInterface` sends
the notes out a virtual OS port live, through the ``clausters-midi`` crate.

A *MIDI message* is raw status/data bytes (``bytes`` or an iterable of ints).
MIDI carries no timetags: timing comes from the clock at emit time.
"""

from .main import main


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
    :class:`MidiScore` to write offline."""

    is_realtime = False

    def __init__(self):
        self.score = MidiScore()

    def emit(self, beat, message):
        self.score.add(beat, message)

    def close(self):
        pass


class MidiRtInterface:
    """Real-time MIDI output (M17 sub-part 2): a virtual OS MIDI port via the
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
    """A MIDI destination for event patterns (M17) — the double-dispatch
    counterpart of the OSC :class:`~clausters.defs.server.Server`. A
    :class:`~clausters.seq.pattern.Pbind` played on a clock with this as the
    destination realizes each :class:`~clausters.seq.event.Event` as a note
    on/off pair, handed to the held interface (NRT score or live port). Note
    number from `event.midinote()`, velocity from `amp` (0..1 → 0..127)."""

    def __init__(self, interface=None, channel: int = 0, ppq: int = 480):
        self.interface = interface if interface is not None else MidiNrtInterface()
        self.channel = channel & 0x0F
        self.ppq = ppq

    @property
    def score(self):
        """The accumulated :class:`MidiScore` (NRT interface only)."""
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
