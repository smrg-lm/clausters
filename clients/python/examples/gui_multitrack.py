#!/usr/bin/env python3
"""A desktop-shaped multitrack editor: menu bar, toolbar, ruler, lanes, transport.

The other GUI examples each open *one* view. This one composes a whole
**application window** out of the model — the shape a DAW has, top to bottom:

1. a **menu bar** whose entries are the view's own options (the ruler, the time
   unit, the spectral colormap) — each one a `menu` over its choices;
2. a **toolbar** repeating the two that are worth one click (a ruler `toggle`, a
   unit `menu`) plus a `slider` over how thick a lane is — the same thickness
   **Ctrl+wheel** over any lane sets, which arrives here as a ``"height"`` edit
   and is applied to the whole stack. Menu bar and
   toolbar are two faces of the same state, so operating either echoes back into
   the other — the host holds a widget's value, the script holds what it *means*;
3. the **time ruler** over the editor, a strip the document places (`timeruler`)
   and the *only* one in the window: a lane's own ruler is reserved out of that
   lane's height, so the lanes below carry none and the strip up here rules them
   all. Hiding it is ``set(h=0)``: it stays in the tree, keeps its navigation
   group, and comes back where it was;
4. the **editor** — three lanes inside a vertical `scroll` (``axis="y"``,
   ``zoom=False``: the wheel scrolls the stack, the time axis never moves), each
   lane a different kind of resource on one shared axis:
   ``takes`` places three **file** clips (raw ``f32`` the host maps and decimates
   through the peak pyramid — no samples on the wire), ``lead`` a **piano-roll**
   clip of note events, and ``spectrum`` a clip of a fourth file drawn as its
   **spectrogram** (``view="spectrogram"``) — a clip in every other respect,
   placed at an offset and ending at its duration, because the trace and the
   texture are two views of one signal. The three lanes join one navigation
   group (``link``), so a zoom or a pan on any of them moves all three and the
   ruler with them;
5. a **transport** row: play/pause, stop and rewind on the left, and beside them
   a **counter** reading the same position in every form at once — bars.beats,
   beats, seconds and samples — with the unit the menu selected in brackets.

The transport is `clausters.gui.Transport` over a `clausters.seq.Timeline`: play
anchors the lanes' playhead to the engine's sample clock (one message, and the
host sweeps the line on its own), pause parks the static cursor where the music
stopped, stop rewinds. Clicking the ruler emits ``"locate"`` and seeks there.

**What sounds is what is drawn, including the edits.** The lanes and the
transport read one description of the piece (`CLIPS` and `LANE_STATE`), and the
timeline is built afresh for every pass — so dragging or resizing a clip moves
what you hear, and a lane's mute, solo and fader are the mixer they look like.
Each of those edits re-schedules the pass from where it is, so it takes effect
on the beat you click it rather than on the next play. Every fundamental is well
clear of the bass register.

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
from clausters.defs import DoneAction, Env, SynthDef, control, env_gen, out, sine
from clausters.gui import (Transport, button, clip, label, layout, menu,
                           samples_to_file, scroll, slider, timeruler, toggle,
                           track, window)
from clausters.seq import Event, Playhead, Timeline

TEMPO = 2.0            # beats per second (120 bpm)
BAR = 4                # beats per bar, for the counter's bars.beats form
LINK = 7               # the navigation group the lanes and the ruler share
LANE_H = 120.0         # a lane's thickness inside the scrolled stack

# %%
session = Session.live(tempo=TEMPO, latency=0.1)
server = session.server
gui = session.gui()

SR = float(server.options.sample_rate)
BEAT = int(SR / TEMPO)         # timeline samples per beat: the axis unit is the
                               # audio sample, so a take's frames place 1:1

# %% [markdown]
# ## The take, as three kinds of resource
# A take is written to a file of raw little-endian ``f32`` — the bulk path a real
# minutes-long take needs, and the one a clip reads by ``path``. The lead is a
# list of ``(start, dur, pitch)`` events, drawn as a piano-roll. The spectral
# lane's clip gets a file of its own, a sweep. Nothing here rides the wire as
# JSON.

# %%
def tone(freq: float, beats: float, decay: float = 3.0) -> list:
    """A decaying sine of ``beats`` beats — one take's samples."""
    frames = int(beats * BEAT)
    return [math.sin(2 * math.pi * freq * i / SR) * math.exp(-decay * i / frames)
            for i in range(frames)]


