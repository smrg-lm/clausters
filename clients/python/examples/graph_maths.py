#!/usr/bin/env python3
"""Maths on a UGen graph: the full operator set, not just ``+ - * /``.

Runs from the *installed* package, offline, like ``offline_render.py``::

    python -m venv .venv && . .venv/bin/activate
    pip install ./clients/python
    python clients/python/examples/graph_maths.py out.wav

Every operator and math method beyond `+ - * /` — ``%``, ``min``/``max``, the
comparisons, ``.midicps()``, ``.distort()``, ``.clip2()`` … — composes the
server's generic ``BinaryOpUGen``/``UnaryOpUGen``, computed by the same
``clausters-core`` code the client uses off the RT path (so a value you compute
ahead of time and the UGen on the audio thread agree bit-for-bit). The point of
interest is the `SynthDef`: it does real per-sample maths, no Faust needed.
"""

import sys

from clausters import Session
from clausters.base import Routine
from clausters.defs import SynthDef, control, out, sine
from clausters.defs import Synth

SR = 48000.0


def maths_lead(name: str = "maths_lead") -> SynthDef:
    """A lead whose pitch, timbre and tremolo are all built with graph maths."""
    note = control("note", 60.0)
    amp = control("amp", 0.3)

    freq = note.midicps()                       # UnaryOpUGen: MIDI note -> Hz
    vib = sine(5.0) * (freq * 0.01)          # 1% vibrato (Mul, then +)
    tone = sine(freq + vib)

    shaped = tone.distort()                      # UnaryOpUGen: soft saturation
    # A unipolar LFO clipped to 0.8 -> a gentle tremolo (>=, clip2 both compose).
    trem = (sine(3.0) * 0.5 + 0.5).clip2(0.8)

    sig = shaped * trem * amp
    return SynthDef(name, out(0.0, sig), out(1.0, sig))


def main():
    out_path = next((a for a in sys.argv[1:] if not a.startswith("-")), "graph_maths.wav")

    session = Session.nrt(tempo=2.0)
    maths_lead().send(session.server)    # freq/timbre are all in the def
    lead = Synth.new("maths_lead", {"note": 48.0, "amp": 0.3}, server=session.server)

    def sequence():
        # Set MIDI notes directly — the def turns them into Hz with .midicps().
        for midi in [48, 52, 55, 59, 60, 59, 55, 52]:
            session.server.send_bundle(("/n_set", lead.id, "note", float(midi)))
            yield 0.5
        yield 1.0
        session.server.send_bundle(("/n_free", lead.id))

    Routine(sequence).play(session.clock)
    stats = session.render(sample_rate=SR, channels=2, path=out_path)

    peak = max(stats.peak, default=0.0)
    print(f"rendered {stats.frames} frames ({stats.duration:.2f} s) | peak {peak:.3f}")

    print(f"wrote {out_path} - listen with: pw-play {out_path}")


if __name__ == "__main__":
    try:
        main()
    except (OSError, RuntimeError) as e:
        sys.exit(str(e))
