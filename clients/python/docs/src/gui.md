# The visual elements: a GUI the script builds

A GUI in Clausters is a **def**, like an instrument. You build a tree of nodes,
send it in one message, and from then on you mutate live widgets one at a time.
Nothing is compiled, nothing is subclassed, and the window is a separate
process — the **GUI host** — that owns the pixels and talks OSC.

This page builds one from nothing and then walks every element there is,
finishing with an instrument that keeps its GUI after the script is gone.

Install once, from the repo root:

```sh
python -m venv .venv
.venv/bin/pip install -e ./clients/python      # bundles the GUI binary
```

Everything below needs a display and a GPU adapter.

## A window of controls

Start the host and open a window. `GuiHost().boot()` launches a
`clausters-gui` process and connects to it; no audio server is involved yet.

```python
from clausters.gui import GuiHost, button, knob, label, menu, panel, slider, window

gui = GuiHost().boot()

win = gui.open(window(
    label("a filter", h=24),
    panel(knob(name="cutoff", label="cutoff", min=20.0, max=20000.0, value=800.0),
          knob(name="res", label="res", min=0.0, max=1.0, value=0.3),
          layout="row"),
    panel(slider(name="mix", label="mix", min=0.0, max=1.0, value=0.5),
          button(name="reset", label="reset"),
          menu(name="wave", options=["sine", "saw", "square"], index=0),
          layout="row", h=40),
    title="filter", w=520, h=260, layout="col"))
```

A window appears. Two things about that tree are worth noticing before
anything else.

**No widget has an id.** You passed `name=` instead, and `open` hands back a
window handle you index by name — `win["cutoff"]`. The host does use integer
ids on the wire, and `open` allocated them for you; a name is client-side only
and never travels. Unlike an id (which recycles when a window is freed), a name
is stable, which is what a live edit addresses against.

**The children came first.** The positional slot of every builder belongs to
what the widget is made of — a container's children, a label's text, a menu's
options — so an ordinary tree mentions no ids at all.

### Driving it, and listening

Setting a prop is one call, and it is live:

```python
win["cutoff"].set(value=2000.0, label="freq")
```

Reading the user's side is a responder, in the same shape the audio client uses
for OSC:

```python
win["cutoff"].on_event(lambda v: print("cutoff ->", v))
win["reset"].on_event(lambda v: print("reset", v))

gui.pump(timeout=5.0)      # deliver whatever arrived, for five seconds
```

Turn the knob and click the button: the values print. Nothing is delivered
until you pump — the host's messages queue up, and the script decides when it
is listening, exactly as it does for the audio server.

## What a `type` names

Every node is `{id, type, props, children}`, and a `type` names one of three
things:

| Kind | What it is | Types |
|---|---|---|
| **Container** | owns 0, 1 or 2 **axes**, and so a coordinate system its children are placed in | `window`, `layout`, `plane`, `field` |
| **Element** | draws against the axes of the container holding it | `signal`, `notes`, `curve`, `score`, `keys`, `nodes`, `meter`, `canvas`, `label` |
| **Control** | an element with a value and no axis | `slider`, `knob`, `number`, `button`, `toggle`, `text`, `menu` |

That is the whole vocabulary. The builders you just used — `panel`, and later
`waveform`, `track`, `scope` — are **shortcuts**: each builds one of those
nodes with the props of one common case. `layout`, `plane`, `field` and
`signal` are there beside them for the cases no shortcut names, and you can mix
the two freely in one tree.

Two consequences worth having in mind while reading on. A **ruler, a navigation
window, a selection, a playhead and a value range belong to a container's
axes**, not to each element drawn against them — which is why several views can
share one time axis without any of them owning it. And an **element's
capabilities are props**: whether a view navigates, carries a selection or
edits back is a choice over any presentation, not a different kind of widget.

From here on the snippets are **trees**, not whole scripts: each one is
something you pass to `gui.open(window(...))` on the host you already booted.

## The keyboard, and where it points

