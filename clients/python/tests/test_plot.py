"""The free-standing ``plot``: dispatch, window-tree building and the offline
render paths, without a GUI host process (a fake host captures the tree)."""

import itertools
import os

import pytest

from clausters.gui import guidef
from clausters.plot import PlotWindow, _sequence, plot
from clausters.seq import Pseq, Pwhite

SR = 48_000.0


def _embed_or_skip():
    try:
        from clausters import _native
        _native.lib()
    except OSError as e:
        pytest.skip(f"clausters-ffi not built: {e}")


class FakeHost:
    """Captures what plot() would send to a GuiHost."""

    def __init__(self):
        self.opened = []
        self.sets = []
        self.closed = []
        self._ids = itertools.count(1000)

    def alloc_id(self):
        return next(self._ids)

    def open(self, tree, *blobs, id=None):
        self.opened.append(tree)
        return 1000

    def set(self, id, **props):
        self.sets.append((id, props))

    def close(self, id):
        self.closed.append(id)


def _plot_widget(tree: dict) -> dict:
    assert tree["type"] == "window"
    (widget,) = tree["children"]
    assert widget["type"] == "plot"
    return widget


def test_guidef_plot_carries_the_new_props():
    w = guidef.plot(7, data=[0.0, 1.0], channels=2, view="spectrum",
                    overlay=True, sample_rate=48000.0, min=-2.0, max=2.0,
                    ruler="time", ruler_y="off", fft_size=1024,
                    db_floor=-90.0, db_ceil=-3.0, freq_scale="mel",
                    label="sig")
    assert w["type"] == "plot" and w["id"] == 7
    assert w["view"] == "spectrum" and w["freq_scale"] == "mel"
    assert w["overlay"] == 1 and w["channels"] == 2
    assert w["fft_size"] == 1024 and isinstance(w["fft_size"], int)
    assert (w["min"], w["max"]) == (-2.0, 2.0)
    assert (w["ruler"], w["ruler_y"]) == ("time", "off")
    # Omitted props do not ride the wire (auto-fit relies on their absence).
    lean = guidef.plot(8, data=[0.5])
    assert "min" not in lean and "max" not in lean and "view" not in lean


def test_sequence_materializes_lists_patterns_and_channels():
    # A plain list is one channel, index axis (rate 0).
    samples, channels, rate, label = _sequence([1.0, 2.0, 3.0], n=10)
    assert (samples, channels, rate) == ([1.0, 2.0, 3.0], 1, 0.0)
    assert label == "sequence"
    # A finite pattern materializes fully; an endless one caps at n.
    seq, _, _, _ = _sequence(Pseq([1.0, 2.0], repeats=2), n=100)
    assert seq == [1.0, 2.0, 1.0, 2.0]
    capped, _, _, _ = _sequence(Pwhite(0.0, 1.0), n=50)
    assert len(capped) == 50
    assert all(0.0 <= v <= 1.0 for v in capped)
    # A list of per-channel rows interleaves (truncated to the shortest).
    inter, channels, _, _ = _sequence([[1.0, 2.0, 3.0], [10.0, 20.0]], n=100)
    assert channels == 2
    assert inter == [1.0, 10.0, 2.0, 20.0]


def test_plot_opens_one_window_with_inline_data():
    host = FakeHost()
    win = plot([40.0, 47.0, 60.0], host=host)
    assert isinstance(win, PlotWindow)
    assert win.id == 1000 and len(host.opened) == 1
    widget = _plot_widget(host.opened[0])
    assert widget["data"] == [40.0, 47.0, 60.0]
    # A sequence has no rate: the x ruler reads index counts, and the value
    # range is auto-fitted (no min/max on the wire).
    assert widget["ruler"] == "samples"
    assert "sample_rate" not in widget and "min" not in widget
    # The handle drives the display live and closes its own window.
    win.set(view="spectrum")
    assert host.sets == [(widget["id"], {"view": "spectrum"})]
    win.close()
    assert host.closed == [1000]


def test_each_plot_takes_a_fresh_widget_id():
    # Widget ids share the host's global namespace: two windows carrying the
    # same widget id would collide (the second is skipped at define time and
    # its handle would retune the first window's plot).
    host = FakeHost()
    a = plot([1.0, 2.0], host=host)
    b = plot([3.0, 4.0], host=host)
    assert a.widget_id != b.widget_id
    ids = [_plot_widget(t)["id"] for t in host.opened]
    assert ids == [a.widget_id, b.widget_id]


def test_plot_bulk_data_goes_through_a_mapped_file():
    host = FakeHost()
    big = [float(i % 100) for i in range(10_000)]
    plot(big, n=20_000, host=host)
    widget = _plot_widget(host.opened[0])
    assert "data" not in widget
    path = widget["path"]
    assert os.path.getsize(path) == 10_000 * 4, "raw f32, all samples"


def test_plot_spectrum_props_reach_the_widget():
    host = FakeHost()
    plot([0.0, 1.0] * 8, view="spectrum", freq_scale="bark", fft_size=512,
         db_floor=-80.0, host=host)
    widget = _plot_widget(host.opened[0])
    assert widget["view"] == "spectrum"
    assert widget["freq_scale"] == "bark"
    assert widget["fft_size"] == 512
    assert widget["db_floor"] == -80.0


def test_plot_renders_a_synthdef_offline():
    _embed_or_skip()
    from clausters.defs import SynthDef, control, out, sin_osc

    freq = control("freq", 440.0)
    sd = SynthDef("plot_probe", out(0.0, sin_osc(freq) * 0.5))
    host = FakeHost()
    win = plot(sd, dur=0.25, sample_rate=SR, channels=1, host=host)
    assert isinstance(win, PlotWindow)
    widget = _plot_widget(host.opened[0])
    # A def render is real audio: it rides the bulk file with its rate.
    assert widget["sample_rate"] == SR
    assert widget["label"] == "plot_probe"
    frames = os.path.getsize(widget["path"]) // 4
    assert abs(frames - 0.25 * SR) <= 128, f"~0.25 s rendered, got {frames}"


def test_plot_renders_an_env_through_the_engine():
    _embed_or_skip()
    from clausters.defs import Env

    env = Env(levels=[0.0, 1.0, 0.0], times=[0.1, 0.3])
    host = FakeHost()
    plot(env, sample_rate=SR, host=host)
    widget = _plot_widget(host.opened[0])
    assert widget["label"] == "env"
    frames = os.path.getsize(widget["path"]) // 4
    assert abs(frames - 0.4 * SR) <= 128, f"~0.4 s (sum of times), got {frames}"


def test_plot_renders_a_graphdef_offline():
    _embed_or_skip()
    from clausters.defs import GraphDef, SynthDef, control, out, sin_osc

    freq = control("freq", 330.0)
    member = SynthDef("plot_gsrc", out(0.0, sin_osc(freq) * 0.25))
    g = GraphDef("plot_chain")
    g.add("plot_gsrc")
    host = FakeHost()
    # A GraphDef's member defs ride along via `defs` (the ephemeral offline
    # session starts empty).
    win = plot(g, defs=[member], dur=0.1, sample_rate=SR, channels=1, host=host)
    assert isinstance(win, PlotWindow)
    widget = _plot_widget(host.opened[0])
    assert widget["label"] == "plot_chain"
    frames = os.path.getsize(widget["path"]) // 4
    assert abs(frames - 0.1 * SR) <= 128, f"~0.1 s rendered, got {frames}"
