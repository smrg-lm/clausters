#!/usr/bin/env python3
"""The whole loop: compose the arrangement, edit it on screen, hear it, undo it,
save it, open it again.

The multitrack **editor**: `clausters.gui.FormEditor` is the bridge between the
arrangement (`clausters.form` — elements placed recursively by offset) and the
multitrack view (tracks of clips on one shared time axis). It draws the
arrangement tree as a GuiDef, applies the clip edit-backs the host sends *onto
the arrangement*, and re-renders it. So the thing you drag is not a picture of
the music: it is the music, and the score follows.

What the mapping does, in one paragraph. The root aggregate's members are the
**lanes**; a lane's members are its **clips**. A `Vector` clip names its server
buffer and spans its frames (the host fetches it and decimates it — a real take
never rides the wire as JSON). An element of *clangs* draws a **piano-roll**, and
a contained generator is bounced in the same pass, so a `Pbind` lane shows the
notes it is about to play — the *change of state*, on screen. An `Automation`
draws its **curve** as the clip body, editable in place. A nested aggregate
draws as the labeled rectangle that summarizes it, until you ``expand`` it into
lanes of its own: that collapse/expand is the **base level**, the zoom that
summarizes or resolves.

The axis is navigable: the wheel zooms and Shift+drag pans, and every lane moves
with it (they share one axis).

The loop, and why each step needs the one before it:

1. **Edit.** Drag a clip (move) or its edge (resize). The gesture leaves the host
   as an *intent* — where the hand put it, absolute — and the **shared crate**
   decides what it becomes: the musical ``quant`` grid snaps it there, not here.
   What comes back is the value that actually holds, and the window adopts it. So
   the clip lands on the grid even though nothing on this side snapped anything.
2. **Hear it.** With ``follow=True`` the composition is re-scheduled from the
   playhead on every edit, so you hear the clip where you dropped it. The lanes'
   playhead sweeps the clips with the engine clock.
3. **Undo it** — the buttons, or **Ctrl+Z** / **Ctrl+Shift+Z** over the window.
   The history is **not this editor's**: it lives with the document, in the same
   crate. A log a view keeps sees only the gestures *that view* made, so a script
   editing the arrangement, or a second window on the same piece, would leave it
   describing a composition that has moved on — and undoing would then write a
   state nobody was ever in.
4. **Save it.** A *session* is the document plus the one half a document
   deliberately lacks: the table saying where its samples live. Written here
   beside the WAV it references, with the **provenance** of the script that made
   it — carried opaquely, which is what makes re-generating possible without the
   format knowing how.
5. **Open it again.** The file rebuilds the arrangement and the node ids survive
   it, so the reopened piece is the same composition by *identity*. What it can
   **play** depends on who is there to supply it: the take resolves through the
   source table, the curve through the recipe this script still holds, and the
   pattern lane comes back **frozen** — a generator is code, the document carries
   a reference to it and never the algorithm, so a lane whose recipe nobody
   supplies is structure that draws and does not sound.

**One thing to try that is not a button**, because it is the rule the whole
placement model rests on: shorten a clip over its own notes. You hear fewer
notes and the element keeps all of them — lengthen it again and they come back.
A placement is a **window onto** an element, never a rewrite of it, which is why
this is reversible and why a trim is not the same act as *rendering* the
element down to what it produced.

**Two more, on the take.** An edge drag is a **trim**, so dragging the head of
the audio clip to the right hides the take's first frames and the waveform stops
moving — the samples stand still and the clip shows less of it; drag the edge
back and the frames come out again. And with the pointer over a clip, **`e`**
cuts it in two at the time cursor and **`j`** joins it back with what touches it:
the two halves are two windows onto one buffer, which is why the join can put
back exactly what the cut separated. (Over a roll the same two letters cut and
join *notes* — but only once notes are selected, so with nothing in hand they
still reach the clip.)

The drums lane holds **two different files** for that reason. Drag the second
take against the first and press `j`: what you get is one clip that reads both,
back to back — you hear the two notes in a row from a single clip, and the clip
draws a waveform per piece. Press `e` inside it and it comes apart into the two
windows it was made of, because nothing was copied. That element is a
`Segments`, and you can write one directly:
``Segments([(buf, 0, 1.0), (other_buf, 0, 1.0)], instrument="take")`` — a
window's length being in seconds, like every length over samples.

**And one about holding several clips at once.** Alt+drag over a lane sweeps a
**marquee**: the clips inside the swept span go into your hand and are drawn in
the selection's colours (Alt+click adds or removes one — the same key that adds
a note to a roll's selection, which is why it is this one and not Ctrl: over a
roll, Ctrl *removes* a note). Grab any of them by
its body and the whole block moves rigidly, keeping the distances between them —
and **Ctrl+Z puts all of them back in one step**, because one gesture is one
edit. That is the whole reason the host reports a block as a single message
(``"clips"`` on the lane) rather than one per clip: several placements the
owner applies as **one transaction**. Press **`q`** with the pointer over the
lane and the clips in hand quantize onto the lane's own grid — the same grid a
drag already lands on, so a quantize puts a clip where dragging it would have.
An edge is always one clip's: two clips of different lengths have no one edge
to pull.

**And one you should not see at all: the view never re-frames itself.** The
editor is built with ``autofit=False``, which says the picture is *yours*. It is
one switch with three faces, because a view fits itself to what is in it in
three places: the time axis stays where you left it when the composition's
length changes, a roll's pitch range stops re-centring when you drag a note, and
a clip whose length nobody stated stops resizing itself when you move the last
note of the phrase inside it. It is off here and on by default in the host, and
the reason is the difference between a monitor and an editor: in an editor the
content change is nearly always your own edit, and an edit that re-frames the
view is the window starting over under the hand that made it. Turn it on
(``autofit=True``) and every structural edit — a split, a join, a clip moved to
another lane, an undo — refits the view, which is what it did before there was a
switch.

**And one that was not possible until now: drag a clip onto another lane.** It
follows your hand down the stack, and the drop is one edit — the clip leaves one
lane's aggregate and joins the other's, in a single transaction, so Ctrl+Z puts
it back where it came from in one step. It is the same call that moves a note
between rows in a roll: a lane and a semitone are one structure, so the rule
that decides which one the cursor is over is written once. An edge drag never
does it — a trim is a length, and says nothing about which lane a clip is on.

**And one about what your hand is on.** A clip draws its contents over each
other and *one* of them is being edited: press the sweep's curve and you edit the
curve (its points, and the bend between two of them) while the clip's grips
disappear; press the clip's own background and the clip is back in hand, grips
and all. A clip whose notes are a pattern's — a rendering of an algorithm —
hands the press straight to the clip, so it still moves and trims like any
other.

Needs an audio device, a display and a GPU adapter; the install bundles the GUI
binary (see ``views/editor.py`` for the setup notes). Run it as a script
(``python composer.py``) or cell by cell (``# %%``): the window stays up
between cells.
"""