tmp = Path(tempfile.mkdtemp(prefix="clausters-multitrack-"))

# The three takes: a fundamental each, none of them in the bass.
TAKES = (("hit", 220.0, 2.0, 0.0, 5.0),      # name, freq, beats, offset (beats), decay
         ("swell", 330.0, 4.0, 2.0, 1.2),
         ("tail", 440.0, 4.0, 6.0, 2.0))

samples = {}
paths = {}
for name, freq, beats, _at, decay in TAKES:
    samples[name] = tone(freq, beats, decay)
    paths[name] = str(tmp / f"{name}.f32")
    samples_to_file(samples[name], paths[name])

# The lead: note events relative to its clip, in timeline samples.
LEAD_AT = 2.0                                          # beats
LEAD = ((0.0, 1.0, 64), (1.0, 1.0, 67), (2.0, 1.0, 71),
        (3.0, 2.0, 76), (5.0, 1.0, 71))                # (start, dur, pitch) in beats
EXTENT = 10.0                       # the piece at rest, in beats (`extent`
                                    # reads the live one off the clips)

# The spectral lane's own take: a **sweep**, not one of the tones above. A clip
# shown as a spectrogram is worth looking at only if its samples moves in
# frequency, and it has to be its own recording — a clip is a clip, so this one
# starts where it starts and ends where it ends, like the takes beside it.
SWEEP_AT, SWEEP_BEATS = 1.0, 6.0
SWEEP_LO, SWEEP_HI = 300.0, 3000.0


def sweep(lo: float, hi: float, beats: float) -> list:
    """An exponential glide from ``lo`` to ``hi``, with two fixed partials over
    it — a picture the trace cannot show and the STFT can."""
    frames = int(beats * BEAT)
    out, phase = [], 0.0
    for i in range(frames):
        f = lo * (hi / lo) ** (i / frames)
        phase += 2 * math.pi * f / SR
        env = min(1.0, i / (0.05 * frames)) * min(1.0, (frames - i) / (0.2 * frames))
        out.append(env * (0.6 * math.sin(phase)
                          + 0.25 * math.sin(2 * math.pi * 660.0 * i / SR)
                          + 0.15 * math.sin(2 * math.pi * 1320.0 * i / SR)))
    return out


sweep_samples = sweep(SWEEP_LO, SWEEP_HI, SWEEP_BEATS)
sweep_path = str(tmp / "sweep.f32")
samples_to_file(sweep_samples, sweep_path)
print(f"wrote {len(TAKES)} takes and a {SWEEP_BEATS:.0f}-beat sweep under {tmp}")

# %% [markdown]
# ## The instruments
# The lanes draw the samples; these are what sound it. One decaying sine for the
# tones and the lead notes, and one glide for the sweep the spectral clip shows —
# both freed by their own envelope, and both shaped like the file drawn beside
# them: an instrument whose envelope is not the picture's makes the take look
# like it ends after it is over.

# %%
def voice(name: str = "multi_tone") -> SynthDef:
    """A decaying sine, self-freeing when its envelope ends.

    Its length is the ``secs`` control, not ``dur``: ``dur`` is one of the
    event's **reserved** keys — the scheduling — so it never reaches the synth,
    and a def that reads it gets its default forever (every take the same
    length, whatever the clip says). A clip's length in seconds travels as a
    control of its own.

    **Its shape is the file's shape**, and that is the point rather than a
    detail: the take drawn on the lane is a file written with
    ``exp(-decay · t/secs)``, so the synth plays that same curve — a ramp over
    the clip's span, exponentiated — with ``decay`` arriving as a control the
    way ``secs`` does. Written with a ready-made percussive envelope instead
    (which is what it was), the two disagree: at the end of a four-beat take
    the picture is at 0.30 and the sound at 0.001, so the eye sees samples
    where the ear hears none and the take reads as ending early."""
    freq = control("freq", 440.0, "ir")
    amp = control("amp", 0.2, "ir")
    secs = control("secs", 1.0, "ir")
    decay = control("decay", 3.0, "ir")
    # The ramp is the clip's own span: it is what frees the synth at the end,
    # and what the decay is measured on.
    ramp = env_gen(Env([0.0, 1.0], [1.0]), time_scale=secs,
                   done_action=DoneAction.FREE_SELF)
    sig = sine(freq) * (-decay * ramp).exp() * amp
    return SynthDef(name, out(0.0, sig), out(1.0, sig))