One widget at a time holds the **focus**, and it is the only one keys reach.
Click a `text` field to focus it and type; press **Tab** to walk the window's
focusable widgets in the order they are laid out, **Shift+Tab** back along
them. The focused widget wears a ring in the theme's `focus` role, so where the
keyboard is pointing is always visible.

A script can point it too — for a field that should be ready to type into the
moment its window opens:

```python
win["name"].focus()             # ...and .focus(False) gives it up
win["name"].on_event(lambda *a: print(a))
# ('focus', 1) when it gains the keyboard, ('focus', 0) when it loses it
```

Both ends of every move **the user makes** are reported, so a script that mirrors
the focus hears about the widget that lost it as well as the one that gained it.
Your own `focus()` is not echoed back — no `set` is, since you already know what
you asked for. A widget that reads no keyboard refuses the focus rather than
swallowing it.

**Tab past the last widget hands the keyboard back.** A window in a web page
sits inside a document, and a ring that wrapped would trap the keyboard in the
canvas and leave the page around it unreachable — so the ring runs out, and the
browser's own tab order carries on. On the desktop nothing is focused and the
next Tab enters the ring again.

## Arranging: `layout` and its flows

A `layout` has no axes; it arranges its children by `flow`:

```python
from clausters.gui import layout

layout(a, b, c, flow="row")               # side by side
layout(a, b, c, flow="col")               # stacked (the default)
layout(a, b, c, flow="grid", cols=2)      # a fixed grid
layout(a, b, flow="free")                 # x/y place the children absolutely
```

`panel(...)` is the same node — it takes `layout=` as the old spelling of
`flow=`, which is why the first window read `layout="row"`.

Every widget, whatever its type, may carry the **place props**: `w`, `h`,
`weight`, and `x`/`y` inside a `free` parent. On a row's or a column's main
axis the size resolves in one order — an explicit size wins, then an explicit
`weight` takes that share of the leftover, then the widget's **natural size**
(how big that kind wants to be), then an equal share. So a column of controls
is a stack of control-high rows, and `weight=1.0` is what stretches something
past its natural size:

```python
window(
    panel(label("transport"), h=28),      # chrome, at the height it names
    waveform(name="take", data=take, weight=1.0),   # the work surface takes the rest
    layout="col", margin=8.0, gap=6.0)
```

A container has no natural size of its own — it takes what it is given —
unless it carries **`hug`**, and then it wants exactly what it holds: a row
adds its children up along its axis and takes the tallest of them across it, a
column the other way round, a grid counts its cells. That is the strip above
without the number:

```python
window(
    panel(label("transport"), button(label="play"), layout="row", hug=True),
    waveform(name="take", data=take, weight=1.0),
    layout="col", margin=8.0, gap=6.0)
```

`hug` is asked of the whole subtree, so a plain panel nested inside a hugging
one is measured too, and an axis a child leaves elastic — a plane, a lane, a
heavy view — is one the container hands back to the layout. On the `window`
itself it sizes the window: a window holding one knob opens knob-sized instead
of putting a strip at the top of an empty pane.

What a size may read is fixed by **where the value is resolved**. A prop that
settles when you build it or `set` it may size a hugging container — a label's
text, a menu's options. A **value** never sizes anything: the option a menu is
on, what a field holds, what a number reads, a view's samples. So a stream of
values cannot move a layout, and a control does not resize under the gesture
writing it.

A container additionally takes `margin` (the inset before its children), `gap`
(between them) and `cols`.

### One child at a time: tabs

`flow="stack"` shows a single child — the one `index` names — and neither lays
out nor draws the others:

```python
from clausters.gui import stack, spectrogram, waveform

stack(waveform(name="wave", data=take),
      spectrogram(data=take),
      name="pages", index=0)
```

The hidden page stays in the tree, so a heavy view keeps its GPU slot and its
bus reads across a switch and comes back without re-uploading anything. Flip it
from the script with `win["pages"].set(index=1)` — or, better, let a control do
it without the script at all.

## Values that never come back to the script

A widget can be **bound**, and then its value bypasses this process entirely.

To the **audio server** — the low-latency path, since nothing waits on Python's
scheduler:

