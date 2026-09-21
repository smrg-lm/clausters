#!/usr/bin/env python3
"""Generate seq-vectors.json from the Python client's reference sequencing layer.

The Python client is the reference; this script freezes what a **curve** emits
-- the flat ``/buffer_gen "env"`` argument list an envelope fills a buffer with,
the break-point round trip, and the span a curve covers -- so the TS side can
assert it emits the same in `tests/seq-parity.test.ts`.

The two sides are written independently and only the emitted values are
compared, which is the same contract the def and GuiDef vectors keep: the wire
is shared, the language surface is not.

The JSON is committed; regenerate with:

    python3 gen-seq-vectors.py

(from clients/web/tests/, with the Python client importable -- the repo's
.venv has it installed editable).
"""

import json
import pathlib
import sys

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[2] / "python"))

from clausters.defs.ugens import Bpf, Env, env_gen_args  # noqa: E402


def curve_cases():
    """(name, curve source, the values a client must reproduce) per case."""
    cases = []

    # A curve drawn in the bpf widget's own form: absolute times, real control
    # units, a shape per segment.
    # Shapes are the wire's numbers here, as a "points" event carries them:
    # 1 linear, 2 exponential, 5 the custom curvature `curve` then names.
    points = [(0.0, 200.0, 1, 0.0), (2.0, 4000.0, 2, 0.0), (3.0, 800.0, 5, -4.0)]
    flat = [x for p in points for x in p]
    curve = Bpf(points)
    cases.append({
        "name": "drawn_curve",
        "points": flat,
        "env_args": env_gen_args(curve),
        "to_points": curve.to_points(),
        "duration": curve.duration(),
    })

    # The same curve written with the shape *names* an Env takes, which is the
    # other spelling of the same points and must resolve to the same wire.
    named = Bpf([(0.0, 200.0, "lin"), (2.0, 4000.0, "exp"), (3.0, 800.0, -4.0)])
    cases.append({
        "name": "named_shapes",
        "points": named.to_points(),
        "env_args": env_gen_args(named),
        "to_points": named.to_points(),
        "duration": named.duration(),
    })

    # A curve whose first break-point is late: the drawn delay is a leading
    # hold segment, so what was drawn and what plays stay identical.
    delayed = [1.0, 0.0, 1, 0.0, 3.0, 1.0, 1, 0.0]
    curve = Bpf(delayed)
    cases.append({
        "name": "leading_delay",
        "points": delayed,
        "env_args": env_gen_args(curve),
        "to_points": curve.to_points(),
        "duration": curve.duration(),
    })

    # An Env built directly rather than drawn -- the same datum in the other
    # basis, and the sustain has to survive the trip both ways.
    env = Env.adsr(0.01, 0.2, 0.6, 0.4)
    curve = Bpf.from_env(env)
    cases.append({
        "name": "adsr_env",
        "points": None,
        "env_args": env_gen_args(env),
        "to_points": curve.to_points(),
        "duration": curve.duration(),
        "release_node": curve.release_node,
    })

    return cases


def main():
    vectors = {"curves": curve_cases()}
    out_path = pathlib.Path(__file__).with_name("seq-vectors.json")
    out_path.write_text(json.dumps(vectors, ensure_ascii=False, indent=2) + "\n")
    print(f"wrote {out_path.name}: {len(vectors['curves'])} curves")


if __name__ == "__main__":
    main()