# %%
import json
import sys
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
from clausters.gui import FormEditor, button, label, panel
from clausters.form import Aggregate, Element, Clang, Sequence, Track, Vector
from clausters.form.document import from_session, to_session
from clausters.seq import Automation, Timeline
from clausters.seq.event import Event as SeqEvent
from clausters.seq.pattern import Pbind, Pseq

TEMPO = 2.0          # beats per second (120 bpm)
QUANT = 0.5          # the drag grid: half a beat


# %% [markdown]
# ## The instrument that plays a buffer
# The notes use the server's stock ``default`` def; the **take** needs an
# instrument of its own, because a buffer is *data* — a `Vector` element sounds
# through the def named to play it, which reads the buffer number from its ``buf``
# control. That is the arrangement's rule, and this is the def that satisfies it.
#
# It also reads the element's **window**: a clip is a window onto a segment of its
# samples, so ``start`` says which frame to begin at and ``loop`` whether to wrap
# — the two the arrangement sends when the window is not the whole buffer. A def
# that named neither would play from the beginning whatever the clip drew, which
# is a picture and a sound disagreeing.

# %%
def sampler(name: str = "take") -> SynthDef:
    """Plays the segment of a buffer a clip shows. The event frees the synth
    after its `sustain`, which is the clip's length — so the take stops when the
    clip ends."""
    buf = control("buf", 0.0, "ir")
    amp = control("amp", 0.8, "ir")
    start = control("start", 0.0, "ir")     # the window's first frame
    loop = control("loop", 0.0, "ir")       # ...and whether it wraps
    sig = play_buf(buf, 0.0, 1.0, loop, 0.0, start) * amp
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
#
# The folder holding the WAV is where the session will be saved too, which is what
# makes the saved file's source table point at something that is still there —
# and lets it name it by a **relative** path, so the pair of files moves together.

