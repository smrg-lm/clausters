#!/usr/bin/env python3
"""Generate seq-vectors.json from the Python client's reference sequencing layer.

The Python client is the reference; this script freezes what its automation
lane emits — the internal control def's spec, the flat ``/buffer_gen "env"``
argument list a curve discretizes into, and the break-point round trip — so the
TS side can assert it emits the same in `tests/seq-parity.test.ts`.

The two sides are written independently and only the emitted values are
compared, which is the same contract the def and GuiDef vectors keep: the wire
is shared, the language surface is not.

The JSON is committed; regenerate with:

    python3 gen-seq-vectors.py

(from clients/web/tests/, with the Python client importable — the repo's
.venv has it installed editable).
"""

import json
import pathlib
import sys

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[2] / "python"))

from clausters.defs.ugens import Env  # noqa: E402
from clausters.seq.automation import (  # noqa: E402
    Automation, LANE_DEF, auto_lane_def, _env_gen_args,
)


def automation_cases():
    """(name, curve source, the values a client must reproduce) per case."""
    cases = []

    # A curve drawn in the bpf widget's own form: absolute times, real control
    # units, a shape per segment.
    # Shapes are the wire's numbers here, as a "points" event carries them:
    # 1 linear, 2 exponential, 5 the custom curvature `curve` then names.
    points = [(0.0, 200.0, 1, 0.0), (2.0, 4000.0, 2, 0.0), (3.0, 800.0, 5, -4.0)]
    flat = [x for p in points for x in p]
    auto = Automation.from_points(points, None, name="cutoff")
    cases.append({
        "name": "drawn_curve",
        "points": flat,
        "env_args": _env_gen_args(auto.env),
        "to_points": auto.to_points(),
        "duration": auto.duration(),
    })

    # A curve whose first break-point is late: the drawn delay is a leading
    # hold segment, so what was drawn and what plays stay identical.
    delayed = [1.0, 0.0, 1, 0.0, 3.0, 1.0, 1, 0.0]
    auto = Automation.from_points(delayed, None)
    cases.append({
        "name": "leading_delay",
        "points": delayed,
        "env_args": _env_gen_args(auto.env),
        "to_points": auto.to_points(),
        "duration": auto.duration(),
    })

    # An Env built directly rather than drawn — the same object the widget
    # round-trips through.
    auto = Automation(Env.adsr(0.01, 0.2, 0.6, 0.4), None, name="amp")
    cases.append({
        "name": "adsr_env",
        "points": None,
        "env_args": _env_gen_args(auto.env),
        "to_points": auto.to_points(),
        "duration": auto.duration(),
    })

    return cases


def main():
    vectors = {
        "lane_def": {"name": LANE_DEF, "spec": auto_lane_def().spec()},
        "automations": automation_cases(),
    }
    out_path = pathlib.Path(__file__).with_name("seq-vectors.json")
    out_path.write_text(json.dumps(vectors, ensure_ascii=False, indent=2) + "\n")
    print(f"wrote {out_path.name}: 1 def + {len(vectors['automations'])} curves")


if __name__ == "__main__":
    main()
