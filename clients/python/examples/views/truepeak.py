#!/usr/bin/env python3
"""True peak: what happened between the samples, drawn and measured.

The largest **sample** is not the largest value of the signal. A peak can fall
between two samples, and every converter sees it — so a signal whose samples all
read at full scale can be three decibels over it, and nothing that looks at
samples has anything to report. That is the **true peak**, in dBTP, and this
example shows both halves of it:

- **drawn**: zoomed in far enough that the samples are separate points, the
  ``waveform`` draws the band-limited **curve** between them rather than the
  straight segments a renderer would otherwise invent — and marks every peak
  that leaves full scale, with how far past it the loudest one went. The dots
  are still the data: the curve passes exactly through them;
- **measured**: ``ipc.true_peak`` reports the same fact as a number, through the
  shared core, with the filter ITU-R BS.1770-4 Annex 2 specifies — so the figure
  printed here is the one a delivery specification means when it asks for dBTP,
  and ``-1 dBTP`` is the ceiling those specifications name.

Three takes, read against each other. The first is the standard's own worst
case: a tone at a **quarter of the sample rate**, sampled at 45°, whose samples
sit at exactly ±1 while the signal between them reaches √2. The second is that
tone six decibels down, where nothing leaves full scale. The third is an
ordinary signal -- a band-limited **sawtooth at 440 Hz**, normalized to full
scale on its samples -- which is there to show the size of the effect in music
rather than in a test tone: its signal sits **0.2 dB** above its samples, not 3.

This file is organized as ``# %%`` cells (the VS Code / Jupyter convention) and
**runs out of the box**. Install once, from the repo root::

    python -m venv .venv
    .venv/bin/pip install -e ./clients/python      # bundles the server + GUI binaries

Run it cell by cell (Shift+Enter), or as a plain script --
``python clients/python/examples/views/truepeak.py``. It self-launches the audio
server and the GUI host; by hand that is ``clausters`` and ``clausters-gui
--server 127.0.0.1:57110``. Needs a display and a GPU adapter. It makes no
sound: the takes are looked at, not played.
"""

# %%
import math
import sys
from array import array

from clausters import Session, ipc
from clausters.defs import Buffer
from clausters.gui import view, waveform

# %% [markdown]
# ## Launch the server and the GUI
# The host fetches the buffers over its client leg, so it needs `--server`,
# which `Session.gui()` wires up.

# %%
session = Session.live()
server = session.server
gui = session.gui()

# %% [markdown]
# ## Three takes
# A quarter of the sample rate, sampled at 45°: the samples land at ±1 and the
# signal between them reaches √2. The second is the same tone six decibels
# down, where nothing leaves full scale. The third is an ordinary one, a
# band-limited sawtooth at 440 Hz, for the size of the effect in music.

# %%
FRAMES = 240


def quarter_rate(amp: float) -> array:
    """A tone at fs/4 sampled at 45°: +a, +a, -a, -a, ..."""
    return array("f", [amp if (i // 2) % 2 == 0 else -amp for i in range(FRAMES)])


def sawtooth(freq: float, amp: float, rate: float = 48_000.0) -> array:
    """A band-limited sawtooth: the harmonics up to Nyquist, summed -- what a
    real take of one holds, as against the ideal ramp, which no sampled signal
    is."""
    out = []
    for i in range(FRAMES):
        t = i / rate
        v, h = 0.0, 1.0
        while freq * h < rate / 2:
            v += (-1.0) ** (h + 1) * (2.0 / (h * math.pi)) * math.sin(
                2 * math.pi * freq * h * t)
            h += 1
        out.append(v)
    scale = amp / max(abs(v) for v in out)
    return array("f", [v * scale for v in out])


over = quarter_rate(1.0)
clean = quarter_rate(0.5)
saw = sawtooth(440.0, 1.0)

over_take = Buffer.from_samples(over, 1, server=server)
clean_take = Buffer.from_samples(clean, 1, server=server)
saw_take = Buffer.from_samples(saw, 1, server=server)
server.sync()

# %% [markdown]
# ## The numbers, before the picture
# The sample peak and the true peak of each take, through the shared core. The
# first reads full scale on its samples and three decibels over on the signal;
# the second is six decibels down on both; the sawtooth is a fifth of a decibel
# apart, which is what an ordinary signal looks like.

# %%
def report(name: str, samples: array) -> None:
    (sample_peak,), _ = ipc.channel_stats(samples, 1)
    (peak,) = ipc.true_peak(samples, 1)
    db = lambda a: 20 * math.log10(a) if a > 0 else float("-inf")
    print(f"{name:>6}: samples {db(sample_peak):+6.2f} dBFS | "
          f"signal {db(peak):+6.2f} dBTP")


report("over", over)
report("clean", clean)
report("saw", saw)

# %% [markdown]
# ## The picture
# Both takes at sample zoom, so the curve regime is entered: the dots are the
# samples, the line between them is the reconstruction, and the marks at the top
# and bottom of the first lane are the peaks that left full scale.

# %%
win = view(
waveform(buffer=over_take.bufnum, name="over", label="fs/4 at 45 deg, full scale",
         ruler_y=True),
waveform(buffer=clean_take.bufnum, name="clean", label="the same tone, -6 dB",
         ruler_y=True),
waveform(buffer=saw_take.bufnum, name="saw", label="440 Hz sawtooth, full scale",
         ruler_y=True),
title="True peak: between the samples", w=760, h=620, layout="col").open()
print("zoom into the first lane until the sample dots separate: the curve "
      "between them leaves the +-1 band, and the marks say where. Close the "
      "window to stop")

# %%
if __name__ == "__main__" and not hasattr(sys, "ps1"):
    try:
        win.wait()
    finally:
        session.close()
else:
    print("takes up - session.close() to end")
