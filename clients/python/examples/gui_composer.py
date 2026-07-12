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
from clausters.defs import (
    DoneAction,
    Env,
    SynthDef,
    control,
    env_gen,
    in_ctl,
    out,
    play_buf,
    sin_osc,
)
from clausters.gui import Editor, button, panel
from clausters.model import Buffer, Event, Group, Material, Sequence, Track
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
#
# The voice it drives is **in the composition**, not held by the script: an event
# with a length, placed beside the curve. It reads the automation's control bus
# straight (`in_ctl`), so nothing has to be `/n_map`-ed to a node that outlives its
# clip — the voice starts when the playhead reaches the clip and ends with it. Seek
# past the clip and there is simply no voice; a synth still humming over empty
# timeline is not a drone, it is a leak.

# %%
SWEEP = 4.0        # the sweep's length in beats (the curve's, and its voice's)


def drone(name: str = "drone") -> SynthDef:
    """A sine whose **frequency** is read from a control bus — the one the
    automation writes — held by a gate and freed on release, so its life is the
    *event's* (an envelope timed by a `sustain` control could not do it: `sustain`
    is the event's own key and never reaches the def).

    A curve's floor is its **parameter's minimum**, nothing more: this envelope
    reaching the bottom of its clip is the lowest *frequency*, not a silence. Draw
    an envelope over `amp` instead and the picture reads the other way — the bottom
    is silence, and the silence is part of the clip's length. Same clip, same
    curve; what it *means* is the control it drives."""
    bus = control("freq_bus", 0.0, "ir")
    amp = control("amp", 0.12, "kr")
    gate = control("gate", 1.0, "kr")
    shape = env_gen(Env.asr(attack=0.05, release=0.4), gate=gate,
                    done_action=DoneAction.FREE_SELF)
    sig = sin_osc(in_ctl(bus)) * shape * amp
    return SynthDef(name, out(0.0, sig), out(1.0, sig))


server.add_synthdef(drone())

sweep = Automation.from_points(
    [(0.0, 200.0, 1, 0.0),      # 200 Hz ...
     (2.0, 900.0, 2, 0.0),      # ... up to 900 (exponential) ...
     (4.0, 300.0, 1, 0.0)],     # ... back down (linear); shapes are the server's
    target=None, name="freq")    # no target node: it just writes its bus
sweep.prepare(server)            # the control buffer + bus, off the clock thread

# The envelope **attached to the voice it shapes**: a group whose members start
# and end together. The model already says what that is — its temporal relation is
# *simultaneous* — and the editor draws it as **one clip with layered bodies** (the
# curve over the note), which drags as one. The voice cannot outlive its envelope,
# and the envelope cannot be left behind.
voice = Event(SeqEvent(instrument="drone", freq_bus=sweep.bus.index,
                       dur=SWEEP, legato=1.0, amp=0.12, has_gate=True))
sweep_clip = Group([(0.0, voice), (0.0, Material(sweep, duration=SWEEP))],
                   name="sweep")

# The composition: four lanes, each a group placing one material in time.
song = Group([
    (0.0, Group([(0.0, take), (4.0, take)], name="drums")),
    (0.0, Group([(0.0, bass)], name="bass")),
    (2.0, Group([(0.0, melody)], name="lead")),
    (0.0, Group([(0.0, sweep_clip)], name="sweep")),
], name="song")

# %% [markdown]
# ## Open the editor, with a transport
# The model tree becomes a multitrack window: a lane per member, its materials as
# clips on one shared axis. ``extra`` places widgets of the script's own under the
# lanes — here the transport. Their ids are the script's (the editor allocates
# from 10000 up, so small ids never collide) and their events are the script's
# too: `Editor.apply` ignores them.

# %%
PLAY, PAUSE, STOP, REWIND, BAR = 1, 2, 3, 4, 5
transport = panel(BAR,
                  button(PLAY, label="play"),
                  button(PAUSE, label="pause"),
                  button(STOP, label="stop"),
                  button(REWIND, label="rewind"),
                  layout="row", height=0.25)

gui = session.gui()
editor = Editor(song, sample_rate=SR, tempo=TEMPO, quant=QUANT,
                extra=[transport], title="Composer")
win = editor.open(gui)
print(f"opened window {win} — drag a clip to move it, an edge to resize it")

# %% [markdown]
# ## The transport
# The editor owns it: `play` from where the cursor is (a fresh realization, so it
# plays the composition as it now stands), `pause` where we are, `stop` back to the
# top, and `locate` — which is also what a click on a lane's ruler does. Every play
# re-reads the model, so an edit made meanwhile is simply played.
#
# Nothing here silences anything, and nothing needs to: every voice in the
# composition is an event with a length, so it ends with its clip.

# %%
session.start()                       # the clock runs the routines

TRANSPORT = {PLAY: editor.play, PAUSE: editor.pause, STOP: editor.stop,
             REWIND: lambda: editor.locate(0.0)}
editor.realize(server, session.clock, at=0.0)   # the destination and the clock
editor.pause()                                  # ... but wait at the top
print("press play — click a lane's ruler (or its empty space) to move the cursor")


def ended() -> bool:
    """Whether the playhead ran past the end of the *current* composition (its
    length is read from the model, so dragging a clip out lengthens the piece)."""
    ph = editor.playhead
    return ph is not None and ph.playing and ph.position() >= editor.extent()


# %% [markdown]
# ## Edit it
# `Editor.apply` takes the host's events into the **model**: a dragged clip becomes
# a placement — its **offset** *and* its **length**, and the length trims what the
# material plays — and a dragged break-point becomes the automation's new curve.
# Anything it does not recognize is the script's: here, the transport buttons.
#
# An edit does not interrupt what is sounding. It marks the composition
# (`Editor.dirty`), and the next transport action re-reads it: realizing always
# re-flattens the model, so play, a resume after pause and a rewind all play the
# composition *as it now stands*.

# %%
if __name__ == "__main__":
    try:
        while editor.window is not None:
            if ended():
                editor.pause()         # the piece is over: the cursor stays at
                                       # the end (rewind goes back to the top)
            msg = gui.poll(0.05)
            if msg is None:
                continue
            addr, args = msg
            # A button reports its press (1) *and* its release (0): act on the
            # press, or every click would fire the transport twice.
            pressed = (addr == "/gui_event" and len(args) >= 2 and args[1] == 1)
            action = TRANSPORT.get(args[0]) if pressed else None
            if action is not None:
                action()
            elif editor.apply(addr, args):
                # An edit does not interrupt what is sounding: it changes the
                # *model*, and the next play (or a resume, or a rewind) plays it —
                # `realize` always re-flattens the composition, so it picks up the
                # new placements and lengths.
                print("  edited — press play to hear it"
                      if not editor.playhead or not editor.playhead.playing
                      else "  edited — press play to re-read the composition")
    finally:
        # No `sys.exit` here: it would replace an exception raised in the loop
        # and the window would just vanish with no word of why. Nothing to
        # silence, either: every voice in the composition ends with its clip.
        session.close()

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
