#!/usr/bin/env python3
"""Self-contained offline render with the *installed* package.

Unlike the examples under the repo-root ``examples/`` (which insert
``clients/python`` onto ``sys.path``), these ship with the wheel and import
``clausters`` straight from the installed package -- the bundled embed cdylib is
found automatically, so this runs from anywhere in a venv with no server, no
audio device and no ``target/`` directory in sight::

    python -m venv .venv && . .venv/bin/activate
    pip install ./clients/python            # or: pip install clausters-*.whl
    python clients/python/examples/offline_render.py out.wav

It renders a short arpeggio through the embedded NRT renderer and writes a WAV.
Because the synthesis (native ``Sine``) and the offline render both run inside
the bundled libraries, the result is bit-identical to the live server's.
"""

import sys

from clausters import Session
from clausters.seq import Pbind, Pseq, Pwhite

SR = 48000.0


def phrase() -> Pbind:
    """A one-bar arpeggio. ``degree`` walks a major scale (Event maps it to a
    midinote then a frequency in the shared native core); ``amp`` jitters."""
    return Pbind(
        degree=Pseq([0, 2, 4, 7, 4, 2], repeats=2),
        dur=0.25,
        amp=Pwhite(0.1, 0.2),
    )


def main():
    out = next((a for a in sys.argv[1:] if not a.startswith("-")), "offline_render.wav")

    # An NRT session: same Pbind API as live, but the clock drives a score the
    # bundled embed renderer turns into samples. No server, no audio device.
    # The session is its own random context, so its seed reproduces every random
    # draw (Pwhite here) end to end -- independently of any other session.
    session = Session.nrt(tempo=2.0)
    session.seed(1)
    session.play(phrase())
    # The server's own seed is a separate one: it starts the render's stochastic
    # UGens, and with none given the render draws a fresh one -- a score with
    # noise in it is a new take every run. `stats.seed` is how you get a take
    # back: pass it as `seed=` and the render repeats sample for sample.
    stats = session.render(sample_rate=SR, channels=2, path=out)

    peak = max(stats.peak)
    print(f"rendered {stats.frames} frames ({stats.duration:.2f} s) | peak {peak:.3f}")
    print(f"seed {stats.seed} - pass seed={stats.seed} to render this take again")

    print(f"wrote {out} - listen with: pw-play {out}")


if __name__ == "__main__":
    try:
        main()
    except (OSError, RuntimeError) as e:
        sys.exit(str(e))
