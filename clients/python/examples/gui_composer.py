#!/usr/bin/env python3
"""Compose in the model, edit it on screen, hear the edit — the whole loop.

The multitrack **editor**: `clausters.gui.Editor` is the bridge between the
compositional model (`clausters.model` — materials placed recursively by offset)
and the multitrack view (tracks of clips on one shared time axis). It renders the
model tree to a GuiDef, applies the clip edit-backs the host sends *onto the
model*, and re-realizes it. So the thing you drag is not a picture of the music:
it is the music, and the score follows.

What the mapping does, in one paragraph. The root group's members are the
**lanes**; a lane's members are its **clips**. A `Buffer` clip names its server
buffer and spans its frames (the host fetches it and decimates it — a real take
never rides the wire as JSON). A material of *events* draws a **piano-roll**, and
a contained generator is bounced in the same pass, so a `Pbind` lane shows the
notes it is about to play — the model's *change of state*, on screen. An
`Automation` draws its **curve** as the clip body, editable in place. A nested
group draws as the labeled rectangle that summarizes it, until you ``expand`` it
into lanes of its own: that collapse/expand is the model's **base level**, the
zoom that summarizes or resolves.

The axis is navigable: the wheel zooms and Shift+drag pans, and every lane moves
with it (they share one axis).

Drag a clip (move) or its edge (resize) and, with ``follow=True``, the
composition is re-scheduled from the playhead — you hear it where you dropped it.
The lanes' playhead sweeps the clips with the engine clock, and the drag snaps to
the musical ``quant`` grid, which is the same grid the model re-schedules on.

Run it as a script (``python gui_composer.py``) or cell by cell (``# %%``).
Needs a display and a GPU adapter; the install bundles the GUI binary (see
``gui_editor.py`` for the setup notes).
"""

# %%
import struct
import sys
import tempfile
import time
import wave
from pathlib import Path

from clausters import Session
from clausters.defs import SynthDef, control, out, play_buf
from clausters.gui import Editor
from clausters.model import Buffer, Group, Material, Sequence, Track
from clausters.seq import Automation, Timeline
from clausters.seq.event import Event as SeqEvent
from clausters.seq.pattern import Pbind, Pseq

TEMPO = 2.0          # beats per second (120 bpm)
QUANT = 0.5          # the drag grid: half a beat


# %% [markdown]
# ## The instrument that plays a buffer
# The notes use the server's stock ``default`` def; the **take** needs an
# instrument of its own, because a buffer is *data* — a `Buffer` material sounds
# through the def named to play it, which reads the buffer number from its ``buf``
# control. That is the model's rule, and this is the def that satisfies it.

# %%
def sampler(name: str = "take") -> SynthDef:
    """Plays a buffer once. The event frees the synth after its `sustain`, which
    is the clip's length — so the take stops when the clip ends."""
    buf = control("buf", 0.0, "ir")
    amp = control("amp", 0.8, "ir")
    sig = play_buf(buf, 0.0, 1.0, 0.0) * amp
    return SynthDef(name, out(0.0, sig), out(1.0, sig))


# `latency` schedules each event a touch ahead (a wall-clock timetag), so the
# server plays it on time instead of "as soon as possible".
session = Session.live(tempo=TEMPO, latency=0.1)
server = session.server
server.add_synthdef(sampler())

# %% [markdown]
# ## The take
# A real audio clip, made the way a composition really makes one: bounced offline
# and **loaded from the file** (a buffer is loaded or generated on the server,
# never push-filled). The GUI then draws it from the buffer itself — the host
# fetches the take and decimates it through its peak pyramid, so its length costs
# nothing on the wire.

# %%
SR = float(server.options.sample_rate)
BEAT = SR / TEMPO


def bounce_take(path: str, beats: float = 2.0) -> str:
    """Render a two-beat bass note offline and write it to a WAV — the take a
    composition loads from disk. (The event closes the score: it schedules the
    ``/n_free`` that ends it.)"""
    offline = Session.nrt(tempo=TEMPO)
    offline.play(Pbind(midinote=Pseq([36], 1), dur=beats, legato=1.0, amp=0.3))
    samples, frames = offline.render(sample_rate=SR, channels=1)
    with wave.open(path, "wb") as w:
        w.setnchannels(1)
        w.setsampwidth(2)
        w.setframerate(int(SR))
        w.writeframes(b"".join(struct.pack("<h", int(max(-1.0, min(1.0, s)) * 32767))
                               for s in samples))
    print(f"bounced {frames} frames of take -> {path}")
    return path


