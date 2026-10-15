"""A `Voices` node played by events, end to end in an offline render: a note
whose instrument is a node is one of its voices, and a voice's joined line
is one held voice of it."""

from clausters import Event, Session, Voices
from clausters.seq import pbind, pseq, pvoice
from clausters.base.stream import Routine
from clausters import play

SR = 48_000.0


def _take(build):
    session = Session.nrt()
    with session:
        build()
    return session.render(sample_rate=SR, channels=1).samples


def _rms(samples):
    return (sum(x * x for x in samples) / max(1, len(samples))) ** 0.5


def test_a_pattern_plays_the_voices_of_a_node():
    def build():
        keys = Voices("default", 4)
        pbind(instrument=keys, midinote=pseq([69, 72, 76]), dur=0.25, amp=0.3).play()

    samples = _take(build)
    assert _rms(samples[: int(0.7 * SR)]) > 1e-2, "the node's voices sound"


def test_an_event_on_a_node_sounds_and_ends():
    def build():
        keys = Voices("default", 2)

        def body():
            Event(instrument=keys, midinote=69, amp=0.4, dur=0.2, legato=1.0).play()
            yield 1.5
        play(Routine(body))

    samples = _take(build)
    assert _rms(samples[: int(0.2 * SR)]) > 1e-2
    assert _rms(samples[int(1.2 * SR): int(1.5 * SR)]) < 1e-4, "its note-off released it"


def test_a_voice_line_on_a_node_is_one_held_voice():
    def build():
        keys = Voices("default", 1)
        pvoice(keys, midinote=pseq([69, 71, 72]), dur=0.25, amp=0.3).play()

    samples = _take(build)
    quarter = int(0.25 * SR)
    # One voice held across the joins: no note re-attacks from silence, so no
    # quarter of the line is quiet.
    assert min(_rms(samples[i * quarter:(i + 1) * quarter]) for i in range(3)) > 1e-2