# %%
# `query_info` rather than the launch options: it is the one spelling both
# clients have -- a page's engine is not a process anyone launched with flags,
# so the web `Server` has no `options` at all -- and this file and its page
# twin ask the same question.
SR = server.query_info().nominal_sample_rate
BEAT = SR / TEMPO
#: The bounce's length, twice: in **beats** of the clock that renders it, and in
#: the **seconds** it therefore lasts. A take's length is the second one -- the
#: samples are as long as they are, and a tempo change does not shorten a
#: recording -- so this is what the elements below are placed with.
TAKE_BEATS = 2.0
TAKE_SECS = TAKE_BEATS / TEMPO
folder = Path(tempfile.mkdtemp(prefix="clausters-"))


def bounce_take(path: str, beats: float = TAKE_BEATS, note: int = 60) -> str:
    """Render a two-beat bass note offline and write it to a WAV — the take a
    composition loads from disk. (The event closes the score: it schedules the
    ``/node_free`` that ends it.)"""
    offline = Session.nrt(tempo=TEMPO)
    # One octave under the melody rather than three: the take has to be *heard*
    # moving when a clip is dragged, and a low C mostly makes the speaker move.
    offline.play(Pbind(midinote=Pseq([note], 1), dur=beats, legato=1.0, amp=0.3))
    stats = offline.render(sample_rate=SR, channels=1, path=path)
    print(f"bounced {stats.frames} frames of take -> {path}")
    return path


wav = folder / "take.wav"
bounce_take(str(wav))
buf = ServerBuffer.read(str(wav), server=server)    # on the server, shape known

# This is the one line the web page cannot write: it has no folder to keep a
# take in, so it installs the render's samples directly with
# ``Buffer.from_samples`` -- the same verb this client has, and the one to use
# here too for a take that never becomes a file. What a page lacks is the file,
# not the call.

# A **second** take, from a second file: two different takes on one lane, so
# that joining them is a real join — an element that reads both, back to back,
# rather than two placements of one buffer.
other_wav = folder / "take_low.wav"
bounce_take(str(other_wav), note=48)
other_buf = ServerBuffer.read(str(other_wav), server=server)

# %% [markdown]
# ## The take
# Three elements, three of the five primitives: the take is a **Vector** (data
# — it sounds through the *instrument* named to play it), the melody a **Track**
# (an aggregate of clangs placed in time), the bass a **Sequence** wrapping a
# pattern — a **Function**, a generator the editor bounces to draw and the render
# bounces to play. Same tree, both times.

# %%
# Two **elements** over one server buffer, since this lane places the take
# twice: the samples are shared, the placements are not. One object in two
# places would be one name for two positions, and an edit-back could not say
# which of them it meant.
take = Vector(buf, duration=TAKE_SECS, instrument="take")       # the element over it
take_again = Vector(other_buf, duration=TAKE_SECS, instrument="take")  # the other file
melody = Track(Timeline([                                 # an aggregate of clangs
    (0.0, SeqEvent(midinote=72, dur=1.0)),
    (1.0, SeqEvent(midinote=76, dur=1.0)),
    (2.0, SeqEvent(midinote=79, dur=2.0)),
]))
bass = Sequence(Pbind(midinote=Pseq([48, 48, 55, 53], 2),  # a Function (generator)
                      dur=1.0, amp=0.15),
                name="bassline")   # the key a reopened session finds it by

