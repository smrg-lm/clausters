#!/usr/bin/env python3
"""Typed controls: a `tr` trigger, a lagged control and an `ir` scalar.

Runs from the *installed* package, offline, like ``offline_render.py``::

    python -m venv .venv && . .venv/bin/activate
    pip install ./clients/python
    python clients/python/examples/typed_controls.py out.wav

The point of interest is the SynthDef and the control **types**:

- ``freq`` carries a ``lag`` (0.12 s), so when the routine sets a new note the
  pitch **glides** to it instead of jumping — a portamento lead.
- ``gate`` is a trigger (``rate="tr"``): a ``/n_set gate 1`` holds for one block
  and the server resets it, so each set **re-plucks** the percussive envelope.
  A plain ``kr`` gate would stay 1 and never re-trigger.
- ``detune`` is drawn once with ``rand`` (an ``ir`` scalar): a small random
  offset frozen for the synth's life — re-run the render and it redraws.

One persistent synth is driven by a `Routine` that sets ``freq``/``gate`` per
note; the lag and the trigger only make sense on a synth that outlives its
notes, which is exactly what a routine (not one ``/s_new`` per note) gives.
"""

import sys

from clausters import Session
from clausters.base import Routine
from clausters.defs import (
    DoneAction,
    Env,
    SynthDef,
    control,
    env_gen,
    out,
    rand,
    sine,
)

SR = 48000.0


def glide_lead(name: str = "glide_lead") -> SynthDef:
    """A sine lead: ``freq`` glides (lag), ``gate`` re-triggers a pluck (tr),
    ``detune`` is a fixed random offset (ir)."""
    freq = control("freq", 220.0, lag=0.12)          # portamento
    gate = control("gate", 0.0, rate="tr")           # one-block re-trigger
    amp = control("amp", 0.2)
    detune = rand(-4.0, 4.0)                          # ir: drawn once, held
    env = env_gen(Env.perc(attack=0.005, release=0.2), gate=gate,
                  done_action=DoneAction.NONE)
    sig = sine(freq + detune) * env * amp
    return SynthDef(name, out(0.0, sig), out(1.0, sig))


def mtof(midi: float) -> float:
    return 440.0 * 2.0 ** ((midi - 69.0) / 12.0)


def main():
    out_path = next((a for a in sys.argv[1:] if not a.startswith("-")), "typed_controls.wav")

    session = Session.nrt(tempo=2.0)
    session.server.add_synthdef(glide_lead())        # /d_recv at time 0

    lead = session.server.synth("glide_lead", {"amp": 0.2, "freq": mtof(48)})

    def sequence():
        # A little melody; consecutive notes glide, each one re-plucked. In an
        # NRT score, control changes must be *timetagged* to spread over time, so
        # they go out as `send_bundle` (which stamps them with the routine's
        # logical beat) rather than the immediate `set`/`free`, which would all
        # collapse onto time 0 and free the synth before it ever sounds.
        for midi in [48, 55, 60, 63, 60, 55, 51, 48]:
            session.server.send_bundle(
                ("/n_set", lead.id, "freq", mtof(midi), "gate", 1.0))
            yield 0.5                                 # beats between notes
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
