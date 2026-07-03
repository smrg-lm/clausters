#!/usr/bin/env python3
"""Pausing and resuming a node with ``/n_run`` (S4) -- pause is not terminal.

Runs from the *installed* package, offline, like ``offline_render.py``::

    python -m venv .venv && . .venv/bin/activate
    pip install ./clients/python
    python clients/python/examples/pause_resume.py out.wav

The point of interest is ``Server.pause`` / ``Server.resume`` (the ``/n_run``
command). A paused node stays in the tree and keeps its state, but is skipped
during processing -- silent and free of CPU -- and resumes *exactly* where it
left off. This is what makes ``DoneAction.PAUSE_SELF`` non-terminal: a synth
parked by its envelope can be brought back with ``/n_run 1``.

The render is a steady drone that is paused for one beat and then resumed, so
the WAV has an audible gap of silence in the middle with the tone continuing
unchanged on either side. In an NRT score the toggles must be *timetagged*, so
they go out through ``send_bundle`` (stamped with the routine's logical beat)
rather than the immediate ``pause``/``resume``, which would collapse onto time
0. A live RT session would call ``session.server.pause(node)`` /
``.resume(node)`` directly instead.
"""

import struct
import sys
import wave

from clausters import Session
from clausters.base import Routine
from clausters.defs import SynthDef, control, out, sin_osc

SR = 48000.0


def drone(name: str = "drone") -> SynthDef:
    """A plain sustained sine -- no envelope, so it runs until paused or freed;
    its phase is what we watch survive a pause and resume."""
    freq = control("freq", 220.0)
    amp = control("amp", 0.2)
    sig = sin_osc(freq) * amp
    return SynthDef(name, out(0.0, sig), out(1.0, sig))


def main():
    out_path = next((a for a in sys.argv[1:] if not a.startswith("-")), "pause_resume.wav")

    session = Session.nrt(tempo=2.0)
    session.server.add_synthdef(drone())             # /d_recv at time 0
    node = session.server.synth("drone", {"freq": 220.0, "amp": 0.2})

    def sequence():
        yield 1.0                                    # a beat of tone
        session.server.send_bundle(("/n_run", node.id, 0))   # pause: goes silent
        yield 1.0                                    # a beat of silence
        session.server.send_bundle(("/n_run", node.id, 1))   # resume: tone returns
        yield 1.0                                    # a beat of tone again
        session.server.send_bundle(("/n_free", node.id))

    Routine(sequence).play(session.clock)
    samples, frames = session.render(sample_rate=SR, channels=2)

    # The middle beat is silent, the outer beats are not -- a quick sanity check.
    third = frames // 3
    rms = lambda a: (sum(s * s for s in a) / max(1, len(a))) ** 0.5
    mono = samples[0::2]
    print(f"rendered {frames} frames ({frames / SR:.2f} s)")
    print(f"beat RMS: {rms(mono[:third]):.3f} (on) "
          f"{rms(mono[third:2 * third]):.3f} (paused) "
          f"{rms(mono[2 * third:]):.3f} (resumed)")

    with wave.open(out_path, "wb") as w:
        w.setnchannels(2)
        w.setsampwidth(2)
        w.setframerate(int(SR))
        w.writeframes(b"".join(
            struct.pack("<h", int(max(-1.0, min(1.0, s)) * 32767)) for s in samples
        ))
    print(f"wrote {out_path} - listen with: ffplay -autoexit {out_path}")


if __name__ == "__main__":
    try:
        main()
    except (OSError, RuntimeError) as e:
        sys.exit(str(e))