def glide(name: str = "multi_sweep") -> SynthDef:
    """The sweep the spectral clip draws: an exponential glide from `SWEEP_LO`
    to `SWEEP_HI` over the event's span, with two fixed partials over it — the
    picture the trace cannot show and the STFT can.

    The glide is an **exponential envelope**, which is why its ends are the
    ratio (``1`` to ``hi/lo``, scaled by ``lo``) and not the frequencies: an
    exponential segment cannot start at zero, and a ratio is what a geometric
    axis measures anyway."""
    amp = control("amp", 0.2, "ir")
    secs = control("secs", 1.0, "ir")
    freq = env_gen(Env([1.0, SWEEP_HI / SWEEP_LO], [1.0], "exp"),
                   time_scale=secs) * SWEEP_LO
    shape = env_gen(Env([0.0, 1.0, 1.0, 0.0], [0.05, 0.75, 0.2]),
                    time_scale=secs, done_action=DoneAction.FREE_SELF)
    sig = (sine(freq) * 0.6 + sine(660.0) * 0.25 + sine(1320.0) * 0.15) * shape * amp
    return SynthDef(name, out(0.0, sig), out(1.0, sig))


voice().send(server)
glide().send(server)
server.sync()

# %% [markdown]
# ## The arrangement: one description, two consumers
# What follows is the piece — where each clip sits and what it sounds — and it is
# read by *both* the lanes (which draw it) and the transport (which plays it).
# That is the point of writing it down instead of freezing a `Timeline` at
# startup: drag a clip and the next pass plays it where you dropped it, mute a
# lane and it stops sounding. A picture built once and a timeline built once are
# two things that drift; this is one thing seen twice.

# %%
def tone_events(freq: float, decay: float, amp: float = 0.2):
    """A take: one tone filling the clip. ``secs`` is the clip's own length, so a
    resized clip sounds as long as it looks — and ``decay`` is the one the file
    was written with, so it sounds the shape it draws."""
    def events(beats, gain):
        return [(0.0, Event(instrument="multi_tone", freq=freq, dur=beats,
                            secs=beats / TEMPO, decay=decay, amp=amp * gain))]
    return events


def lead_events(notes, amp: float = 0.14):
    """The lead: its notes, relative to the clip and **cut to it** — shortening
    the clip drops the notes past its end, the way a DAW trims a part."""
    def events(beats, gain):
        return [(start, Event(instrument="multi_tone",
                              freq=440.0 * 2 ** ((pitch - 69) / 12),
                              dur=min(dur, beats - start),
                              secs=min(dur, beats - start) / TEMPO, amp=amp * gain))
                for start, dur, pitch in notes if start < beats]
    return events


def sweep_events(amp: float = 0.18):
    """The spectral clip: one glide over the whole clip."""
    def events(beats, gain):
        return [(0.0, Event(instrument="multi_sweep", dur=beats,
                            secs=beats / TEMPO, amp=amp * gain))]
    return events


#: clip name -> where it sits (beats) and what it sounds. The `"clip"` edit-back
#: writes `at`/`beats` here, which is why an edit is heard and not only seen.
CLIPS = {name: {"lane": "takes", "at": at, "beats": beats,
                "events": tone_events(freq, decay)}
         for name, freq, beats, at, decay in TAKES}
CLIPS["theme"] = {"lane": "lead", "at": LEAD_AT, "beats": 6.0,
                  "events": lead_events(LEAD)}
CLIPS["sweep"] = {"lane": "spectrum", "at": SWEEP_AT, "beats": SWEEP_BEATS,
                  "events": sweep_events()}

#: lane name -> its header controls. The `"mute"`/`"solo"`/`"level"` edit-backs
#: write here, and `lane_gain` is what the pass reads.
LANES = ("takes", "lead", "spectrum")
LANE_STATE = {name: {"mute": False, "solo": False, "level": 0.8} for name in LANES}


def lane_gain(lane: str) -> float:
    """What a lane contributes to a pass: nothing when it is muted, nothing when
    something else is soloed, its fader otherwise — the mixer's three rules."""
    st = LANE_STATE[lane]
    soloing = any(s["solo"] for s in LANE_STATE.values())
    if st["mute"] or (soloing and not st["solo"]):
        return 0.0
    return st["level"]


