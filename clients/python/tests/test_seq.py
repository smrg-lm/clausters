"""C5 tests: patterns, events, and the event stream — including the headline
that a Pbind plays through the seam (NRT score -> render) with **yield-exact**
timing."""

import struct

import pytest

from clausters.base import TempoClock, OscNrtInterface
from clausters.base import _osclib as osc
from clausters.base.builtins import midicps
from clausters.defs import Server
from clausters.seq import Event, Pbind, Pn, Pgeom, Pser, Pseq, Pseries, Pwhite, rest


def _embed_or_skip():
    try:
        from clausters import _native
        _native.lib()
    except OSError as e:
        pytest.skip(f"clausters-ffi not built: {e}")


# ---- value patterns ----

def test_value_patterns():
    assert list(Pseq([1, 2, 3], 2)) == [1, 2, 3, 1, 2, 3]
    assert list(Pser([1, 2, 3], 4)) == [1, 2, 3, 1]
    assert list(Pseries(0, 2, 4)) == [0, 2, 4, 6]
    assert list(Pgeom(1, 2, 4)) == [1, 2, 4, 8]
    assert list(Pn(Pseq([0, 1]), 2)) == [0, 1, 0, 1]
    # nesting: a sub-pattern is embedded in place
    assert list(Pseq([0, Pseq([1, 2]), 3])) == [0, 1, 2, 3]
    xs = list(Pwhite(0.0, 1.0, 100))
    assert len(xs) == 100 and all(0.0 <= x <= 1.0 for x in xs)


# ---- the random context (one seedable source for a whole script) ----

def test_random_patterns_replay_under_the_root_seed():
    _embed_or_skip()
    from clausters.base.main import main
    from clausters.seq import Prand

    # No per-pattern seeds: seeding the root context reproduces every draw,
    # in order, across pattern kinds.
    main.seed(11)
    a = list(Pwhite(0.0, 1.0, 8)) + list(Prand([1, 2, 3], 8))
    main.seed(11)
    b = list(Pwhite(0.0, 1.0, 8)) + list(Prand([1, 2, 3], 8))
    assert a == b
    main.seed(12)
    assert list(Pwhite(0.0, 1.0, 8)) != a[:8]


def test_rand_functions_draw_from_the_same_context():
    _embed_or_skip()
    from clausters import choice, next_below, next_f64, uniform
    from clausters.base.main import main

    main.seed(3)
    a = [next_f64(), uniform(2.0, 4.0), next_below(10), choice("abcd")]
    main.seed(3)
    assert a == [next_f64(), uniform(2.0, 4.0), next_below(10), choice("abcd")]
    assert 2.0 <= a[1] < 4.0 and 0 <= a[2] < 10 and a[3] in "abcd"


def test_routines_get_their_own_streams_reproducible_per_routine():
    _embed_or_skip()
    from clausters.base import Routine
    from clausters.base.main import main
    from clausters.base.rand import uniform

    def collect(out, n):
        def gen():
            for _ in range(n):
                out.append(uniform(0.0, 1.0))
                yield 0.25
        return gen

    def run_pair(swap):
        """Two routines on one clock; ``swap`` flips their scheduling order
        (and interleave), which must not change either routine's values."""
        main.seed(42)
        a_out, b_out = [], []
        ra = Routine(collect(a_out, 4))    # creation order fixes each stream:
        rb = Routine(collect(b_out, 4))    # ra then rb, derived from the root
        clock = TempoClock(tempo=4.0)
        if swap:
            clock.sched_abs(0.125, rb)     # rb wakes first and interleaves
            clock.sched_abs(0.25, ra)
        else:
            clock.sched_abs(0.0, ra)
            clock.sched_abs(1.0, rb)
        clock.render()
        return a_out, b_out

    a1, b1 = run_pair(swap=False)
    a2, b2 = run_pair(swap=True)
    # Per-routine reproducibility: the wake interleaving changed, the values
    # of each routine did not (each has its own derived stream).
    assert a1 == a2 and b1 == b2
    assert a1 != b1                        # ...and the two streams differ


# ---- events ----

