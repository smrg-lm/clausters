#!/usr/bin/env python3
"""Compose the arrangement, edit it on screen, hear the edit — the whole loop.

The multitrack **editor**: `clausters.gui.Editor` is the bridge between the
arrangement (`clausters.form` — elements placed recursively by offset) and the
multitrack view (tracks of clips on one shared time axis). It draws the
arrangement tree as a GuiDef, applies the clip edit-backs the host sends *onto
the arrangement*, and re-renders it. So the thing you drag is not a picture of
the music: it is the music, and the score follows.

What the mapping does, in one paragraph. The root group's members are the
**lanes**; a lane's members are its **clips**. A `Buffer` clip names its server
buffer and spans its frames (the host fetches it and decimates it — a real take
never rides the wire as JSON). An element of *events* draws a **piano-roll**, and
a contained generator is bounced in the same pass, so a `Pbind` lane shows the
notes it is about to play — the *change of state*, on screen. An `Automation`
draws its **curve** as the clip body, editable in place. A nested group draws as
the labeled rectangle that summarizes it, until you ``expand`` it into lanes of
its own: that collapse/expand is the **base level**, the zoom that summarizes or
resolves.

The axis is navigable: the wheel zooms and Shift+drag pans, and every lane moves
with it (they share one axis).

Drag a clip (move) or its edge (resize) and, with ``follow=True``, the
composition is re-scheduled from the playhead — you hear it where you dropped it.
**undo** and **redo** walk that back and forward — the two buttons, or
**Ctrl+Z** / **Ctrl+Shift+Z** over the window. The history is the shared
crate's, beside the document it inverts, so it is one history however many
surfaces edit the piece.
The lanes' playhead sweeps the clips with the engine clock, and the drag snaps to
the musical ``quant`` grid, which is the same grid the arrangement re-schedules on.

Run it as a script (``python gui_composer.py``) or cell by cell (``# %%``).
Needs a display and a GPU adapter; the install bundles the GUI binary (see
``gui_editor.py`` for the setup notes).
"""

# %%
import tempfile
from pathlib import Path

