#!/usr/bin/env python3
"""A multitrack timeline: tracks of clips placed on one shared time axis.

The DAW-style track editor. A ``track`` is a horizontal lane; a ``clip`` is a
placed rectangle on it spanning ``[offset, offset + dur]`` in timeline sample
units — the model's **graphic unit**, whose *length is its duration*. The
window's tracks share **one time axis** (aligned lanes), so a clip at a given
offset lines up across tracks — the seat the linked-views work
(``gui_linked.py``) designed: a member with a *placement* (offset) on the
shared timeline.

This example lays out three tracks — two audio takes whose bodies are **mapped
files** (the bulk path: a real take is minutes long, so it never rides the wire
as JSON; the host maps it and decimates it to the clip's pixel width through the
peak pyramid), and one **piano-roll** lead whose clip carries ``(start, dur,
pitch)`` note events drawn as bars (pitch on the vertical axis). The bottom lane
draws a **time ruler**, and every lane shows a **playhead** anchored to the
engine's sample clock, so the composition can be watched playing over its clips.

Dragging a clip (move) or its edge (resize) flows back as a ``"clip"`` event
carrying the new ``offset``/``dur`` — the edit-back pattern — so a driver can
update the composition model and re-realize. This script just prints those
events.

Run it as a script (``python gui_multitrack.py``) or cell by cell (``# %%``).
Needs a display and a GPU adapter; the install bundles the GUI binary (see
``gui_editor.py`` for the setup notes).
"""

# %%
import math
import sys
import tempfile
import time
from pathlib import Path

from clausters import Session
from clausters.gui import clip, label, samples_to_file, track, window

# %%
session = Session.live()
server = session.server
gui = session.gui()

SR = float(server.sample_rate)
TEMPO = 2.0                    # beats per second (120 bpm)
BEAT = int(SR / TEMPO)         # timeline samples per beat: the axis unit is the
                               # audio sample, so a take's frames place 1:1


def take(path: Path, beats: float, freq: float, decay: float = 3.0) -> tuple[str, int]:
    """Write a decaying tone of ``beats`` beats to ``path`` as raw ``f32`` and
    return ``(path, frames)`` — the take a clip maps.

    This is the **bulk path**: the host memory-maps the file and builds (and
    caches) its peak pyramid, so the lane draws one min/max column per pixel no
    matter how long the take is. Nothing crosses OSC. A clip's ``dur`` is the
    take's frame count, so it sits 1:1 on the shared sample axis."""
    frames = int(beats * BEAT)
    samples = [math.sin(2 * math.pi * freq * i / SR) * math.exp(-decay * i / frames)
               for i in range(frames)]
    samples_to_file(samples, str(path))
    return str(path), frames


# %% [markdown]
# ## Compose the tracks
# Three lanes under one window (a ``col`` layout stacks them). Each clip names an
# ``offset`` (its start on the shared timeline) and a ``dur`` (its length). The
# two audio lanes name a mapped file (``path``); the lead is a piano-roll. The
# tracks align because the window computes one time axis spanning the longest
# clip end — and the bottom lane rules that axis in beats.

# %%
tmp = Path(tempfile.mkdtemp(prefix="clausters-multitrack-"))
kick, kick_frames = take(tmp / "kick.f32", 2, 80.0, decay=6.0)
fill, fill_frames = take(tmp / "fill.f32", 4, 160.0, decay=2.0)
root, root_frames = take(tmp / "root.f32", 4, 55.0, decay=1.0)
turn, turn_frames = take(tmp / "turn.f32", 4, 65.0, decay=1.0)

DRUMS, BASS, LEAD = 1, 2, 3
LANES = (DRUMS, BASS, LEAD)

# The lane chrome: `snap` is the drag grid (a quarter beat), and the bottom lane
# rules the shared axis in beats (`tempo` + `sample_rate` label the ticks).
lane_chrome = dict(snap=BEAT / 4, sample_rate=SR, tempo=TEMPO)

win = gui.open(window(
    track(DRUMS,
          clip(10, offset=0 * BEAT, dur=kick_frames, path=kick, label="kick"),
          clip(11, offset=2 * BEAT, dur=kick_frames, path=kick, label="kick"),
          clip(12, offset=4 * BEAT, dur=fill_frames, path=fill, label="fill"),
          label="drums", **lane_chrome),
    track(BASS,
          clip(20, offset=0 * BEAT, dur=root_frames, path=root, label="root"),
          clip(21, offset=4 * BEAT, dur=turn_frames, path=turn, label="turn"),
          label="bass", **lane_chrome),
    track(LEAD,
          # A piano-roll clip: (start, dur, pitch) events relative to the clip,
          # pitch mapped over [min, max]. The whole roll moves with the clip.
          clip(30, offset=2 * BEAT, dur=6 * BEAT, min=48, max=72,
               notes=[(0 * BEAT, BEAT, 60), (1 * BEAT, BEAT, 64),
                      (2 * BEAT, BEAT, 67), (3 * BEAT, 2 * BEAT, 72),
                      (5 * BEAT, BEAT, 67)],
               label="theme"),
          label="lead", ruler="beats", **lane_chrome),
    label(99, "Multitrack: clips placed on one shared time axis"),
    title="Multitrack timeline", w=1000, h=520, layout="col",
))
print(f"opened window {win} — clips of the three tracks line up on one axis")

# %% [markdown]
# ## Roll the playhead
# ``playhead_at`` anchors timeline position 0 to a value of the engine's sample
# clock; the host draws the line at ``clock - playhead_at`` every frame, so it
# sweeps the lanes on its own. Anchoring it at *now* starts it at the left edge —
# call ``roll()`` again to re-anchor (a locate).

# %%
def roll():
    """Anchor every lane's playhead at the current engine clock."""
    _, args = server.request("/clock", expect=("/clock.reply",))
    now = float(args[0])
    for lane in LANES:
        gui.set(lane, playhead_at=now)


roll()

# %% [markdown]
# ## Move and resize clips from the script
# A clip's placement is live: ``gui.set`` its ``offset`` (start) or ``dur``
# (length). The lane redraws with the clip in its new spot; because the shared
# axis spans the longest clip, pushing one clip out lengthens the whole view.

# %%
gui.set(12, offset=5 * BEAT)          # slide the drum fill a beat later
gui.set(30, dur=8 * BEAT)             # stretch the lead theme


def drain_events(closed=[False]):
    """Print any clip edit-back events (drag/resize) — the ``"clip"`` payload."""
    while (msg := gui.poll(0.0)) is not None:
        addr, args = msg
        if addr == "/gui_closed":
            closed[0] = True
            print("window closed")
        elif addr == "/gui_event" and len(args) >= 4 and args[1] == "clip":
            wid, _, offset, dur = args[:4]
            print(f"clip {wid}: offset {offset:.0f} dur {dur:.0f} samples "
                  f"({offset / BEAT:.2f} .. {(offset + dur) / BEAT:.2f} beats)")
    return closed[0]


# %%
if __name__ == "__main__":
    try:
        deadline = time.monotonic() + 60.0
        while time.monotonic() < deadline and not drain_events():
            time.sleep(0.05)
    finally:
        gui.free(win)
        session.close()
        sys.exit(0)
