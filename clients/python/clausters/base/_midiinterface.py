"""MIDI destination interfaces (port of ``sc3/base/_midiinterface.py``).

The same RT/NRT split as the OSC interfaces, for MIDI. It is the parallel half
of the destination-swap design: a clock + routine can target a live MIDI port
or a :class:`MidiScore` for offline rendering. C2 ships the structure; the live
backend (``MidiRtInterface``) needs an external library (e.g. python-rtmidi),
which is not a dependency here, so it is a documented stub. NRT accumulation is
functional.

A *MIDI message* is raw status/data bytes (``bytes`` or an iterable of ints).
"""

from .main import main


class MidiInterface:
    time_mode = "unix"

    def send(self, target, when, message):
        raise NotImplementedError(f"{type(self).__name__}.send")


class MidiRtInterface(MidiInterface):
    """Real-time MIDI output — stub. Needs a backend (python-rtmidi or similar)
    that this package does not depend on."""

    time_mode = "unix"

    def __init__(self, *args, **kwargs):
        raise NotImplementedError(
            "MIDI RT output needs a backend (e.g. python-rtmidi), not a "
            "dependency here (clients/PLAN.md: MidiRtInterface stub)"
        )


class MidiScore:
    """Accumulated MIDI events ordered by **beat** (M17). Beats are
    clock-agnostic; the PPQ chosen at write time maps them to file ticks."""

    def __init__(self):
        self.events = []  # (beat, bytes)

    def add(self, beat, message):
        self.events.append((float(beat), bytes(message)))

    def sorted(self):
        # Stable sort keeps same-beat order (a note-off before a re-trigger).
        return sorted(self.events, key=lambda e: e[0])

    def to_smf(self, ppq: int) -> bytes:
        """Standard MIDI File bytes via the `clausters-midi` crate. Beats become
        ticks at `ppq` ticks per quarter note."""
        from .. import _midi

        events = [(round(beat * ppq), msg) for beat, msg in self.sorted()]
        return _midi.write_smf(events, ppq)


class MidiNrtInterface(MidiInterface):
    """Non-real-time MIDI: accumulate events into a :class:`MidiScore`."""

    time_mode = "score"

    def __init__(self):
        self.score = MidiScore()

    def send(self, target, when, message):
        self.score.add(when, message)


class MidiServer:
    """A MIDI destination for event patterns (M17 client sub-part 1) — the
    double-dispatch counterpart of the OSC :class:`~clausters.defs.server.Server`.
    A :class:`~clausters.seq.pattern.Pbind` played on a clock with this as the
    destination realizes each :class:`~clausters.seq.event.Event` as a note
    on/off pair into a :class:`MidiScore` (in beats); :meth:`write` serializes
    it to a `.mid` through the `clausters-midi` crate. Timing comes from the
    clock at emit time — MIDI carries no timetags."""

    def __init__(self, channel: int = 0, ppq: int = 480):
        self.channel = channel & 0x0F
        self.ppq = ppq
        self.score = MidiScore()

    def play_event(self, event):
        """Record a note on at the routine's logical beat and a matching note
        off after the event's sustain. Note number from `event.midinote()`,
        velocity from `amp` (0..1 → 0..127)."""
        if event.get("type") == "rest":
            return None
        beat = getattr(main.current_tt, "_logical_beat", 0.0) or 0.0
        note = int(round(event.midinote())) & 0x7F
        amp = max(0.0, min(1.0, float(event.get("amp", 0.0))))
        velocity = int(round(amp * 127)) & 0x7F
        status = 0x90 | self.channel
        self.score.add(beat, bytes((status, note, velocity)))
        self.score.add(beat + event.sustain(), bytes((0x80 | self.channel, note, 0)))
        return None

    def write(self, path, ppq: int | None = None):
        """Write the accumulated score as a Standard MIDI File at `path`."""
        data = self.score.to_smf(ppq if ppq is not None else self.ppq)
        with open(path, "wb") as f:
            f.write(data)
        return path
