#!/usr/bin/env python3
"""Generate score-vectors.json from the Python client's reference NRT session.

The acceptance W13 is written against is "one piece, one score": a piece
written once emits a score **byte-identical** to the Python client's for the
same input. This script freezes that side of it — the Python client plays a few
small pieces into an offline session and the accumulated score bytes are
recorded; `tests/score-parity.test.ts` writes the same pieces with the TS
client and compares the bytes.

What that actually asserts is the whole offline stack at once: the timetag
packing (a score's epoch is the render's start, not the wall clock), the
ordering rule, the bundle framing, the beat-to-second mapping the clock does,
and every command's argument tagging along the way.

The pieces play the server's **built-in** instrument and send no def, on
purpose. A def's wire payload is JSON *text*, and the two clients' serializers
lay it out differently (Python spaces its separators and writes `440.0` where
`JSON.stringify` writes `440`) — a difference in formatting, not in the def,
and one `tests/def-parity.test.ts` already pins by comparing the parsed spec.
Letting it into these vectors would replace a meaningful byte comparison with
a formatting one.

The pieces are deliberately small and written twice by hand — once here, once
in TypeScript — because a generated pair would only prove the generator
agrees with itself.

The JSON is committed; regenerate with:

    python3 gen-score-vectors.py

(from clients/web/tests/, with the Python client importable — the repo's
.venv has it installed editable).
"""

import json
import pathlib
import sys

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[2] / "python"))

from clausters.base import Routine  # noqa: E402
from clausters.defs import Synth  # noqa: E402
from clausters.seq import Event, Pbind, Pseq  # noqa: E402
from clausters.session import Session  # noqa: E402


def one_synth(session):
    """One instance of the built-in instrument, freed a second later — the
    smallest score there is, and the shape every `render(def)` produces."""
    server = session.server
    node = Synth("default", {"freq": 330.0}, server=server)
    server.send_bundle_after(1.0, ("/node_free", node.id))


def a_routine(session):
    """Three notes a routine yields between: the beats become score seconds
    through the clock's own tempo."""
    server = session.server

    def melody():
        for freq in (440.0, 550.0, 660.0):
            Event(freq=freq, dur=0.5, amp=0.2).play(server)
            yield 0.5

    Routine(melody).play(session.clock)


def a_pattern(session):
    """An event pattern, which is the same thing through the pattern layer —
    and the case where the client's own event defaults (sustain, the release
    message) reach the score."""
    Pbind(degree=Pseq([0, 2, 4]), dur=0.25, amp=0.15).play(
        session.clock, session.server)


CASES = [
    ("one_synth", one_synth, 1.0),
    ("routine", a_routine, 1.0),
    ("pattern", a_pattern, 2.0),
]

#: How many points of a rendered envelope are frozen. The whole render is a
#: second of audio; what a comparison needs is the *shape*, so it is decimated
#: to this many evenly spaced samples.
ENV_POINTS = 64


def env_traces():
    """What `plot(Env)` draws, rendered through the engine's own EnvGen.

    Both clients now render an envelope the same way — a one-node offline
    render, gate-released at the sustain point — so this freezes the drawn
    curve itself: the frame count, the peak, and a decimated trace. It is the
    check that "what you plot is what an EnvGen plays" holds *across* clients
    and not just within one.
    """
    from clausters.defs.ugens import Env
    from clausters.plot import _render_env

    traces = []
    for name, env in (
        ("adsr", Env.adsr()),
        ("perc", Env.perc(0.01, 0.5)),
        ("triangle", Env([0.0, 1.0, 0.0], [0.25, 0.25])),
    ):
        samples, chans, rate, _ = _render_env(env, 48_000.0)
        step = max(1, len(samples) // ENV_POINTS)
        traces.append({
            "name": name,
            "frames": len(samples),
            "channels": chans,
            "sample_rate": rate,
            "trace": [round(float(samples[i]), 6)
                      for i in range(0, len(samples), step)][:ENV_POINTS],
        })
    return traces


def main():
    vectors = []
    for name, build, tempo in CASES:
        session = Session.nrt(tempo=tempo)
        with session._active():
            build(session)
        session.clock.render()
        score = session.server.interface.score.bytes()
        vectors.append({"name": name, "tempo": tempo, "hex": score.hex()})
    payload = {"scores": vectors, "envelopes": env_traces()}
    out_path = pathlib.Path(__file__).with_name("score-vectors.json")
    out_path.write_text(json.dumps(payload, indent=2) + "\n")
    print(f"wrote {out_path.name}: {len(vectors)} scores, "
          f"{len(payload['envelopes'])} envelopes")


if __name__ == "__main__":
    main()