```python
win["cutoff"].bind("/node_set", 1000, "freq")
```

Now turning that knob sends `/node_set 1000 freq <value>` straight from the
host. To **another widget** — its value lands on that widget's prop:

```python
win["picker"].bind_widget(win["pages"], "index")
```

A `menu` bound to a stack's `index` *is* a tab bar: the pages flip inside the
host, and nothing prints here while you click. A binding fires an **apply,
never another binding**, so two widgets wired to each other settle instead of
cascading.

## The signal element: every view of a signal

Six names in the old catalog — a waveform, a plot, an oscilloscope, a
spectroscope, a spectrogram, a goniometer — were six points of one element:

```python
signal(view=…, <source>, navigable=…, selectable=…, editable=…, measure=…)
```

- **`view`** is the presentation: `"trace"` (value against time, the default),
  `"spectrum"` (magnitude against frequency), `"spectrogram"` (the STFT,
  magnitude against time *and* frequency), `"phase"` (the goniometer of a
  stereo pair).
- **the source** is either a `bus` (with `rate`), read forward-only, or
  addressable samples — `data`, `blob`, `buffer`, `path`, `cache` — which is
  what lets a view navigate, slice and select.
- **the capabilities** are `navigable`, `selectable`, `editable`.
- **`measure`** is what the picture measures: `"peak"` (the default — the
  min/max envelope the signal reached) or `"rms"` (the symmetric body of the
  level it held).

### What a picture measures

A measure is a *factor* of the view, not a widget of its own — and a view may
name more than one, which is the classic editor picture: the RMS body drawn
inside the peak envelope.

```python
from clausters.gui import waveform

waveform(cache="take.clpk", measure="peak rms", label="the take")
```

One view and not two, because **every view of a signal paints its own field
before it draws**: two of them on one rectangle do not layer, the second hides
the first. Measuring twice into one body is also what keeps the rest single —
one axis, one ruler, one selection, one playhead, one upload of the samples —
and the order is the host's: the envelope is the outer shape, so it goes under
whatever order you name them in.

Three things follow from what the measure is:

- **The level is averaged over a fixed 50 ms of the source**, not over the pixel
  column it is drawn in — 2400 samples at 48 kHz, the RMS window an audio editor
  defaults to. A root-mean-square is an average over a *duration*, so a window
  that followed the column would make the body's own values follow the **zoom**:
  the level would move while you navigated samples that had not changed. This
  way it stands still. (A live bus view, whose window is already stated in
  milliseconds and whose rate the host does not know, averages what each column
  covers.)
- **The body disappears when the envelope comes down onto it.** The envelope
  narrows as you zoom in, since a column covers less of the wave; once it is
  within a fifth of the level the two are saying the same thing, so the layer
  goes — at full weight, in one step, rather than fading. That is also what
  keeps it from ever showing outside the envelope that contains it. It is the
  behavior an audio editor's RMS layer has: it is a reading of the overview, and
  zooming in to work leaves you the wave.
- The body costs no second pass over the samples. The mean square rides in the
  peak cache beside the min and max, at every resolution level, so the body
  reads the same mapped file the envelope does.

A take you are **recording into** wants one more prop, `fills`:

```python
waveform(buffer=take.bufnum, sample_rate=rate, fills=True)   # ...and fills=False when it stops
```

It says the samples are being written as they are drawn, so the view stops at the
buffer's write frontier and leaves the axis past it empty. Without it the
buffer's own zeros are drawn — the minimum-ink rule puts a flat line across the
whole take before anything has been recorded into it — because past the frontier
there is no silence, there is no samples yet. The host cannot work this out for
itself: a frontier alone does not tell a recording from a loaded take that one
write touched, and you are the one who allocated the buffer.
- **A cache built before the measure existed draws no body.** Its energy was
  never measured, and zeros would say silence over samples that is not
  silent — so the layer is simply absent, and rebuilding the cache
  (`peaks_cache_file`) is what fills it in.

The shortcuts name the six common points, and the props of each are documented
with them in the [API reference](api.md):