# %% [markdown]
# ## An automation lane
# A break-point curve placed in time, driving a control — the same `bpf` model the
# envelope editor draws, now a **clip** on a lane: its body *is* the curve, and it
# is edited in place (drag a point, Ctrl+click to add or remove one). The edit
# flows back onto the `Automation`, whose `Env` is what the next render
# plays, so the curve you draw is the curve you hear.
#
# The voice it drives is **in the composition**, not held by the script: a clang
# with a length, placed beside the curve. It reads the automation's control bus
# straight (`in_ctl`), so nothing has to be `/node_map`-ed to a node that outlives its
# clip — the voice starts when the playhead reaches the clip and ends with it. Seek
# past the clip and there is simply no voice; a synth still humming over empty
# timeline is not a drone, it is a leak.

# %%
SWEEP = 4.0                 # the curve's length in **seconds** (an `Env`'s times)
SWEEP_BEATS = SWEEP * TEMPO   # ...and the same stretch in beats, for its voice


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

# The envelope **attached to the voice it shapes**: an aggregate whose members
# start and end together. The model already says what that is — its temporal relation is
# *simultaneous* — and the editor draws it as **one clip with layered bodies** (the
# curve over the note), which drags as one. The voice cannot outlive its envelope,
# and the envelope cannot be left behind.
voice = Clang(SeqEvent(instrument="drone", freq_bus=sweep.bus.index,
                       dur=SWEEP_BEATS, legato=1.0, amp=0.12, has_gate=True))
sweep_clip = Aggregate([(0.0, voice), (0.0, Element(sweep, duration=SWEEP))],
                   name="sweep")

# The composition: four lanes, each an aggregate placing one samples in time.
song = Aggregate([
    (0.0, Aggregate([(0.0, take), (4.0, take_again)], name="drums")),
    (0.0, Aggregate([(0.0, bass)], name="bass")),
    (2.0, Aggregate([(0.0, melody)], name="lead")),
    (0.0, Aggregate([(0.0, sweep_clip)], name="sweep")),
], name="song")

# %% [markdown]
# ## Saving, and opening again
# A **session** is the document plus the table saying where its samples live.
# The document says *what plays when* and deliberately not where a source is —
# inside a running system a source is a server buffer, a mapped file or a
# rendered result, and the tree has no business knowing which — so the table is
# the half that lets the thing be closed and opened.
#
# `FormEditor.load` points the open window at the reopened tree. The node ids survive
# the file, so it is the same composition by identity; the **history is dropped**,
# because its inverses describe a session that is over.

# %%
SESSION_FILE = folder / "song.claust"


def takes_of(element):
    """Every server buffer the composition currently plays.

    A source table has to describe *this* tree: the buffers a reopened piece
    holds are not the ones the script read at startup, because resolving a
    session reads each take again into a buffer of its own.
    """
    from clausters.form import Segments, Vector

    found = {}
    stack = [element]
    while stack:
        current = stack.pop()
        if isinstance(current, Vector) and getattr(current.wraps, "bufnum", None) is not None:
            found[current.wraps.bufnum] = current.wraps
        # A joined clip reads **several** buffers as one element, and a table
        # that named only the first would reopen with the rest missing.
        if isinstance(current, Segments):
            for seg in current.segments:
                if getattr(seg.buffer, "bufnum", None) is not None:
                    found[seg.buffer.bufnum] = seg.buffer
        stack.extend(child for _, _, child in getattr(current, "members", []) or [])
    return list(found.values())


def save():
    """Write the composition and where its samples are.

    **The table is built from the composition being saved**, not from the
    samples this script started with, and that distinction is the whole of a
    defect this example had: reopening resolves each take into a *new* server
    buffer, so a table naming the buffer read at startup stops covering the tree
    one save later — and the file it writes reopens with the takes unresolved
    and nothing drawn. `to_session` refuses such a file now, which is what turned
    a picture that quietly lost its waveforms into an error at the moment of
    saving.
    """
    sources = {
        take_buffer.bufnum: {
            # Relative to the session file's own folder, which is what makes the
            # pair of files movable together.
            "location": {"at": "file", "path": wav.name},
            "lifetime": "session",
            "generation": 0,
            "channels": 1,
            "frames": int(take_buffer.frames),
            "sample_rate": SR,
        }
        for take_buffer in takes_of(editor.element)
    }
    document = to_session(editor.element, sources=sources,
                          provenance={"script": "composer.py"})
    SESSION_FILE.write_text(json.dumps(document, indent=2))
    say(f"saved {SESSION_FILE} ({len(SESSION_FILE.read_text())} bytes)")


