#!/usr/bin/env python3
"""The box API: Faust library DSP glued together from Python.

Runs from the *installed* package, offline, like ``offline_render.py``::

    python -m venv .venv && . .venv/bin/activate
    pip install ./clients/python
    python clients/python/examples/boxes_library.py out.wav

``clausters.defs.boxes`` reuses the Faust libraries **without transcribing
them**: ``box.faust(...)`` compiles any Faust expression into a ``Box`` that
composes like a primitive. This example builds an instrument out of three
library pieces — an oscillator (``os.osc``), a lowpass (``fi.lowpass``) and a
stereo reverb (``re.stereo_freeverb``) — wired to sliders and arithmetic
built in Python, then renders a short phrase offline and writes a WAV.

The two application stages at work, kept apart in the syntax:

- arguments to ``box.faust`` are **evaluation-stage**, spliced into the Faust
  source text (the filter order ``3``, the reverb's structural parameters);
- arguments to *calling* the resulting box are **composition-stage**, boxes
  wired to its signal inputs (the sliders, the previous stage).
"""

import struct
import sys
import wave

from clausters import Session
from clausters.base import Routine
from clausters.defs import FaustDef
from clausters.defs import boxes as box

SR = 48000.0


def soft_voice(name: str = "soft_voice") -> FaustDef:
    """osc -> lowpass -> stereo reverb, all from the Faust libraries."""
    freq = box.hslider("freq", 220.0, 20.0, 2000.0, 0.1)
    cutoff = box.hslider("cutoff", 900.0, 50.0, 8000.0, 1.0)
    amp = box.hslider("amp", 0.2, 0.0, 1.0, 0.001)

    # box.faust("os.osc") is the unapplied oscillator: one input (the
    # frequency), one output. Calling it wires the slider in; the result is
    # an ordinary Box, so `* amp` composes arithmetic around it.
    tone = box.faust("os.osc", ins=1, outs=1)(freq) * amp

    # fi.lowpass(3): the order is structural, so it is an eval-arg (spliced
    # into the source); the cutoff and the signal are its two inputs.
    dry = box.faust("fi.lowpass", 3, ins=2, outs=1)(cutoff, tone)

    # The reverb's feedback/damp/spread bake into the source too. It takes a
    # stereo pair: reusing the `dry` VALUE twice is fine (and deliberate) --
    # a repeated subexpression is computed once, this is not two filters.
    wet = box.faust("re.stereo_freeverb", 0.80, 0.70, 0.55, 23,
                    ins=2, outs=2)(dry, dry)

    # Channel selection needs the arity, which only the Faust compiler knows
    # for fragments -- hence outs=2 above. A gentle dry/wet per side (the
    # reverb has gain; keep the sum comfortably below full scale).
    left, right = wet.outs()
    return FaustDef.from_box(name, box.par(dry * 0.5 + left * 0.15,
                                           dry * 0.5 + right * 0.15))


def main():
    out_path = next((a for a in sys.argv[1:] if not a.startswith("-")), "boxes_library.wav")

    session = Session.nrt(tempo=2.0)
    session.server.add_faustdef(soft_voice())   # NRT: scored at time 0
    voice = session.server.synth("soft_voice", {"freq": 220.0, "amp": 0.25})

    def sequence():
        # The fragment sliders are ordinary controls: /n_set by label.
        for step, midi in enumerate([57, 60, 64, 67, 64, 60, 57, 52]):
            hz = 440.0 * 2.0 ** ((midi - 69) / 12.0)
            session.server.send_bundle(("/n_set", voice.id, "freq", hz,
                                        "cutoff", 600.0 + 400.0 * step))
            yield 0.5
        yield 2.0                                # let the reverb tail ring
        session.server.send_bundle(("/n_free", voice.id))

    Routine(sequence).play(session.clock)
    samples, frames = session.render(sample_rate=SR, channels=2)

    peak = max((abs(s) for s in samples), default=0.0)
    print(f"rendered {frames} frames ({frames / SR:.2f} s) | peak {peak:.3f}")

    with wave.open(out_path, "wb") as w:
        w.setnchannels(2)
        w.setsampwidth(2)
        w.setframerate(int(SR))
        w.writeframes(b"".join(
            struct.pack("<h", int(max(-1.0, min(1.0, s)) * 32767)) for s in samples
        ))
    print(f"wrote {out_path} - listen with: pw-play {out_path}")


if __name__ == "__main__":
    try:
        main()
    except (OSError, RuntimeError) as e:
        sys.exit(str(e))
