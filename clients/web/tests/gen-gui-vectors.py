#!/usr/bin/env python3
"""Generate gui-vectors.json from the Python client's reference GuiDef builders.

The Python client is the reference GUI client; this script freezes the JSON
its builders emit for a set of trees, so the TS builders can assert they emit
the same in `tests/gui-parity.test.ts`. Each case names the TS expression that
must reproduce it — the two surfaces are written independently (the TS one
takes camelCase options where Python takes snake_case keywords) and only the
emitted document is compared, which is exactly the contract: the wire is
shared, the language surface is not.

Every tree is stored as it goes on the wire — through `to_json`, so the
client-only `name` key is stripped there too, and the comparison covers that.

The JSON is committed; regenerate with:

    python3 gen-gui-vectors.py

(from clients/web/tests/, with the Python client importable — the repo's
.venv has it installed editable).
"""

import json
import pathlib
import sys

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[2] / "python"))

from clausters.defs.ugens import Env  # noqa: E402
from clausters.gui import guidef as g  # noqa: E402


def cases():
    """(name, tree) for each reference GuiDef, as the wire document."""
    out = []

    # A control panel: the light widgets, the place props and a named widget
    # (the name is client-only and must not reach the JSON).
    out.append((
        "panel_controls",
        g.window(
            g.label("clausters", id=1, text_size=3.0, align="center", h=24),
            g.panel(
                g.knob(id=3, label="freq", min=50.0, max=2000.0, value=220.0,
                       name="freq"),
                g.slider(id=4, label="cutoff", min=20.0, max=20000.0,
                         value=800.0, vertical=True),
                g.number(id=5, label="amp", min=0.0, max=1.0, value=0.2),
                id=2, layout="row", gap=10.0,
            ),
            g.panel(
                g.button(id=7, label="ping"),
                g.toggle(id=8, label="gate", value=True),
                g.menu(["sine", "saw", "pulse"], id=9, index=1, label="wave"),
                g.text(id=10, value="/node_set 1000 freq 440", multiline=False),
                id=6, layout="row", h=40,
            ),
            title="panel", w=480, h=420, layout="col", margin=8.0,
        ),
    ))

    # Containers: a scroll workspace configured down to a vertical view, a
    # theme group over a subtree, and a stack driven by an inline-bound menu.
    out.append((
        "containers",
        g.window(
            g.scroll(
                g.panel(id=2, x=0.0, y=0.0, w=200.0, h=1200.0,
                        theme={"panel_fill": "#101018", "accent": "#40c0a0"}),
                id=1, axis="y", zoom=False, content_h=1200.0, view_y=40.0,
                view_zoom=1.0, layout="free",
            ),
            g.menu(["one", "two"], id=3, index=1, bind=["widget", 4, "index"]),
            g.stack(
                g.panel(id=5),
                g.panel(id=6),
                id=4, index=1, margin=4.0,
            ),
            title="workspace", layout="col", theme={"window_fill": "#0d0d12"},
        ),
    ))

    # The heavy views, with their sources and the shared timeline chrome.
    out.append((
        "heavy_views",
        g.window(
            g.waveform(id=1, path="take.f32", channels=2, base_bucket=512,
                       ruler="beats", ruler_y="db", tempo=2.0, beat_at=0.0,
                       quant=4.0, sel_start=1000.0, sel_len=4000.0,
                       playhead_at=48000.0, playhead=32000.0,
                       playhead_loop_start=1000.0, playhead_loop_len=8000.0,
                       y_start=0.25, y_len=0.5, link=7,
                       overlay=True),
            g.spectrogram(id=2, cache="take.stft", window_size=2048, hop=512,
                          sample_rate=48000.0, db_floor=-90.0, db_ceil=0.0,
                          freq_scale="mel", colormap=1, ruler="time",
                          ruler_y="hz", link=7),
            g.plot(id=3, data=[0.0, 0.5, -0.5, 1.0], view="spectrum",
                   fft_size=1024, db_floor=-100.0, freq_scale="log",
                   ruler="samples", ruler_y="off", label="render"),
            title="views", layout="col",
        ),
    ))

    # The live views: the audio server's buses, at both rates.
    out.append((
        "live_views",
        g.window(
            g.meter(10, id=1, rate="control", min=-1.0, max=1.0, label="bus"),
            g.scope(10, id=2, rate="control", min=-1.0, max=1.0),
            g.scope(0, id=3, channels=2, window_ms=20.0, trigger=0.0,
                    overlay=True, ruler=False, ruler_y="off"),
            g.phasescope(0, id=4, window_ms=30.0, hold=False),
            g.spectrum(0, id=5, channels=2, fft_size=2048, db_floor=-100.0,
                       db_ceil=0.0, freq_scale="bark", averaging=0.5,
                       peak_hold=True),
            g.nodetree(id=6, group=0, controls=True),
            title="live", layout="col",
        ),
    ))

    # The envelope editor: both point forms (tuples with a named shape and a
    # numeric curvature, and the flat wire quads), plus the `Env` round trip.
    out.append((
        "bpf_points",
        g.window(
            g.bpf(id=1, points=[(0.0, 0.0), (0.5, 1.0, "exp"), (1.0, 0.0, -4.0)],
                  min=0.0, max=1.0, duration=1.0, exp=False, label="env"),
            g.bpf(id=2, points=g.env_to_points(Env.adsr(0.01, 0.2, 0.6, 0.4))),
            g.bpf(id=3, points=[0.0, 0.0, 1, 0.0, 2.0, 1.0, 8, 0.0]),
            title="envelopes", layout="col",
        ),
    ))

    # The timeline editors: a piano-roll with notes and OSC flags, a piano
    # playing host-managed voices, and two lanes of clips.
    out.append((
        "timeline_editors",
        g.window(
            g.pianoroll(id=1, notes=[(0.0, 4800.0, 60), (4800.0, 4800.0, 67, 90, 1)],
                        osc=[(0.0, "start"), 9600.0], min=48, max=84,
                        snap=1200.0, velocity=True, osc_lane=True,
                        ruler="beats", tempo=2.0, playhead_at=-1.0,
                        playhead=2400.0, playhead_loop_start=0.0,
                        playhead_loop_len=9600.0),
            g.piano(id=2, min=36, max=96, active_min=48, active_max=84,
                    velocity=100, channel=0, voice="piano_voice",
                    voice_args=[("amp", 0.3)], overview=True, pan=True),
            g.track(
                g.clip(id=4, offset=0.0, dur=48000.0, path="take.f32",
                       channels=1, base_bucket=256, label="take"),
                g.clip(id=5, offset=48000.0, dur=24000.0,
                       notes=[(0.0, 12000.0, 64)], min=48.0, max=84.0),
                g.clip(id=6, offset=72000.0, dur=24000.0,
                       points=[(0.0, 0.0), (24000.0, 1.0, "sin")], exp=False),
                id=3, label="drums", height=2.0, snap=1200.0, ruler="time",
                sample_rate=48000.0, playhead_at=0.0, playhead=12000.0,
                playhead_loop_start=0.0, playhead_loop_len=96000.0, link=7,
            ),
            title="arrangement", layout="col",
        ),
    ))

    # The free-standing ruler over a navigation group, the engraved page with
    # its whole cursor track, and the legacy boolean frequency-axis alias.
    out.append((
        "ruler_score_legacy",
        g.window(
            g.timeruler(id=1, h=24.0, link=7, ruler="beats", tempo=2.0,
                        beat_at=0.0, quant=4.0),
            g.score(id=2,
                    display_list={
                        "vb": [100.0, 50.0],
                        "glyphs": {"e0a4": "M0 0 L1 1 Z"},
                        "prims": [{"id": "n1", "kind": "glyph", "cp": "e0a4",
                                   "x": 10.0, "y": 20.0}],
                        "cursors": [{"t": 0.0, "x": 10.0, "y0": 0.0, "y1": 40.0}],
                        "step": 2.5,
                    },
                    playhead=250.0, playhead_at=48000.0,
                    playhead_loop_start=0.0, playhead_loop_len=4000.0,
                    sample_rate=48000.0, selected="n1", editable=True),
            g.spectrogram(id=3, data=[0.0, 0.5, -0.5, 1.0], log_freq=True),
            g.spectrum(0, id=4, log_freq=False, label="legacy"),
            title="ruler and score", layout="col",
        ),
    ))

    # The patcher and a shader canvas.
    out.append((
        "patch_canvas",
        g.window(
            g.patch(
                id=1,
                boxes=[
                    {"def": "gsrc", "inlets": [], "outlets": ["out"],
                     "x": 0.0, "y": 0.0},
                    {"def": "gsink", "inlets": ["in", {"name": "gain",
                                                       "rate": "control"}],
                     "outlets": [], "x": 0.0, "y": 120.0},
                ],
                cords=[0, 0, 1, 0], label="graph",
            ),
            g.canvas("return vec4<f32>(uv, u.params.x, 1.0);", id=2,
                     params=[0.5, 0.0, 0.0, 0.0], buses=[10, -1, -1, -1]),
            title="patch", layout="row",
        ),
    ))

    # The model's own vocabulary, built by the general builders rather than
    # reached through a shortcut: containers by axis count, one signal element
    # saying the point of the product, and the chrome nested on the axes it
    # belongs to.
    out.append((
        "model_vocabulary",
        g.window(
            g.layout(
                g.signal(id=2, view="trace", path="take.f32", channels=2,
                         navigable=True, selectable=True, base_bucket=512,
                         axes={"x": {"unit": "beats", "tempo": 2.0, "link": 1},
                               "y": {"unit": "db", "min": -1.0, "max": 1.0}}),
                g.signal(id=3, view="spectrum", bus=0, rate="audio", channels=2,
                         fft_size=2048, label="live"),
                id=1, flow="row", gap=4.0,
            ),
            g.plane(g.label("placed in content units", id=6, x=0.0, y=0.0),
                    id=5, axis="both", zoom=True, view_zoom=1.0),
            g.field(
                g.field(id=8, offset=0.0, dur=48000.0, label="take"),
                id=7, label="drums", height=2.0,
                axes={"x": {"unit": "time", "sample_rate": 48000.0, "link": 1}},
            ),
            g.field(id=9, h=24.0, axes={"x": {"unit": "beats", "link": 1}}),
            title="the model", flow="col",
        ),
    ))

    # The generic escape hatch: a node type this client does not name.
    out.append((
        "generic_node",
        g.window(
            g.node("gizmo", id=1, spin=2.5, mode="loose", w=64),
            title="generic",
        ),
    ))

    return [(name, json.loads(g.to_json(tree))) for name, tree in out]


def main():
    vectors = [{"name": n, "tree": t} for n, t in cases()]
    out_path = pathlib.Path(__file__).with_name("gui-vectors.json")
    out_path.write_text(json.dumps(vectors, ensure_ascii=False, indent=2) + "\n")
    print(f"wrote {out_path.name}: {len(vectors)} vectors")


if __name__ == "__main__":
    main()