```python
from clausters.gui import (phasescope, plot, scope, spectrogram, spectrum, waveform)

waveform(path="take.f32", channels=2)     # signal(view="trace"), navigable
plot(data=seq)                            # signal(view="trace", navigable=0)
scope(0, channels=2, trigger=0.05)        # signal(view="trace", bus=0)
spectrum(0, freq_scale="mel")             # signal(view="spectrum", bus=0)
spectrogram(path="take.f32")              # signal(view="spectrogram")
phasescope(0)                             # signal(view="phase", bus=0)
```

They are shortcuts, not the catalog: a point the six names never froze is
written by naming the point, not by finding a name for it —

```python
from clausters.gui import signal

signal(view="spectrogram", bus=0, retention=8.0, navigable=True)   # a waterfall
```

### What `navigable` navigates

An axis is navigable when its domain is **addressable**, and the two live axes
reach that differently:

- **time**, on a forward-only source, is not addressable at all: there is
  nothing behind the newest window to zoom out to. `retention` (in seconds) is
  the policy that supplies a past — the host keeps that many seconds of the bus
  and the view reads *that* — which is what makes the waterfall above a
  spectrogram you can zoom and pan like a file's. A live axis follows the
  newest until you navigate it, and then stays where you put it.
- **frequency** is addressable with no history at all: every bin is there every
  frame. So `navigable` over a `spectrum` costs nothing but the gesture — drag
  the curve to pan its frequency axis, wheel over it to zoom under the cursor,
  `R` to see all of it again:

  ```python
  spectrum(0, navigable=True, view_start=0.5, view_len=0.5)   # the top half
  ```

  That window is the element's **own** — normalized over `[0, Nyquist]`,
  `0, 1` being the whole axis — not a navigation group's: nothing else in a
  window measures in hertz along x, so there is no axis to share. It is live
  through `set(view_start=…, view_len=…)` and comes back as a `"view_x"` event,
  the horizontal sibling of the `"view_y"` every other element's vertical axis
  reports. `spectrum` is also the one view where `navigable` is off unless you
  ask: without it, it is the watching spectroscope.

  The zoom stops where the **analysis** does. A window narrower than a few FFT
  bins across the whole widget shows the interpolation between two neighbouring
  bins — a straight line that no longer answers to the signal — so the host
  floors it at that resolution, computed from `fft_size` and the sample rate.
  It is not a fixed fraction of the axis, because a bin is not one: on a log
  axis one is a twentieth of what you see at 500 Hz and a thousandth of it near
  Nyquist. Raise `fft_size` to zoom further; that is the only thing that buys
  more detail, here as anywhere.

  Because the floor moves with the window, the axis distinguishes the window you
  **asked** for from the one it can **show** you. `view_start`/`view_len` are the
  request — set from the script or by a gesture, it makes no difference — and the
  axis opens it wherever it is finer than the bins there. So panning down towards
  20 Hz widens the picture on its own, and panning back up hands your window
  back: the trip does not spend the zoom. A `set(view_len=…)` finer than the
  analysis is honoured the same way — drawn, and reported as `"view_x"`, opened
  up — so what you read back is always what is on the screen.

  And an axis at a bound is quiet: once the window cannot move, the wheel
  reports no `"view_x"` at all. That holds for every view window — `"view"` and
  `"view_y"` too — so a handler counting events is counting movements, not
  notches.

### Where the samples come from

A view never carries a megabyte over OSC if it does not have to. In the host's
precedence order:

| Prop | What it is |
|---|---|
| `cache` | a prebuilt peak-pyramid file the host maps and renders directly — the most compact path, and the raw samples are never loaded |
| `path` | a file of raw little-endian `f32` the host maps |
| `buffer` | a server buffer number, pulled over the host's client leg |
| `data` | a short list of floats, inline in the JSON |
| `blob` | a binary blob carried beside the JSON in the same message |