def build_timeline() -> Timeline:
    """The arrangement as a `Timeline` the playhead can scan — built fresh for
    every pass, so it is always the piece as it now stands."""
    tl = Timeline()
    for spec in CLIPS.values():
        gain = lane_gain(spec["lane"])
        if gain <= 0.0:
            continue
        for rel, event in spec["events"](spec["beats"], gain):
            tl.add(spec["at"] + rel, event)
    return tl


def extent() -> float:
    """The piece's length in beats: where the last clip ends, read on each use —
    a clip dragged past the end lengthens it."""
    return max((c["at"] + c["beats"] for c in CLIPS.values()), default=0.0)


session.start()

# %% [markdown]
# ## The window
# Five strips in a column. The chrome (menu bar, toolbar, ruler, transport) takes
# a fixed ``h``; the editor takes the leftover ``weight``, so it dominates the
# window at any size — the app-shell rule.
#
# The lanes go inside a `scroll` configured as a plain vertical scroll view
# (``axis="y"``, ``zoom=False``): its content is taller than the strip, so the
# wheel walks the stack while the shared *time* axis stays exactly where the
# ruler says it is.

# %%
UNITS = ("beats", "time", "samples")       # the ruler units, in menu order
COLORMAPS = ("viridis", "magma", "gray")

# The axis every lane and the ruler share. `quant` is **beats per bar** — the
# grid the ruler's `bar:beat` labels count on — not a length in samples: give it
# one and the ruler decides a bar is 24000 beats, finds no step whose labels fit
# once you zoom out, and draws none.
axis = dict(link=LINK, sample_rate=SR, tempo=TEMPO, quant=float(BAR))
# `snap=0` is **no grid**: a clip moves by whole samples, so a drag follows the
# pointer instead of jumping a sixteenth at a time. It is the right setting for
# looking at the drawing (an edit lands where you put it, at any zoom); a piece
# being *assembled* wants the grid back — `snap=BEAT / 4` here — since the value
# is the lane's own, not a global mode.
lane_chrome = dict(snap=0.0, mute=False, solo=False, level=0.8, **axis)

# A row spreads its children over the whole width, because a control is elastic
# across its axis — the right default for a groove that wants the room and the
# wrong one for a menu bar, whose entries should hug their text. So they name a `w`,
# and a spacer with the weight takes what is left. The width is arithmetic on
# the host's 5x7 bitmap font: at the default `text_size` of 2.0 a character
# advances 12 logical pixels and a line is 14 tall, and a menu spends the rest
# on its paddings and the marker gutter at its right edge.
CELL, LINE, PAD = 12.0, 14.0, 4.0


def menu_w(label: str, options) -> float:
    """Wide enough for the longest option (plus the gutter) and for the label."""
    longest = max(len(o) for o in options)
    return max(longest * CELL + 4 * PAD + LINE + PAD, len(label) * CELL + 2 * PAD)


RULER_OPTIONS = ["ruler: shown", "ruler: hidden"]
UNIT_OPTIONS = [f"unit: {u}" for u in UNITS]
COLOR_OPTIONS = [f"colors: {c}" for c in COLORMAPS]

