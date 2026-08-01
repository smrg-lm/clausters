"""C5 leftover: a fuller score-parity golden. The seq layer must render exactly
like the hand-rolled OSC a careful author would write — both go through the same
server engine (the offline render), so identical output proves the
event/pattern/timing layer emits the right score, end to end."""

import pytest

from clausters import render
from clausters.base import OscNrtInterface, TempoClock
from clausters.base import _osclib as osc
from clausters.defs import Server
from clausters.seq import Pbind, Pseq

FREQS = [262.0, 330.0, 392.0, 523.0]


def _embed_or_skip():
    try:
        from clausters import _native
        _native.lib()
    except OSError as e:
        pytest.skip(f"clausters-ffi not built: {e}")


def _handrolled_score(freqs, dur=0.5, amp=0.2, legato=0.8) -> bytes:
    """The same notes a Pbind would play, written out as raw OSC bundles —
    `/synth_new` at the beat, then a gate release after the sustain (dur*legato):
    the built-in ``default`` carries a gated envelope, so the player closes its
    gate (`/node_set gate 0`) rather than freeing the node. Node ids from 1000 like
    the client's allocator."""
    sustain = dur * legato
    bundles = []
    for i, freq in enumerate(freqs):
        node = 1000 + i
        start = i * dur
        bundles.append(osc.score_bundle(
            start, osc.message("/synth_new", "default", node, 1, 0, "freq", freq, "amp", amp)))
        bundles.append(osc.score_bundle(
            start + sustain, osc.message("/node_set", node, "gate", 0.0)))
    return osc.score(*bundles)


def test_pbind_render_matches_handrolled_osc():
    _embed_or_skip()

    server = Server(interface=OscNrtInterface())
    clock = TempoClock(tempo=1.0)
    Pbind(instrument="default", freq=Pseq(FREQS), dur=0.5, amp=0.2).play(clock, server)
    clock.render()

    try:
        _st0 = render(server.interface.score.bytes())
        hi, hi_frames = _st0.samples, _st0.frames
        _st1 = render(_handrolled_score(FREQS))
        lo, lo_frames = _st1.samples, _st1.frames
    except (OSError, RuntimeError, AttributeError) as e:
        pytest.skip(f"embed library not built/usable: {e}")

    # identical render: the high-level seq path emits exactly the hand-rolled OSC
    assert hi_frames == lo_frames
    assert list(hi) == list(lo)

    # stable golden numbers: last gate release at beat 1.9 (1.5 + dur*legato)
    assert hi_frames == round(1.9 * 48_000)
    assert max(abs(s) for s in hi) > 0.0


if __name__ == "__main__":
    import traceback

    for name, fn in sorted(globals().items()):
        if name.startswith("test_") and callable(fn):
            try:
                fn()
                print(f"ok   {name}")
            except BaseException as e:  # noqa: BLE001
                kind = type(e).__name__
                skip = kind in ("Skipped", "OutcomeException")
                print(f"{'skip' if skip else 'FAIL'} {name}: {e}")
                if not skip:
                    traceback.print_exc()