`samples_to_file` and `peaks_cache_file` write the first two from a Python
sequence; `samples_to_blob` packs the last. A cache follows its samples rather
than being rebuilt: `peaks_cache_update_file` re-summarizes the frame span an
edit touched, and `peaks_cache_stream_file` folds a `/buffer_stream.reply`
report into it — the overview of a take being recorded, measured by the writer
and sent instead of the samples, so a picture mapping the file grows as the take
does.

### The axes carry the chrome

A ruler, the visible window, the selection, the playhead and the value range
describe the container's **axes**. The shortcuts take them as flat keywords and
pack them for you:

```python
waveform(path="take.f32", ruler="beats", tempo=2.0, quant=4.0,
         sel_start=48000.0, sel_len=24000.0, ruler_y="db", link=1)
```

and the general builders take the pair directly, which is what goes on the wire:

```python
signal(view="trace", path="take.f32",
       axes={"x": {"unit": "beats", "tempo": 2.0, "quant": 4.0, "link": 1},
             "y": {"unit": "db", "min": -1.0, "max": 1.0}})
```

On the **x** axis: `unit` (`"time"`/`"samples"`/`"beats"`/`"off"`),
`start`/`len` (the navigation window), `tempo`/`beat_at`/`quant`,
`sample_rate`, `link`, `sel_start`/`sel_len` and the `playhead` family. On
**y**: `unit` (`"norm"`/`"db"`/`"bits"`/`"percent"`/`"hz"`/`"off"`),
`start`/`len`, `min`/`max`, `bit_depth`, `sel_min`/`sel_max`.

**`link` is the interesting one.** Views naming the same `link` id form a
**navigation group**: one horizontal window, one selection, one playhead,
shared. Zoom, pan or drag a selection on any member and all of them move —
including the ones nobody is looking at, since membership is read from the tree
and not from what is on screen. Only the vertical window stays per view, on
purpose: a waveform's y is amplitude and a spectrogram's is frequency, and no
single number could mean both. The selection's **value band** stays per view for
the same reason — a sweep with height on a waveform restricts that view to a
range of amplitudes and leaves the spectrogram beside it showing the whole
band, while the time span they share moves for both.

```python
window(
    waveform(name="wave", data=take, ruler="time", link=1),
    spectrogram(data=take, link=1),
    layout="col")
```

Scroll one; the other follows.

## Fields: lanes, clips and a ruler

A `field` is the container with **two independent axes** — time against
whatever the elements on it measure. One container, told apart by what is on
it:

- holding other fields it is a **lane** (`track`),
- carrying `offset`/`dur` it is a **clip** placed on its parent's x axis,
- a bare strip of a given `h` with nothing on it is the free-standing **ruler**
  (`timeruler`).

```python
from clausters.gui import clip, timeruler, track, window

BEAT = 24_000.0          # samples per beat at 48 kHz, two beats a second

win = gui.open(window(
    track(clip(name="a", offset=0.0, dur=4 * BEAT, data=take, label="take"),
          clip(name="b", offset=4 * BEAT, dur=2 * BEAT,
               notes=[(0.0, BEAT, 60), (BEAT, BEAT, 67)]),
          label="drums", link=1),
    track(clip(offset=0.0, dur=6 * BEAT,
               points=[(0.0, 0.0), (3 * BEAT, 1.0, "exp"), (6 * BEAT, 0.0)]),
          label="filter", link=1),
    timeruler(link=1, ruler="beats", tempo=2.0, h=22.0),
    title="a multitrack", w=900, h=420, layout="col"))
```

A clip's **bodies layer**: a take, note events over it, an automation curve
over both — and each keeps its own value axis, so a clip carrying notes does
not draw its take against a pitch range. Give a clip a body prop it does not
have yet and the body grows, which is how a curve is drawn over a take without
rebuilding the def:

```python
win["a"].set(points=[0.0, 0.0, 1, 0.0, 4 * BEAT, 1.0, 1, 0.0])
```

A list or a dict passed to `set` is serialized for you: OSC has no structural
argument, so an `axes` pair, a `theme` table and a list of `points` or `notes`
all ride as their JSON string. What a **live** set takes is the flat wire form
— `t v shape curve` per break-point, `start dur pitch velocity channel` per
note — because converting the friendlier tuples is the *builder's* job, and a
`set` names a prop without knowing what it means.

