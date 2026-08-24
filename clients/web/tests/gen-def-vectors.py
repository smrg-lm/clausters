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
    DoneAction, Env, chans, control, conv, dbrown, dbufrd, demand, dgeom,
    dibrown, disk_in, disk_out, diwhite, drand, dseq, dseries, dshuf, dstutter,
    dswitch1, dup, duty, dwhite, dxrand, env_gen, fft, ifft, impulse, lpf,
    madd, mid_side, mix, out, pan2, pan_az, partconv_frames, pv_add,
    pv_bin_shift, pv_brick_wall, pv_copy_phase, pv_kernel, pv_mag_above,
    pv_mag_below, pv_mag_clip, pv_mag_freeze, pv_mag_mul, pv_mag_shift,
    pv_mag_smear, pv_max, pv_min, pv_mul, rotate2, saw, send_trig, sine,
    stereo_width, svf, svf_morph, tduty, white_noise,
)
from clausters.defs.pv_expr import (  # noqa: E402
    bin_index, binfreq, mag, nbins, param, phase,
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

    # The range maps over a signal: an LFO onto a frequency, a bend with a
    # clip that is not the default, and a bound that is itself a signal.
    lfo = sine(0.2).at_rate("kr")
    cases.append((
        "range_maps",
        SynthDef(
            "maps",
            out(0.0, sine(lfo.linexp(-1.0, 1.0, 200.0, 8000.0))
                * sine(0.7).lincurve(-1.0, 1.0, 0.0, 0.5, -4.0, clip="none")
                * sine(3.0).linlin(-1.0, 1.0, 0.0, sine(0.1))),
        ).spec(),
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

    # ---- the full catalogue: one case per family the port filled out ----

    # Every demand *source*, nested the way the family is meant to be: a
    # sequence whose items are themselves streams, drained rather than taken
    # once. The `dr` rate rides on each of them.
    steps = dseq([
        dseries(3, 60.0, 2.0),
        dgeom(2, 220.0, 1.5),
        dwhite(1, 100.0, 200.0),
        diwhite(1, 48.0, 72.0),
        dbrown(1, 0.0, 1.0, 0.1),
        dibrown(1, 0, 12, 1),
    ], 2.0)
    cases.append((
        "demand_sources",
        SynthDef(
            "sources",
            out(0.0, sine(demand(impulse(8.0), 0.0, steps))),
        ).spec(),
    ))

    # The pickers, the stutter, the buffer read and both drivers, including
    # `tduty`'s extra field.
    picked = dswitch1(
        dxrand([0.0, 1.0, 2.0], 0.0),
        dshuf([110.0, 220.0, 330.0], 1.0),
        dstutter(2.0, drand([440.0, 550.0])),
        dbufrd(control("buf", 0.0, rate="ir"), dseries(0, 0.0, 1.0)),
    )
    cases.append((
        "demand_drivers",
        SynthDef(
            "drivers",
            out(
                0.0,
                sine(duty(dseq([0.25, 0.5], 0.0), 0.0, picked, DoneAction.NONE))
                * tduty(0.5, 0.0, 0.2, DoneAction.NONE, 1.0),
            ),
        ).spec(),
    ))

    # The frequency-domain chain: `fft` carries the static fields, every `pv_*`
    # transforms in place, `ifft` closes it. Two chains, so the combiners have
    # a B side.
    a = fft(white_noise(), fft_size=512, hop=0.25, wintype=1)
    b = fft(saw(110.0), fft_size=512, hop=0.25, wintype=1)
    a = pv_mag_above(a, 3.0)
    a = pv_mag_below(a, 200.0)
    a = pv_mag_clip(a, 50.0)
    a = pv_brick_wall(a, 0.4)
    a = pv_mag_smear(a, 2.0)
    a = pv_mag_freeze(a, control("freeze", 0.0))
    a = pv_bin_shift(a, 1.5, 2.0)
    a = pv_mag_shift(a, 0.5, -1.0)
    b = pv_mul(b, pv_add(pv_min(a, b), pv_max(a, b)))
    b = pv_copy_phase(pv_mag_mul(a, b), b)
    cases.append(("spectral_chain", SynthDef("spectral", out(0.0, ifft(b))).spec()))

    # A per-bin program: every term the expression language has, a unary, a
    # comparison and both expressions, so the postfix token lists are compared
    # whole.
    tilt = param(0) * (1.0 + 4.0 * bin_index / nbins)
    cases.append((
        "pv_kernel_expr",
        SynthDef(
            "kernel",
            out(0.0, ifft(pv_kernel(
                fft(white_noise()),
                mag=mag * (mag >= tilt) * (binfreq / 1000.0).sqrt(),
                phase=phase + param(1),
                params=[control("thresh", 2.0), control("spin", 0.0)],
            ))),
        ).spec(),
    ))

    # Partitioned convolution, whose two static fields size the instance.
    cases.append((
        "convolution",
        SynthDef(
            "conv",
            out(0.0, conv(saw(110.0), control("kernel", 0.0, rate="ir"),
                          fft_size=512, partitions=8)),
        ).spec(),
    ))

    # The stereo field: the three matrices and the ring, each of which builds
    # one UGen per channel with the index as its last input.
    left, right = mid_side(sine(220.0), saw(110.0))
    turned = rotate2(left, right, 0.25)
    widened = stereo_width(turned[0], turned[1], 1.5)
    cases.append((
        "stereo_field",
        SynthDef("field", out(0.0, widened)).spec(),
    ))
    cases.append((
        "ring_pan",
        SynthDef("ring", out(0.0, pan_az(4, white_noise(), 0.3, 0.5, 3.0, 0.0))).spec(),
    ))

    # The state-variable filter, once with the tap gains given directly and
    # once swept by `svf_morph` — with a signal position, whose clamps are
    # graph nodes, and with a constant one, whose clamps fold to numbers.
    pos = control("morph", 0.0)
    cases.append((
        "svf_taps",
        SynthDef(
            "taps",
            out(0.0, svf(saw(110.0), 800.0, 0.3, 1.0, -0.5, 1.0)),
        ).spec(),
    ))
    cases.append((
        "svf_sweep",
        SynthDef(
            "sweep",
            out(0.0, svf(saw(110.0), 800.0, 0.3, *svf_morph(pos))
                + svf(saw(55.0), 400.0, 0.3, *svf_morph(0.5))),
        ).spec(),
    ))

    # Streaming disk I/O: two static fields each, and a def whose root is the
    # recorder's pass-through.
    cases.append((
        "disk_io",
        SynthDef(
            "disk",
            out(0.0, disk_out("/tmp/take.wav",
                              disk_in("/tmp/loop.wav", 0.0, True) * 0.5,
                              "float")),
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


def scalar_cases():
    """The catalogue's one plain-number helper, which sizes a buffer rather
    than building a graph — so it is frozen as values, not as a spec."""
    return [
        {"name": "partconv_frames", "args": list(args),
         "value": partconv_frames(*args)}
        for args in [(1, 1024), (512, 1024), (513, 1024), (44100, 512)]
    ]


def main():
    vectors = {
        "synthdefs": [{"name": n, "spec": s} for n, s in synth_cases()],
        "faustdefs": [{"name": n, "payload": p} for n, p in faust_cases()],
        "graphdefs": [{"name": n, "spec": s} for n, s in graph_case()],
        "scalars": scalar_cases(),
    }
    out_path = pathlib.Path(__file__).with_name("def-vectors.json")
    out_path.write_text(json.dumps(vectors, ensure_ascii=False, indent=2) + "\n")
    total = sum(len(v) for v in vectors.values())
    print(f"wrote {out_path.name}: {total} vectors")


if __name__ == "__main__":
    main()