win = gui.open(window(
    # -- 1. the menu bar: one menu per view option, each cycling its choices
    layout(menu(RULER_OPTIONS, label="View", name="m_ruler",
                w=menu_w("View", RULER_OPTIONS)),
           menu(UNIT_OPTIONS, label="Time", name="m_unit",
                w=menu_w("Time", UNIT_OPTIONS)),
           menu(COLOR_OPTIONS, label="Spectrum", name="m_colors",
                w=menu_w("Spectrum", COLOR_OPTIONS)),
           label("", weight=1.0),          # the bar's empty right end
           # A strip is as tall as the tallest control it holds — a labelled
           # `menu` is a label row over a control row, 48 logical pixels with
           # the default metrics — **plus nothing**, which is why `margin=0` is
           # here: a container insets its children by its margin on every side,
           # so a strip sized to the control exactly would hand it a cell two
           # margins shorter and the field would come out shorter than its own
           # text. The `gap` still separates the entries.
           flow="row", h=48.0, gap=6.0, margin=0.0),

    # -- 2. the toolbar: the same state, one click away, plus the one size the
    # wire owns. Zooming *time* is not here on purpose: the wheel over any lane
    # does it, and it moves the whole navigation group. Lane **thickness** is
    # the other thing entirely — the plane's own zoom is uniform over both axes,
    # so growing the lanes with it would stretch the time axis out from under
    # the ruler; a lane's thickness is its `h`, and a control sets it.
    layout(toggle(label="ruler", value=True, name="t_ruler", w=90.0),
           menu(list(UNITS), label="unit", name="t_unit",
                w=menu_w("unit", UNITS)),
           # A `slider`, not a `number`: a number box fills and drags along the
           # **vertical**, which is the wrong axis for a strip this squat — the
           # bar it draws has a few pixels to move in. A slider lays its groove
           # along the axis the strip actually has, and reads its value out
           # under it.
           slider(label="lane px", min=48.0, max=400.0, value=LANE_H,
                  name="t_laneh", w=260.0),
           label("", weight=1.0),          # the strip's empty right end
           # Same shape as the menu bar: the tallest control's own height (the
           # labelled slider: label row, groove, read-out) and no margin.
           flow="row", h=62.0, gap=6.0, margin=0.0),

    # -- 3. the ruler over the editor, in its own box (no lane pays for it)
    timeruler(name="ruler", ruler="beats", h=22.0, **axis),

    # -- 4. the editor: three kinds of resource on one axis, vertically scrolled
    scroll(
        # A lane of **file** clips: three takes the host maps and decimates.
        track(*[clip(name=name, offset=at * BEAT, dur=len(samples[name]),
                     path=paths[name], label=name)
                for name, _f, _b, at, _d in TAKES],
              name="takes", label="takes", h=LANE_H, **lane_chrome),
        # A lane whose clip is a **piano-roll** of note events.
        track(clip(name="theme", offset=LEAD_AT * BEAT, dur=6 * BEAT,
                   min=48, max=84, label="lead",
                   notes=[(s * BEAT, d * BEAT, p) for s, d, p in LEAD]),
              name="lead", label="lead", h=LANE_H, **lane_chrome),
        # A lane of clips again — but this clip's take is drawn as its
        # **spectrogram**: `view="spectrogram"` picks the presentation, and
        # everything else about it is a clip. It is placed at an offset, it ends
        # at its duration, it drags and resizes with the same handle, and the
        # STFT stops where the samples does instead of spanning the lane. The
        # trace and the texture are two views of one signal, and this is where
        # the model says so.
        track(clip(name="sweep", offset=SWEEP_AT * BEAT, dur=len(sweep_samples),
                   path=sweep_path, view="spectrogram", sample_rate=SR,
                   window_size=1024, freq_scale="log", colormap=0, label="sweep"),
              name="spectrum", label="spectrum", h=LANE_H, **lane_chrome),
        name="editor", axis="y", zoom=False, flow="col", gap=4.0,
        content_h=3 * LANE_H + 8.0, weight=1.0),

    # -- 5. the transport: the buttons on the left, the counter beside them.
    # The buttons are **chrome**, so they take a fixed width and the counter
    # takes the leftover — the row's rule read the other way round. Left
    # elastic they split the row four ways and the read-out, which is the one
    # thing here whose width is its content, came back with an ellipsis: a
    # caption's budget is its box divided by the character cell (12 px at
    # ``text_size`` 2.0), and this one is ~59 characters wide.
    layout(button(label="play/pause", name="b_play", w=110.0),
           button(label="stop", name="b_stop", w=110.0),
           button(label="rewind", name="b_rew", w=110.0),
           label("", name="counter", text_size=2.0, weight=1.0),
           flow="row", h=40.0, gap=6.0),

    title="Clausters multitrack", w=1100, h=640, flow="col"))
print(f"opened window {win} -- menu bar, toolbar, ruler, {3} lanes, transport")

# %% [markdown]
# ## The transport
# `Transport` drives the `Playhead` and the view's line together: `play` anchors
# every lane's playhead to the engine clock (the host sweeps it, one message per
# pass), `pause` parks the static cursor on what stopped, `stop` rewinds. Its
# ``ids`` are read on each use, so the three lanes all carry the line.

# %%
def start_pass(at: float, **kw):
    """Begin a pass at beat ``at``. The transport calls this on **every** play,
    so the timeline is rebuilt from the arrangement each time — which is how a
    clip dragged a beat later, or a lane muted, is heard on the next play."""
    head = Playhead(build_timeline(), session.clock, server)
    head.play(at=at)
    return head


