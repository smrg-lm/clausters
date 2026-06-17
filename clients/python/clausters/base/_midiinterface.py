"""MIDI destination interfaces (port of ``sc3/base/_midiinterface.py``).

The same RT/NRT split as the OSC interfaces, for MIDI. It is the parallel half
of the destination-swap design: a clock + routine can target a live MIDI port
or a :class:`MidiScore` for offline rendering. C2 ships the structure; the live
backend (``MidiRtInterface``) needs an external library (e.g. python-rtmidi),
which is not a dependency here, so it is a documented stub. NRT accumulation is
functional.

A *MIDI message* is raw status/data bytes (``bytes`` or an iterable of ints).
"""


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
    """Accumulated MIDI events, ordered by time."""

    def __init__(self):
        self.events = []  # (time_seconds, bytes)

    def add(self, time_seconds, message):
        self.events.append((time_seconds, bytes(message)))

    def sorted(self):
        return sorted(self.events, key=lambda e: e[0])


class MidiNrtInterface(MidiInterface):
    """Non-real-time MIDI: accumulate events into a :class:`MidiScore`."""

    time_mode = "score"

    def __init__(self):
        self.score = MidiScore()

    def send(self, target, when, message):
        self.score.add(when, message)
