#!/usr/bin/env python3
"""Generate def-vectors.json from the Python client's reference builders.

The Python client is the reference def model; this script freezes the spec
JSON its builders emit for a set of graphs, so the TS builders can assert
they emit the same in `tests/def-parity.test.ts`. Each case names the TS
expression that must reproduce it — the two sides are written independently
and only the emitted spec is compared, which is exactly the contract: the
wire is shared, the language surface is not.

The JSON is committed; regenerate with:

    python3 gen-def-vectors.py

(from clients/web/tests/, with the Python client importable — the repo's
.venv has it installed editable).
"""

import json
import pathlib
import sys

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[2] / "python"))

from clausters.defs import FaustDef, GraphDef, SynthDef  # noqa: E402
from clausters.defs import signals as sig  # noqa: E402
from clausters.defs.ugens import (  # noqa: E402
    DoneAction, Env, chans, control, dup, env_gen, lpf, madd, mix, out, pan2,
    saw, send_trig, sine, white_noise,
)


def synth_cases():
    """(name, spec) for each reference SynthDef."""
    cases = []

    # The smallest graph: one source into one output.
    cases.append(("beep", SynthDef("beep", out(0.0, sine(440.0))).spec()))

    # Controls, a math op, and a stereo output from one channel list.
    freq = control("freq", 440.0)
    amp = control("amp", 0.2)
    cases.append((
        "controls_stereo",
        SynthDef("controls_stereo", out(0.0, dup(sine(freq) * amp))).spec(),
    ))

    # A control reused in two places serializes once and is referenced twice
    # (the dedup the topological walk does).
    shared = control("freq", 200.0)
    cases.append((
        "shared_control",
        SynthDef("shared", out(0.0, sine(shared) * sine(shared * 2.0))).spec(),
    ))

    # Control types and lags.
    gate = control("gate", 1.0, rate="tr")
    cutoff = control("cutoff", 800.0, lag=0.1, lag_down=0.5)
    env = Env.adsr(0.01, 0.2, 0.6, 0.4)
    cases.append((
        "typed_controls_env",
        SynthDef(
            "voice",
            out(0.0, lpf(saw(110.0), cutoff)
                * env_gen(env, gate=gate, done_action=DoneAction.FREE_SELF)),
        ).spec(),
    ))

    # Every generic operator kind: a BinaryOpUGen and a UnaryOpUGen by name.
    cases.append((
        "generic_ops",
        SynthDef("ops", out(0.0, sine(440.0).distort().max(0.1))).spec(),
    ))

    # The fused forms and a mix fold (sum4 + sum3 chunking).
    voices = [sine(110.0 * (n + 1)) for n in range(7)]
    cases.append((
        "mix_fold",
        SynthDef("fold", out(0.0, madd(mix(voices), 0.1, 0.0))).spec(),
    ))

    # Equal-power panning: two Pan2 UGens sharing one source.
    cases.append((
        "pan",
        SynthDef("pan", out(0.0, pan2(white_noise(), 0.3))).spec(),
    ))

    # A side-effect root, with its label and no audio output at all.
    cases.append((
        "side_effect_only",
        SynthDef("watch", send_trig(sine(1.0), 7, 0.5)).spec(),
    ))

    # A rate set explicitly, and a channel list built by hand.
    cases.append((
        "rates_and_chans",
        SynthDef(
            "rates",
            out(0.0, chans(sine(5.0).at_rate("kr"), sine(7.0).at_rate("kr"))),
        ).spec(),
    ))

    return cases


def faust_cases():
    """(name, payload) for each reference FaustDef signal tree."""
    freq = sig.hslider("freq", 440.0, 20.0, 2000.0, 0.01)
    amp = sig.hslider("amp", 0.2, 0.0, 1.0, 0.001)
    phasor = sig.rec(lambda s: (s + freq / sig.sr()) - (s + freq / sig.sr()).floor())
    tone = sig.sin(phasor * (2.0 * sig.PI)) * amp
    return [
        ("faust_tone", json.loads(FaustDef.from_signals("tone", tone).dump_def())),
        (
            "faust_stereo",
            json.loads(
                FaustDef.from_signals("stereo", tone, tone * 0.5).dump_def()
            ),
        ),
    ]


def graph_case():
    g = GraphDef("chain")
    bus = g.bus("mix")
    src = g.add("gsrc", {"out": bus}, level=1.0)
    g.add("gsink", {"in": bus, "out": "OUT"})
    g.add("gvoice", {"out": bus}, voice=True)
    g.port("gain", src["level"].scaled(2.0, 0.1), default=0.5)
    return [("graph_chain", g.spec())]


def main():
    vectors = {
        "synthdefs": [{"name": n, "spec": s} for n, s in synth_cases()],
        "faustdefs": [{"name": n, "payload": p} for n, p in faust_cases()],
        "graphdefs": [{"name": n, "spec": s} for n, s in graph_case()],
    }
    out_path = pathlib.Path(__file__).with_name("def-vectors.json")
    out_path.write_text(json.dumps(vectors, ensure_ascii=False, indent=2) + "\n")
    total = sum(len(v) for v in vectors.values())
    print(f"wrote {out_path.name}: {total} vectors")


if __name__ == "__main__":
    main()