from clausters import Session
from clausters.defs import (
    Buffer as ServerBuffer,
    DoneAction,
    Env,
    SynthDef,
    control,
    env_gen,
    in_ctl,
    out,
    play_buf,
    sine,
)
from clausters.gui import Editor, button, panel
from clausters.form import Buffer, Element, Event, Group, Sequence, Track
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
# control. That is the arrangement's rule, and this is the def that satisfies it.

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
sampler().send(server)

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
    ``/node_free`` that ends it.)"""
    offline = Session.nrt(tempo=TEMPO)
    offline.play(Pbind(midinote=Pseq([36], 1), dur=beats, legato=1.0, amp=0.3))
    stats = offline.render(sample_rate=SR, channels=1, path=path)
    print(f"bounced {stats.frames} frames of take -> {path}")
    return path


wav = bounce_take(str(Path(tempfile.mkdtemp(prefix="clausters-")) / "take.wav"))
buf = ServerBuffer.read(wav, server=server)    # on the server, shape known

# %% [markdown]
# ## The material
# Three elements, three of the five primitives: the take is a **Buffer** (data
# — it sounds through the *instrument* named to play it), the melody a **Track**
# (a set of events placed in time), the bass a **Sequence** wrapping a pattern — a
# **Function**, a generator the editor bounces to draw and the render bounces
# to play. Same tree, both times.

# %%
# Two **elements** over one server buffer, since this lane places the take
# twice: the material is shared, the placements are not. One object in two
# places would be one name for two positions, and an edit-back could not say
# which of them it meant.
take = Buffer(buf, duration=2.0, instrument="take")       # the element over it
take_again = Buffer(buf, duration=2.0, instrument="take")
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
# flows back onto the `Automation`, whose `Env` is what the next render
# plays, so the curve you draw is the curve you hear.
#
# The voice it drives is **in the composition**, not held by the script: an event
# with a length, placed beside the curve. It reads the automation's control bus
# straight (`in_ctl`), so nothing has to be `/node_map`-ed to a node that outlives its
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
    sig = sine(in_ctl(bus)) * shape * amp
    return SynthDef(name, out(0.0, sig), out(1.0, sig))


drone().send(server)

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
sweep_clip = Group([(0.0, voice), (0.0, Element(sweep, duration=SWEEP))],
                   name="sweep")

# The composition: four lanes, each a group placing one material in time.
song = Group([
    (0.0, Group([(0.0, take), (4.0, take_again)], name="drums")),
    (0.0, Group([(0.0, bass)], name="bass")),
    (2.0, Group([(0.0, melody)], name="lead")),
    (0.0, Group([(0.0, sweep_clip)], name="sweep")),
], name="song")

# %% [markdown]
# ## Open the editor, with a transport
# The model tree becomes a multitrack window: a lane per member, its materials as
# clips on one shared axis. ``extra`` places widgets of the script's own under the
# lanes — here the transport, whose buttons are *named*. `Editor.open` hands back
# a window handle (like `GuiHost.open`), so the script resolves each button with
# ``win["play"]`` and never picks an id; their events are the script's too
# (`Editor.apply` ignores them).
#
# The transport is **chrome**, so it takes a fixed `h` and the lanes take the rest:
# a container's size on the main axis is `h`/`w` or a `weight`, and a strip left
# elastic would claim a lane's share of the window. The buttons inside it need no
# size of their own — a button knows how tall it wants to be.

# %%
transport = panel(button(name="play", label="play"),
                  button(name="pause", label="pause"),
                  button(name="stop", label="stop"),
                  button(name="rewind", label="rewind"),
                  button(name="undo", label="undo"),
                  button(name="redo", label="redo"),
                  layout="row", h=34.0)

gui = session.gui()
editor = Editor(song, sample_rate=SR, tempo=TEMPO, quant=QUANT,
                extra=[transport], title="Composer")
win = editor.open(gui)
print(f"opened window {win} — drag a clip to move it, an edge to resize it")

# %% [markdown]
# ## The transport
# The editor owns it: `play` from where the cursor is (a fresh render, so it
# plays the composition as it now stands), `pause` where we are, `stop` back to the
# top, and `locate` — which is also what a click on a lane's ruler does. Every play
# re-reads the arrangement, so an edit made meanwhile is simply played.
#
# Nothing here silences anything, and nothing needs to: every voice in the
# composition is an event with a length, so it ends with its clip.

# %%
session.start()                       # the clock runs the routines

# `play` is where the destination and the clock come from — rendering is *playing*,
# so nothing is rendered until the button is pressed (a window that sounds before
# you press play is a window that plays itself). Each button acts on its press
# (1), ignoring the release, and is wired by name onto the editor's transport.
press = lambda fn: (lambda value: fn() if value == 1 else None)  # noqa: E731
win["play"].on_event(press(lambda: editor.play(server, session.clock)))
win["pause"].on_event(press(editor.pause))
win["stop"].on_event(press(editor.stop))
win["rewind"].on_event(press(lambda: editor.locate(0.0)))
# Undo and redo are the same shape as the transport buttons and are **not** the
# editor's own history: the log lives in the shared crate, beside the document
# it inverts, so a script editing the arrangement or a second view on the same
# composition steps back through the same one. The clip springs back to where
# it was and the window is told so without being redefined.
win["undo"].on_event(press(editor.undo))
win["redo"].on_event(press(editor.redo))
# The keyboard reaches the same history without either button: the host sends
# Ctrl+Z as an ``"undo"`` addressed to the *window* -- undo is aimed at no
# place under the cursor -- and `Editor.apply` answers it in the loop below.
editor.locate(0.0)                              # the cursor waits at the top
print("press play — click a lane's ruler (or its empty space) to move the cursor")


# %% [markdown]
# ## Edit it
# `Editor.apply` takes the host's events into the **model**: a dragged clip becomes
# a placement — its **offset** *and* its **length**, and the length trims what the
# material plays — and a dragged break-point becomes the automation's new curve.
# Anything it does not recognize is the script's: here, the transport buttons.
#
# An edit does not interrupt what is sounding. It marks the composition
# (`Editor.dirty`), and the next transport action re-reads it: rendering always
# re-flattens the tree, so play, a resume after pause and a rewind all play the
# composition *as it now stands*.

# %%
if __name__ == "__main__":
    try:
        while editor.window is not None:
            # The playhead reports the end of its own scan, so the piece ends by
            # itself: the cursor parks at the composition's `extent` -- read from
            # the arrangement, so a clip dragged out lengthens the piece -- rather
            # than sweeping past it (rewind goes back to the top).
            editor.transport.update()
            msg = gui.poll(0.05)
            if msg is None:
                continue
            addr, args = msg
            # The transport buttons are handles: `dispatch` routes their press/
            # release to the `on_event` callbacks above (the `press` guard drops
            # the release). A `/gui_closed` has no handler, so it falls through to
            # `editor.apply`, which nulls the window and stops the loop.
            if gui.dispatch(addr, args):
                continue
            # A clip edit-back onto the arrangement (a move/resize, or a note or
            # break-point dragged in a clip body). Anything the editor does not
            # recognize falls through untouched.
            if editor.apply(addr, args):
                # An edit does not interrupt what is sounding: it changes the
                # *model*, and the next play (or a resume, or a rewind) plays it —
                # `render` always re-flattens the composition, so it picks up the
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
# The same arrangement, rendered offline: `Session.nrt` renders the edited composition
# to a WAV — sample-identical to what the RT engine played, because both converge
# on the same score.
#
# ```python
# offline = Session.nrt(tempo=TEMPO)
# sampler().send(offline.server)
# ServerBuffer.read(wav, server=offline.server)    # the take, on the offline server
# song.render(offline.server, offline.clock)
# samples = offline.render()
# ```