# `clock` is what lets the line cross the **last** clip: a scan runs out when it
# renders its last item, and the piece ends where that item ends, so the
# transport sweeps the tail on the clock rather than parking the cursor early.
transport = Transport(gui, lambda: [win[name].id for name in LANES],
                      source=start_pass, tempo=TEMPO, sample_rate=SR,
                      extent=extent, clock=session.clock)
transport.server = server


def follow():
    """Re-schedule what is playing from where it is. An edit does not interrupt
    the sound by itself (the playhead is scanning a list it already has), so a
    driver that wants the edit *now* plays again from the current position — the
    live-editing loop, and the reason a mute takes effect on the beat it is
    clicked rather than on the next play."""
    if transport.playing:
        transport.play(at=transport.position)

# %% [markdown]
# ## The view state, and the two faces of it
# The menu bar and the toolbar show the same three options, so each handler
# applies the change *and* echoes it into the other widget. That echo is a plain
# `set`: the host owns a widget's value, this script owns what the value means.

# %%
state = {"ruler": True, "unit": 0, "colors": 0, "lane_h": LANE_H}


def show_ruler(shown: bool):
    """Show or hide the time ruler. The strip keeps its place in the tree and
    its navigation group; only its thickness goes to zero."""
    state["ruler"] = bool(shown)
    win["ruler"].set(h=22.0 if shown else 0.0)
    win["m_ruler"].set(index=0 if shown else 1)
    win["t_ruler"].set(value=1 if shown else 0)


def set_unit(index: int):
    """Pick the time unit, and with it the form the counter brackets below.

    Only the free-standing ruler carries it: a lane's own ``unit`` *is* its
    ruler prop, and a strip is reserved out of that lane's height the moment it
    is set — three lanes, three rulers, all saying what the one above them
    already says. The lanes keep the rest of the axis (the tempo, the grid) and
    stay bare."""
    state["unit"] = int(index) % len(UNITS)
    unit = UNITS[state["unit"]]
    win["ruler"].set(axes={"x": {"unit": unit}})
    win["m_unit"].set(index=state["unit"])
    win["t_unit"].set(index=state["unit"])


def set_lane_h(px: float):
    """How thick a lane is. Not a zoom: the plane's own zoom is uniform over
    both axes, so growing the lanes with it would stretch the time axis out from
    under the ruler. A lane's thickness is its `h` on the wire, and the scrolled
    content area grows with it — which is what gives the wheel somewhere to go."""
    state["lane_h"] = min(max(48.0, float(px)), 400.0)
    for name in LANES:
        win[name].set(h=state["lane_h"])
    win["editor"].set(content_h=len(LANES) * state["lane_h"] + 8.0)
    win["t_laneh"].set(value=state["lane_h"])


def set_colors(index: int):
    """The spectral clip's colormap. A clip's display props are the take's, so
    this is addressed to the clip — the same `set` any other clip prop takes."""
    state["colors"] = int(index) % len(COLORMAPS)
    win["sweep"].set(colormap=state["colors"])
    win["m_colors"].set(index=state["colors"])


win["m_ruler"].on_event(lambda i, *_: show_ruler(int(i) == 0))
win["t_ruler"].on_event(lambda v, *_: show_ruler(bool(int(v))))
win["m_unit"].on_event(lambda i, *_: set_unit(int(i)))
win["t_unit"].on_event(lambda i, *_: set_unit(int(i)))
win["m_colors"].on_event(lambda i, *_: set_colors(int(i)))
win["t_laneh"].on_event(lambda v, *_: set_lane_h(float(v)))

# %% [markdown]
# ## The edits that change the piece
# Everything above changes how the composition is *shown*. These two change the
# composition: a clip dragged or resized writes its new placement into
# `CLIPS`, and a lane's mute, solo or fader writes into `LANE_STATE` — the two
# tables the next pass is built from. Each handler ends in `follow`, so what is
# playing is re-scheduled from where it is and the edit is heard immediately.
#
# A lane's controls are the host's own gesture, so the value is already drawn;
# echoing it back with `set` is what keeps the widget and this script's idea of
# it from drifting when the script is the one that changes it.