def reopen():
    """Read it back and show it — the same piece, from the file this time.

    **What the file names, this side supplies**, and that is the point of the
    resolver rather than a limitation of it: a document carries a *reference* to
    samples and to algorithms, never the samples and never the algorithm, so
    what a leaf becomes depends on who is there to answer for it. The take is a
    source id the table locates and this process reads back onto the server; the
    curve is named, so the recipe this script still holds is handed back and the
    lane plays again. The pattern lane is neither — it is code, and the reference
    the document kept is not something a name can find — so it comes back
    **frozen**: drawn, placed, silent. That is not the file being lossy; it is
    what a composition means somewhere its language is not running.
    """
    if not SESSION_FILE.exists():
        return say("nothing saved yet — press save first")
    raw = json.loads(SESSION_FILE.read_text())
    table = {int(k): v for k, v in (raw.get("sources") or {}).items()}
    frozen = [0]

    def resolve(kind, config):
        config = config or {}
        if kind == "vector":
            entry = table.get(int(config.get("source", -1)))
            path = ((entry or {}).get("location") or {}).get("path")
            return (None if path is None
                    else ServerBuffer.read(str(folder / path), server=server))
        if kind == "generator" and config.get("generator") == sweep.name:
            return sweep
        if kind == "sequence" and config.get("sequence") == "bassline":
            return bass.wraps
        frozen[0] += 1
        return None

    element, sources = from_session(raw, resolve=resolve)
    editor.load(element)
    say(f"opened {SESSION_FILE.name}: {len(element)} lanes, "
        f"{len(sources)} source(s) resolved, {frozen[0]} leaf/leaves frozen "
        f"— history cleared, node ids kept")


# %% [markdown]
# ## Open the editor, with a transport
# The model tree becomes a multitrack window: a lane per member, its takes as
# clips on one shared axis. ``extra`` places widgets of the script's own under the
# lanes — here the transport, the history and the file, whose buttons are *named*.
# `FormEditor.open` hands back a window handle (like `GuiHost.open`), so the script
# resolves each button with ``win["play"]`` and never picks an id; their events
# are the script's too (`FormEditor.apply` ignores them).
#
# The transport is **chrome**, so it takes a fixed `h` and the lanes take the rest:
# a container's size on the main axis is `h`/`w` or a `weight`, and a strip left
# elastic would claim a lane's share of the window. The buttons inside it need no
# size of their own — a button knows how tall it wants to be.

# %%
bar = panel(button(name="play", label="play"),
            button(name="pause", label="pause"),
            button(name="stop", label="stop"),
            button(name="rewind", label="rewind"),
            button(name="undo", label="undo"),
            button(name="redo", label="redo"),
            button(name="save", label="save"),
            button(name="open", label="open"),
            label("", name="status", align="start"),
            layout="row", h=34.0)


def say(message: str):
    """Report in the **window**, not only on stdout.

    A GUI example is read where it is looked at: an interactive step whose
    only feedback is a `print` reports to a terminal the person pressing the
    button may not have in front of them -- which is exactly how *save* came
    back as "I don't know what it wrote or where".
    """
    print(message)
    if editor.window is not None:
        editor.window["status"].set(text=message)


gui = session.gui()
editor = FormEditor(song, sample_rate=SR, tempo=TEMPO, quant=QUANT,
                    autofit=False,
                follow=True, extra=[bar], title="Composer")
win = editor.open()
print(f"opened window {win} — drag a clip to move it, an edge to resize it")