The ruler is its **own** strip under the stack rather than one lane's `ruler`,
because a lane's ruler is reserved out of that lane's height — ruling a stack
that way costs the bottom lane a strip of itself. A ruler with no `link` joins
the window's lanes on its own.

### Edits come back as intents

Drag a clip, or its edge. The host draws the move as it happens and, on
release, emits what you did — not pixels:

```python
win["a"].on_event(lambda tag, *rest: print(tag, rest))
# "clip" (offset, dur)  when a clip is moved or resized
# "locate" (position)   when the ruler or empty lane space is clicked
# "view" (start, len)   when the axis is zoomed or panned
# "selection" (start, len[, min, max])   the span, and the value band a
#                                        sweep with height restricted it to
# "cut" (start, len)                     Ctrl+X: cut this span, says the host
# "paste" (position, kind, json, blob…)  Ctrl+V, with the clipboard beside it
# "refused" (verb, reason)               the host could not do its own half
# "notes" / "points"    when a roll or a curve is edited
# "mute" / "solo" / "level"  from a lane header's controls
```

The host holds geometry, never your document: it tells you what was asked for,
in *your* units, and you apply it and send back a fresh drawing. That is what
lets one renderer host editing for data it cannot interpret.

## The rest of the elements

```python
from clausters.gui import (bpf, canvas, curve, keys, meter, nodes, nodetree,
                           notes, piano, pianoroll, score)
```

| Element | Shortcut | What it is |
|---|---|---|
| `notes` | `pianoroll` | the editor-grade piano-roll: a keyboard, a MIDI-note grid, a velocity lane and an OSC-event lane. `notes=[(start, dur, pitch[, vel[, chan]])]`, `osc=[(time, label)]`, `min`/`max` the pitch window, `snap` the grid. `midi_in=True` arms live MIDI painting in the native host |
| `curve` | `bpf` | a drawable break-point envelope, played by the server's own shape math. `points=[(t, v[, shape])]` or an `Env` through `env_to_points`; edits come back as `"points"` |
| `keys` | `piano` | a playable keyboard with real piano proportions. `min`/`max` are the visible range (its overview strip pans and zooms it), `active_min`/`active_max` gray the keys outside a mapping, and `voice="def"` has the **host** manage one server voice per held key |
| `nodes` | `nodetree` | the audio server's node graph, live, with each synth's controls |
| `meter` | — | a bus level, read from the server's shared segment every frame with no messages at all |
| `score` | — | an engraved notation page: the client engraves and sends a display list, the host fits and tessellates it. A click names an element, a drag transposes it |
| `canvas` | — | a script-supplied WGSL shader over the widget area, fed by `params` and by control `buses` |
| `label` | — | static text: `text_size`, `wrap`, `align` |

`notes`, `curve`, `nodes` and `keys` are the model's names; `pianoroll`,
`bpf`, `nodetree` and `piano` are the same builder under the name the catalog
used.

## Planes: a workspace and a patcher

A `plane` is the container with **two axes locked to one scale** — a pannable,
zoomable plane in content units. Drag the empty background to pan, wheel to
zoom anchored at the cursor:

```python
from clausters.gui import plane, scroll

scroll(panel(x=0.0, y=0.0, w=200.0, h=1200.0),
       axis="y", zoom=False)          # a plain vertical scroll view
plane(a, b, axis="both")              # the full free plane
```

The constrained scroll views are configurations of it, not different
containers. With `boxes` and `cords` the same plane is the **patcher** —
boxes with inlets on top and outlets on the bottom, a cord per connection,
coloured by rate:

```python
from clausters.gui import patch

patch(boxes=[{"def": "source", "inlets": [], "outlets": ["out"]},
             {"def": "sink", "inlets": ["in", {"name": "gain", "rate": "control"}],
              "outlets": []}],
      cords=[0, 0, 1, 0])
```

Drawing a cord emits `"wire"`, dragging a box emits `"move"` — the driver owns
the graph and the geometry, and re-renders.