def test_event_defaults_and_derived():
    e = Event(freq=440.0, dur=2.0, amp=0.3)
    assert e["instrument"] == "default" and e["legato"] == 0.8
    assert e.freq() == 440.0
    assert e.delta() == 2.0                     # dur * stretch
    assert e.sustain() == pytest.approx(1.6)    # dur * legato * stretch


def test_explicit_delta_and_sustain_override_the_calculation():
    # SuperCollider semantics: an explicit key wins over dur*stretch / dur*legato.
    e = Event(dur=0.5, delta=2.0, sustain=0.4)
    assert e.delta() == 2.0
    assert e.sustain() == 0.4


def test_event_pitch_from_midinote_and_degree():
    assert Event(midinote=69).freq() == pytest.approx(midicps(69))
    # degree 0 on the default major scale at octave 5 -> midinote 60
    assert Event(degree=0).freq() == pytest.approx(midicps(60.0))


def test_event_and_pbind_accept_a_dict_like_kwargs():
    # The dict form of the constructor is equivalent to kwargs (as in Synth's
    # controls), and keywords win when both are given.
    assert Event({"freq": 440.0, "amp": 0.3}) == Event(freq=440.0, amp=0.3)
    assert Event({"freq": 440.0}, freq=220.0)["freq"] == 220.0

    d = Pbind({"instrument": "default", "freq": Pseq([100.0, 200.0]), "dur": 0.5})
    k = Pbind(instrument="default", freq=Pseq([100.0, 200.0]), dur=0.5)
    assert list(d) == list(k)
    merged = Pbind({"dur": 0.5}, dur=0.25, freq=Pseq([100.0]))
    assert list(merged)[0]["dur"] == 0.25


def test_pbind_yields_events_and_stops_with_finite_key():
    p = Pbind(instrument="default", freq=Pseq([100.0, 200.0]), dur=0.5)
    events = list(p)
    assert len(events) == 2                      # stops when freq runs out
    assert all(isinstance(e, Event) for e in events)
    assert events[0]["freq"] == 100.0 and events[0]["dur"] == 0.5


# ---- the seam + yield-exact timing ----

def _inner_addr(raw: bytes) -> str:
    # raw = "#bundle\0"(8) + timetag(8) + [i32 len][message]
    length = struct.unpack(">i", raw[16:20])[0]
    addr, _ = osc.decode(raw[20:20 + length])
    return addr


def test_pbind_timing_is_yield_exact_in_nrt():
    _embed_or_skip()
    server = Server(interface=OscNrtInterface())
    clock = TempoClock(tempo=1.0)               # 1 beat = 1 second
    Pbind(instrument="default", freq=Pseq([262.0, 330.0, 392.0, 523.0]),
          dur=0.5, amp=0.2).play(clock, server)
    clock.render()

    starts = sorted(when for when, raw in server.interface.score.bundles
                    if _inner_addr(raw) == "/s_new")
    # four notes, exactly 0.5 s apart — no wall-clock jitter
    assert starts == [0.0, 0.5, 1.0, 1.5]


def test_pbind_renders_to_audio():
    _embed_or_skip()
    server = Server(interface=OscNrtInterface())
    clock = TempoClock(tempo=2.0)
    Pbind(instrument="default", freq=Pseq([262.0, 330.0, 392.0, 523.0, 659.0]),
          dur=0.5, amp=0.2).play(clock, server)
    clock.render()
    try:
        _st0 = server.render(sample_rate=48_000.0, channels=2)
        samples, frames = _st0.samples, _st0.frames
    except (OSError, RuntimeError, AttributeError) as e:
        pytest.skip(f"embed library not built/usable: {e}")
    assert frames > 0
    assert max(abs(s) for s in samples) > 0.0


if __name__ == "__main__":
    import traceback

    for name, fn in sorted(globals().items()):
        if name.startswith("test_") and callable(fn):
            try:
                fn()
                print(f"ok   {name}")
            except BaseException as e:  # noqa: BLE001 — smoke harness
                kind = type(e).__name__
                skip = kind in ("Skipped", "OutcomeException")
                print(f"{'skip' if skip else 'FAIL'} {name}: {e}")
                if not skip:
                    traceback.print_exc()