wav = bounce_take(str(Path(tempfile.mkdtemp(prefix="clausters-")) / "take.wav"))
buf = server.query_buffer(server.read_buffer(wav))   # on the server, shape known

# %% [markdown]
# ## The material
# Three materials, three of the model's primitives: the take is a **Buffer** (data
# — it sounds through the *instrument* named to play it), the melody a **Track**
# (a set of events placed in time), the bass a **Sequence** wrapping a pattern — a
# **Function**, a generator the editor bounces to draw and the realization bounces
# to play. Same tree, both times.

# %%
take = Buffer(buf, duration=2.0, instrument="take")       # a Buffer material
melody = Track(Timeline([                                 # a Set of events
    (0.0, SeqEvent(midinote=72, dur=1.0)),
    (1.0, SeqEvent(midinote=76, dur=1.0)),
    (2.0, SeqEvent(midinote=79, dur=2.0)),
]))
bass = Sequence(Pbind(midinote=Pseq([48, 48, 55, 53], 2),  # a Function (generator)
                      dur=1.0, amp=0.15))

# %% [markdown]
# ## An automation lane
# A break-point curve placed in time, driving a control — the same `bpf` model the
# envelope editor draws, now a **clip** on a lane: its body *is* the curve, and it
# is edited in place (drag a point, Ctrl+click to add or remove one). The edit
# flows back onto the `Automation`, whose `Env` is what the next realization
# plays, so the curve you draw is the curve you hear.

# %%
voice = server.synth("default", {"freq": 55.0, "amp": 0.12})
sweep = Automation.from_points(
    [(0.0, 200.0, 1, 0.0),      # 200 Hz ...
     (2.0, 900.0, 2, 0.0),      # ... up to 900 (exponential) ...
     (4.0, 300.0, 1, 0.0)],     # ... back down (linear); shapes are the server's
    target=(voice, "freq"), name="sweep")
sweep.prepare(server)           # the control buffer + bus, off the clock thread

# The composition: four lanes, each a group placing one material in time.
song = Group([
    (0.0, Group([(0.0, take), (4.0, take)], name="drums")),
    (0.0, Group([(0.0, bass)], name="bass")),
    (2.0, Group([(0.0, melody)], name="lead")),
    (0.0, Group([(0.0, Material(sweep))], name="sweep")),
], name="song")

# %% [markdown]
# ## Open the editor
# The model tree becomes a multitrack window: a lane per member, its materials as
# clips on one shared axis. ``follow=True`` re-realizes on every edit, so what you
# drag is what you hear.

# %%
gui = session.gui()
editor = Editor(song, sample_rate=SR, tempo=TEMPO, quant=QUANT, follow=True,
                title="Composer")
win = editor.open(gui)
print(f"opened window {win} — drag a clip to move it, an edge to resize it")

# %% [markdown]
# ## Play it
# `realize` flattens the model to absolute beats and plays it through a playhead
# (the model's own realization — the editor adds no path of its own), and anchors
# the lanes' playhead so the line sweeps the clips as the audio runs.

# %%
session.start()                       # the clock runs the routines
editor.realize(server, session.clock)

# %% [markdown]
# ## Edit it
# `poll` drains the host's events into the model: a dragged clip becomes a
# placement, in beats, snapped to the grid. With ``follow`` on, the composition is
# re-scheduled from the playhead — *re-schedule from here*, not a sample-exact
# splice, so a synth already sounding keeps sounding.

# %%
if __name__ == "__main__":
    try:
        deadline = time.monotonic() + 120.0
        while time.monotonic() < deadline:
            if editor.poll(0.05):
                for offset, _dur, member in song.members:
                    print(f"  {member.name or 'lane'} at beat {offset:g}")
            if editor.window is None:      # the window was closed
                break
    finally:
        session.close()
        sys.exit(0)

# %% [markdown]
# ## Bounce it
# The same model, realized offline: `Session.nrt` renders the edited composition
# to a WAV — sample-identical to what the RT engine played, because both converge
# on the same score.
#
# ```python
# offline = Session.nrt(tempo=TEMPO)
# offline.server.add_synthdef(sampler())
# offline.server.read_buffer(wav)          # the take, on the offline server
# song.realize(offline.server, offline.clock)
# samples = offline.render()
# ```