## Style, and what a drag means

Every chrome colour is a named role in one theme. Any container may carry a
`theme` — a partial table overlaying its parent's for the whole subtree, a
**theme group** — and any widget a single `color` that re-seeds the roles
carrying its function:

```python
panel(knob(color="#40c0a0"), theme={"panel_fill": "#101018", "accent": "#c08040"})
```

Both are live through `set` (a theme rides as its JSON string), and a theme on
the root persists with a named def, so a bundle ships its look.

Two props shape the drawing rather than its colour, both on any widget and both
live: `opacity` (`0`–`1`) and `radius` (a corner radius in logical pixels).

```python
panel(button(label="arm", radius=6), opacity=0.5)
```

`opacity` behaves like a theme group — it multiplies down the whole subtree, so
a control at `0.5` inside a panel at `0.5` draws at `0.25` — while `radius`
applies to the widget alone; a negative number clears either. Each box clamps
the radius to half its shorter side, so a widget's own frame rounds and the
hairlines inside it keep their shape. What the fade covers is the flat drawing:
the chrome, the controls and the text. A heavy view's picture — a waveform's
trace, a spectrogram's texture, a `canvas` shader — is drawn by its own
pipeline and keeps its own opacity, and two overlapping shapes inside a faded
widget show through each other, because the fade is per-shape and not a layer.

Antialiasing is not a prop at all: smoothing every edge is one setting of the
**host** (`--msaa 4`, or `msaa = 4` under `[gui]`), because it is the render
pass that is multisampled — one attachment per window, nothing per widget.

Panning, sweeping a selection and locating the transport are the
**container's** gestures, so any container may carry a `gestures` table keyed
by modifier chord, each value a plan of steps in order:

```python
waveform(data=take, gestures={"drag": "pan", "shift": "select"})
```

The steps are `element` (hand the press to whatever is under the cursor, which
may decline), `pan`, `select`, `select_box`, `locate` and `none`. The order is
the point: `"element locate"` is a lane — grab the clip under the cursor, and
if there is none, locate. A plan that consumes nothing falls outward to the
container around it.

`select` sweeps the **time span**; `select_box` sweeps the same span
**restricted to the band of values** it covered, which is a rectangle rather
than a stripe. The second one declines where the picture has only one measured
axis, so `"select_box select"` is the plan for a stack of heavy views: a
rectangle on a waveform, whose y is amplitude, and the plain span on a
spectrogram, whose y is frequency — a range of *bins*, which is a different
field of a selection and a gesture that does not exist yet. A plain drag stays
a time span everywhere on purpose: that is what a drag over a waveform means in
every editor, and what a band of values is *for* is your business, so you name
the step.

### The clipboard: copy is the host's, cut and paste are yours

Ctrl+C over a selection is a **read**, so the host makes it alone: the span
leaves the samples it has mapped and lands on its own clipboard, typed, with
the rate it was taken at — nothing reaches your script. Where it cannot read the
source (a peak overview has no samples behind it; a live view has no addressable
past) it says so with a `"refused"` event rather than copying silence.

Ctrl+X and Ctrl+V **change data**, which the host does not own, so they arrive
as requests. A paste brings the clipboard with it — the kind, the whole typed
document, and one blob per bulk payload — because the clipboard is the host's:
a block copied in one window is pasted against an owner that never saw the copy.

```python
def on_clip(tag, *vals):
    if tag == "paste" and vals[1] == "samples":
        doc, blob = json.loads(vals[2]), vals[3]
        block = doc["content"]          # channels, frames, sample_rate
        values = array.array("f"); values.frombytes(bytes(blob))
```

The rate travels with the block and nothing converts it: resampling is an edit,
and an edit is something you perform and log, never a side effect of a paste.

`Editor` answers both verbs for the arrangement it holds: a cut whose selection
covers a clip removes that placement (undoably, through the document), and a cut
across a clip — or a paste of samples — is refused with its reason, because
that is a new length for the samples under it and samples belongs to whoever
owns it.

