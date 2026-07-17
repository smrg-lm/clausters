# Automation: a curve as an element

The piece gets its fourth lane: a frequency sweep — a break-point curve
driving a voice — placed in the composition like everything else. This page
carries three ideas: an **`Automation`** is a curve rendered as a control
signal; a curve and the voice it shapes can be **grouped into one clip**; and
the curve is **edited in place**, on the lane, with the same
gesture → `poll()` → `play()` rhythm as a clip.

## The voice: an instrument that reads a bus

The sweep drives a drone's frequency. The def reads its frequency **straight
from a control bus** — the one the automation will write — and is gated, so
the event's length is the voice's length:

```python
from clausters.defs import (DoneAction, Env, SynthDef, control, env_gen,
                            in_ctl, out, sine)

def drone(name: str = "drone") -> SynthDef:
    """A sine whose frequency is read from a control bus, held by a gate and
    freed on release — its life is the event's."""
    bus = control("freq_bus", 0.0, "ir")
    amp = control("amp", 0.12, "kr")
    gate = control("gate", 1.0, "kr")
    shape = env_gen(Env.asr(attack=0.05, release=0.4), gate=gate,
                    done_action=DoneAction.FREE_SELF)
    sig = sine(in_ctl(bus)) * shape * amp
    return SynthDef(name, out(0.0, sig), out(1.0, sig))

server.add_synthdef(drone())
```

## The curve: an `Automation`

```python
from clausters.seq import Automation

SWEEP = 4.0     # the sweep's length in beats (the curve's, and its voice's)

sweep = Automation.from_points(
    [(0.0, 200.0, 1, 0.0),      # 200 Hz ...
     (2.0, 900.0, 2, 0.0),      # ... up to 900 (segment shape 2: exponential)
     (4.0, 300.0, 1, 0.0)],     # ... back down (shape 1: linear)
    target=None, name="freq")
sweep.prepare(server)            # allocate + fill its buffer and bus, once
print(sweep.duration(), sweep.bus.index)
```

Break-points are `(time, value, shape, curve)` — times in beats, values in the
control's real units (Hertz here); the stored curve is an `Env`, the same
object the envelope editor round-trips. How it is rendered is worth knowing,
because it is all server machinery you already have: the curve is discretized
into a **control buffer** on the server (`/b_gen "env"`, evaluated through the
same envelope math the `EnvGen` UGen plays — what is drawn is what is heard),
and at play time a small internal synth reads that buffer onto a **control
bus** over the curve's duration. A target node would follow the bus via
`/n_map`; our drone simply reads the bus itself, so `target=None` — the
automation just writes its bus.

The two-phase shape is deliberate: **`prepare(server)` blocks** (it allocates
and fills the buffer), so it runs here, at the top level, once. Playing —
which happens on the clock thread — only *schedules*, and never blocks. The
same golden rule as everywhere else in the client.

## One clip: the envelope attached to the voice it shapes

The voice is an event **in the composition** — not a synth held by your
session — reading the automation's bus:

```python
from clausters.form import Element, Event, Group
from clausters.seq.event import Event as SeqEvent

voice = Event(SeqEvent(instrument="drone", freq_bus=sweep.bus.index,
                       dur=SWEEP, legato=1.0, amp=0.12, has_gate=True))
sweep_clip = Group([(0.0, voice), (0.0, Element(sweep, duration=SWEEP))],
                   name="sweep")
print(sweep_clip.temporal_relation())    # 'simultaneous'
```

Look at what was just said in model terms: two members that start and end
together — the group's derived relation is **simultaneous**, and the grouping
page promised this moment. A simultaneous group is *one thing on the
timeline*, so the editor draws it as **one clip with layered bodies**: the
curve over the note, dragging as one. The voice cannot outlive its envelope,
and the envelope cannot be left behind.

(The bare `Element(sweep, duration=SWEEP)` is the escape hatch: an
`Automation` already knows how to `play(destination)`, so a plain `Element`
adorning it with a duration is enough — no sixth primitive needed.)

Add the lane and show it:

```python
song.add(Group([(0.0, sweep_clip)], name="sweep"), offset=0.0)
editor.update()      # a structural change: the window grows a fourth lane
```

```python
editor.play(server, session.clock, at=0.0)
```

Four beats of drone slide 200 → 900 → 300 Hz under the piece. Note what the
clip's picture means: **a curve's floor is its parameter's minimum**, nothing
more — this envelope touching the bottom of its clip is the *lowest
frequency*, not silence. Draw a curve over `amp` instead and the bottom would
be silence, and that silence part of the clip's length. Same clip, same
curve; what it means is the control it drives.

And because the voice is an event with a length, placed in the tree: seek
past the clip (`editor.locate(6.0)` and play) and there is simply no drone. A
synth still humming over empty timeline is not a drone, it is a leak — this
piece cannot leak.

## Edit the curve in place

The curve's break-points are live on the lane:

- **drag a point** to move it (a point wins over the clip body — the clip
  still drags by its empty space);
- **Ctrl+click** on empty curve to add a point;
- **Ctrl+click on a point** to remove it.

Pull the middle point up toward 1500 Hz, then the rhythm you know:

```python
editor.poll()               # the points land on the automation's Env
print(sweep.to_points())    # the curve, as (t, v, shape, curve) — as drawn
sweep.prepare(server)       # re-fill the control buffer from the edited Env
editor.play(at=0.0)
```

The edit went back onto `sweep.env` itself — the `Env` is the single source of
truth shared by the picture and the data. One step is still yours today: the
**server-side control buffer** was filled from the `Env` when you first
`prepare`d it, and rendering schedules the lane synth over that buffer
without re-filling it — so after a curve edit, call `prepare(server)` again
(it re-runs the `/b_gen` on the buffer it already owns; cheap, and safe at the
top level). Skip it and the next play sounds the curve as it was at the last
`prepare`, not as drawn.

The concrete side of the piece is now complete: four lanes, three element
kinds, one automation, all editable from either side of the loop.
One grouping kind remains.

Next: [The logical side: groups as signal graphs](logical.md).
