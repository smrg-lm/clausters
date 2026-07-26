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

import struct
import sys
import wave

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
    samples, frames = session.render(sample_rate=SR, channels=2)

    peak = max(abs(s) for s in samples)
    print(f"rendered {frames} frames ({frames / SR:.2f} s) | peak {peak:.3f}")

    with wave.open(out, "wb") as w:
        w.setnchannels(2)
        w.setsampwidth(2)
        w.setframerate(int(SR))
        w.writeframes(b"".join(
            struct.pack("<h", int(max(-1.0, min(1.0, s)) * 32767)) for s in samples
        ))
    print(f"wrote {out} - listen with: pw-play {out}")


if __name__ == "__main__":
    try:
        main()
    except (OSError, RuntimeError) as e:
        sys.exit(str(e))