## The instrument without the script: a bundle

Everything so far kept Python in the loop. A **bundle** is the other posture:
the instrument written to a directory, and the script gone. The GUI is what
drives it, because its widgets are bound.

The pieces are the ones you have already seen, plus two kinds of hole. A
**symbol** is something the instrument allocates — a bus, a node, a buffer —
and a **parameter** is a value the mount supplies:

```python
from clausters.bundle import Bundle
from clausters.gui import knob, label, meter, panel, scope, toggle, window

b = Bundle("fm-voice")
freq = b.param("freq", float, default=220.0, min=60.0, max=700.0)
node = b.node("graph")        # -> "@graph"
lfo  = b.bus("lfo")           # -> "@lfo"

voice = b.synthdef(fm_voice())          # named "fm-voice.voice"
trem  = b.synthdef(tremolo())
graph = b.graphdef(rig(voice, trem))    # a GraphDef: members wired by buses

b.gui(window(
    panel(label("the GraphDef's surface", weight=3),
          toggle(label="play", value=True, bind=["/node_run", node], weight=1),
          layout="row", h=30),
    panel(knob(label="freq", min=60.0, max=700.0, value=freq,
               bind=["/node_set", node, "freq"]),
          knob(label="depth", min=0.0, max=1.0, value=0.5,
               bind=["/node_set", node, "depth"]),
          layout="row"),
    panel(meter(lfo, rate="control", min=0.0, max=1.0, label="lfo"),
          scope(lfo, rate="control", min=0.0, max=1.0),
          layout="row"),
    title="FM + tremolo", w=680, h=400, layout="col"))

b.boot(["/graph_new", graph, node, 0, 0, "lfo_bus", lfo, "freq", freq])
b.preset("bright", freq=110.0)
b.write("./fm-voice")
```

Each knob carries an inline `bind` — the same binding as `WidgetHandle.bind`,
declared in the tree so that a def which is only ever *loaded* still arrives
wired. `/graph_new` in the boot list brings the instance up: its own node id,
its own LFO bus, and its parameter as an initial port value.

Open it with no script at all:

```sh
# from clients/gui
cargo run --features standalone --bin clausters-gui -- \
    --standalone fm-voice --data-dir ./fm-voice
```

The window comes up, the graph starts, the knobs drive it and the meter reads
the bus — with nothing running but the host and the server embedded in it.

Two authoring rules follow from the holes living only in the GuiDef and the
boot list. **A bus, a node or a buffer reaches a def as a control**, never as a
baked constant, or two instances would write the same bus. And **widget ids are
local** to the bundle — the root is 1 and the rest are yours from 2 up, since
the mount offsets the whole block per instance. `write` checks both before it
emits anything: an unmountable bundle is unwritable.

The same directory is what a browser tab mounts as a custom element, one line
of markup and no client library at run time. That leg — the format, the mount's
two phases, several instruments in one document — is the
[bundles chapter](bundles.md) and the web client's
[components chapter](https://clausters-web.readthedocs.io/en/latest/components.html).

## Where to go next

- The GUI host wired to a running audio server is one call:
  `session.gui()` returns the same `GuiHost`, its client leg pointed at the
  session's server and mapping the same shared segment — which is what lets a
  meter, a scope and a playhead read the engine with no per-frame messages.
- Every builder's own props, defaults and event payloads are in the
  [API reference](api.md).
- The wire underneath — the `/gui_*` commands, the event payloads, the axis
  properties — is the server guide's
  [GUI protocol chapter](https://clausters.readthedocs.io/en/latest/gui-protocol.html).
- The arrangement model draws itself through these same `field` containers:
  the [composition chapter](composition.md) is the layer above.
- The runnable demos live in `clients/python/examples/`, one capability each;
  `gui_window.py` is the "first pixels" one to start from.
- Everything above used the shortcuts, which is what a script reaches for.
  [Building from the model alone](gui/model.md) writes the same kind of window
  without any of them — the four containers, the elements and `node` itself —
  which is both how to say what no shortcut names and what the wire actually
  carries.