# %% [markdown]
# ## The transport
# The editor owns it: `play` from where the cursor is (a fresh render, so it
# plays the composition as it now stands), `pause` where we are, `stop` back to the
# top, and `locate` — which is also what a click on a lane's ruler does. Every play
# re-reads the arrangement, so an edit made meanwhile is simply played.
#
# Nothing here silences anything, and nothing needs to: every voice in the
# composition is a clang with a length, so it ends with its clip.

# %%
session.start()                       # the clock runs the routines

# `play` is where the destination and the clock come from — rendering is *playing*,
# so nothing is rendered until the button is clicked (a window that sounds before
# you press play is a window that plays itself). Each button acts on its
# **click** — the press completed on the button, so sliding off before letting go
# cancels it — and is wired by name onto the editor's transport.


def play():
    """Play from the top when the last pass ran out.

    The transport **parks at the end** of a pass rather than rewinding
    (`clausters.gui.transport`), so a bare play from there starts at the end
    and sounds nothing — which reads as a dead button unless you know to press
    stop first. A transport bar is not a puzzle, so this rewinds for you; the
    parking itself is right, and is what lets a pause resume where the music
    got to.
    """
    if editor.transport.extent is not None and editor.transport.at >= editor.transport.extent():
        editor.locate(0.0)
    editor.play(server, session.clock)


win["play"].on_click(play)
win["pause"].on_click(editor.pause)
win["stop"].on_click(editor.stop)
win["rewind"].on_click(lambda: editor.locate(0.0))
# Undo and redo are the same shape as the transport buttons and are **not** the
# editor's own history: the log lives in the shared crate, beside the document
# it inverts, so a script editing the arrangement or a second view on the same
# composition steps back through the same one. The clip springs back to where
# it was and the window is told so without being redefined.
win["undo"].on_click(editor.undo)
win["redo"].on_click(editor.redo)
win["save"].on_click(save)
win["open"].on_click(reopen)
# The keyboard reaches the same history without either button: the host sends
# Ctrl+Z as an ``"undo"`` addressed to the *window* -- undo is aimed at no
# place under the cursor -- and `FormEditor.apply` answers it in the loop below.
editor.locate(0.0)                              # the cursor waits at the top
print("press play — click a lane's ruler (or its empty space) to move the cursor")
print("undo/redo: the buttons, or Ctrl+Z / Ctrl+Shift+Z over the window")


# %% [markdown]
# ## Edit it
# `FormEditor.apply` takes the host's events into the **model**: a dragged clip becomes
# a placement — its **offset** *and* its **length**, and the length trims how much of the
# take plays — and a dragged break-point becomes the automation's new curve.
# Anything it does not recognize is the script's: here, the buttons above.
# `FormEditor.poll` drains the window's whole stream into it, so one call is the loop.
#
# With ``follow=True`` an edit re-schedules the composition from the playhead, so
# what you dropped is what you hear; rendering always re-flattens the tree, so a
# play, a resume after pause and a rewind all play the composition *as it now
# stands*.

# %%
def run():
    """Hold until the window is closed — a by-eye and by-ear test ends when the
    person looking at it says so, not on a timer."""
    while editor.window is not None:
        # The playhead reports the end of its own scan, so the piece ends by
        # itself: the cursor parks at the composition's `extent` -- read from
        # the arrangement, so a clip dragged out lengthens the piece -- rather
        # than sweeping past it (rewind goes back to the top).
        editor.transport.update()
        editor.poll(0.05)


# %%
if __name__ == "__main__" and not hasattr(sys, "ps1"):
    try:
        run()
    finally:
        # No `sys.exit` here: it would replace an exception raised in the loop
        # and the window would just vanish with no word of why. Nothing to
        # silence, either: every voice in the composition ends with its clip.
        session.close()
else:
    print("up — run() to hold the window, session.close() to end")

# %% [markdown]
# ## Bounce it
# The same arrangement, rendered offline: `Session.nrt` renders the edited composition
# to a WAV — sample-identical to what the RT engine played, because both converge
# on the same score.
#
# ```python
# offline = Session.nrt(tempo=TEMPO)
# sampler().send(offline.server)
# ServerBuffer.read(str(wav), server=offline.server)   # the take, on the offline server
# song.render(offline.server, offline.clock)
# samples = offline.render()
# ```