# %%
def on_clip(name: str):
    """A clip's move/resize: ``"clip" offset dur`` in timeline samples."""
    def handler(tag, *vals):
        if tag != "clip" or len(vals) < 2:
            return
        CLIPS[name]["at"] = float(vals[0]) / BEAT
        CLIPS[name]["beats"] = max(float(vals[1]) / BEAT, 0.0)
        print(f"clip {name}: {CLIPS[name]['at']:.2f} .. "
              f"{CLIPS[name]['at'] + CLIPS[name]['beats']:.2f} beats")
        follow()
    return handler


def on_lane(name: str):
    """A lane's edit-backs: its header controls (``"mute"``/``"solo"`` 0/1,
    ``"level"``) and its thickness (``"height"``, from Ctrl+wheel over it)."""
    def handler(tag, *vals):
        if not vals:
            return
        if tag == "height":
            # The host resized the lane under the cursor; this stack wants one
            # thickness, so the number goes to all of them (and to the control
            # that also sets it). Which lanes follow is the driver's call —
            # that is why the host says what happened instead of deciding.
            set_lane_h(float(vals[0]))
            return
        if tag in ("mute", "solo"):
            LANE_STATE[name][tag] = bool(int(vals[0]))
            win[name].set(**{tag: 1 if LANE_STATE[name][tag] else 0})
        elif tag == "level":
            LANE_STATE[name]["level"] = float(vals[0])
            win[name].set(level=LANE_STATE[name]["level"])
        else:
            return
        st = LANE_STATE[name]
        print(f"lane {name}: mute {int(st['mute'])} solo {int(st['solo'])} "
              f"level {st['level']:.2f} -> gain {lane_gain(name):.2f}")
        follow()
    return handler


for _name in CLIPS:
    win[_name].on_event(on_clip(_name))
for _name in LANES:
    win[_name].on_event(on_lane(_name))

# %% [markdown]
# ## The transport's buttons, and the ruler as a scrub bar
# A `button` emits ``1`` on press and ``0`` on release, so each handler acts on
# the press. A press on the ruler emits ``"locate"`` in timeline units — the same
# seek a DAW does when you click its ruler.

# %%
def play_pause(pressed):
    if not int(pressed):
        return
    if transport.playing:
        transport.pause()
    else:
        transport.play(server)


def on_ruler(tag, *vals):
    if tag == "locate" and vals:
        transport.locate(float(vals[0]) / BEAT)


win["b_play"].on_event(play_pause)
win["b_stop"].on_event(lambda v, *_: int(v) and transport.stop())
win["b_rew"].on_event(lambda v, *_: int(v) and transport.locate(0.0))
win["ruler"].on_event(on_ruler)

_closed = False
win.on_closed(lambda: globals().__setitem__("_closed", True))

# %% [markdown]
# ## The counter
# The same position in every form at once — bars.beats, beats, seconds and
# samples — with the unit the menus chose in brackets, so the read-out and the
# ruler always agree about what is being measured.

# %%
def readout() -> str:
    """The transport's position, in all of its forms."""
    beats = transport.position
    bars, in_bar = divmod(beats, BAR)
    forms = {"beats": f"{beats:8.3f} beats",
             "time": f"{beats / TEMPO:8.3f} s",
             "samples": f"{int(beats * BEAT):10d} smp"}
    shown = UNITS[state["unit"]]
    return f"{int(bars) + 1:3d}.{in_bar + 1:06.3f}   " + "   ".join(
        f"[{text.strip()}]" if unit == shown else text
        for unit, text in forms.items())


def run(seconds: float | None = None) -> None:
    """Drain the host's events and keep the counter live for ``seconds``.

    Script-run there is no bound and the window is what ends it; the
    ``seconds`` argument is for a cell run, where a notebook wants the loop to
    give the prompt back.
    """
    start = time.monotonic()
    shown = None
    while not _closed and (seconds is None or time.monotonic() - start < seconds):
        gui.pump(timeout=0.05)
        transport.update()          # parks the cursor when the piece ends
        text = readout()
        if text != shown:           # a stopped transport sends nothing
            win["counter"].set(text=text)
            shown = text


set_unit(0)
show_ruler(True)
transport.locate(0.0)
win["counter"].set(text=readout())

# %%
if __name__ == "__main__" and not hasattr(sys, "ps1"):
    try:
        run()
    finally:
        session.close()
    sys.exit(0)
else:
    print("editor up - transport.play(server), run(10) to drive it, "
          "session.close() to end")
