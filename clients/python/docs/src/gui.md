# The visual elements: a GUI the script builds

A GUI in Clausters is a **def**, like an instrument. You build a tree of nodes,
send it in one message, and from then on you mutate live widgets one at a time.
Nothing is compiled, nothing is subclassed, and the window is a separate
process — the **GUI host** — that owns the pixels and talks OSC.

This page builds one from nothing and then walks every element there is,
finishing with an instrument that keeps its GUI after the script is gone.

**The host is what draws, always.** A script names what to look at — `plot(...)`,
`scope(bus)`, a `waveform`/`scope`/`spectrum` widget in a tree like the ones
below — and the host reads the buffer or the bus itself and paints it. The
client computes no picture: not a pixel column, not a trigger, not a decibel
curve. That is what keeps the Python and TypeScript clients one client in two
languages, and what keeps a figure from being drawn two ways. If you would
rather do your own arithmetic over your own canvas, the data paths are open and
nothing stops you — but that is your program, and it is not a surface this
project provides, documents or keeps in step.

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
from clausters.gui import GuiHost, button, knob, label, menu, panel, slider, view

gui = GuiHost().boot()

v = view(
    label("a filter", h=24),
    panel(knob(name="cutoff", label="cutoff", min=20.0, max=20000.0, value=800.0),
          knob(name="res", label="res", min=0.0, max=1.0, value=0.3),
          layout="row"),
    panel(slider(name="mix", label="mix", min=0.0, max=1.0, value=0.5),
          button(name="reset", label="reset"),
          menu(name="wave", options=["sine", "saw", "square"], index=0),
          layout="row", h=40),
    title="filter", w=520, h=260, layout="col")

win = v.open()
```

A window appears. Four things about that tree are worth noticing before
anything else.

**A view with no parent is a window.** There is one container, `view`, not a
window type and a panel type: nested in another view it is a panel, and at the
root it is the window. Any node opens, so the frame is only there when you want
its properties:

```python
view(knob(name="freq"), title="voice", w=200, h=200).open()   # a titled window
layout(knob(name="freq"), knob(name="res")).open()            # a window of two knobs
knob(name="freq").open()                                      # a window that is a knob
```

The last two are framed for you, in a window that hugs what it holds — the wire
opens an OS window for a `window`-rooted document and nothing else. Reach for
`view()` when a title, a size or a theme matters, since those belong to a root
nobody frames. (`window` is the older spelling of `view` and still works.)

**The view opens itself.** A builder returns a `View` — the GUI's counterpart
of a `SynthDef`: a tree you compose and then send, not a live widget. So the
tree is the subject of the sentence (`view(...).open()`), the way a def is
(`synthdef.send(server)`), and `open()` finds the host the way `plot` and
`scope` do: the one `GuiHost().boot()`, `GuiHost().attach()` or
`Session.gui()` registered, else one it boots. Pass `host=` to say which, and
`gui.open(tree)` still works — it is the low-level door `open()` goes through.
(`attach()` is the host this handle did not start; see
[Sessions](sessions.md#several-servers-and-the-one-you-did-not-start).)

**No widget has an id.** You passed `name=` instead, and `open` hands back a
window handle you index by name — `win["cutoff"]`. The host does use integer
ids on the wire, and `open` allocated them for you — **in the document it
sends**, not in the tree you wrote, because an id names a *live* widget and the
view is a definition. That is what lets one view open twice:

```python
a = v.open()
b = v.open()              # a second window; its own ids, the same view
a["cutoff"].set(value=200.0)   # b is not touched
```

A name is client-side only and never travels. Unlike an id (which recycles when
a window is freed), a name is stable, which is what a live edit addresses
against.

**The children came first.** The positional slot of every builder belongs to
what the widget is made of — a container's children, a label's text, a menu's
options — so an ordinary tree mentions no ids at all.

### A control widget is built from the control it drives

A `knob`, `slider`, `number`, `toggle` or `button` takes a **def's control**
positionally, and reads its name and its default off it:

```python
freq = control("freq", 220.0)
sd = SynthDef("voice", out(0.0, sine(freq=freq) * 0.2))

knob(freq, min=110.0, max=880.0)             # the control object you held
slider(sd["amp"], min=0.0, max=1.0, label="level")   # or one off the def
view(*[knob(c, min=0.0, max=1.0) for c in sd.controls])
```

**The range is the widget's**: a control is a signal in the graph and says
nothing about how a knob is drawn, so `min`/`max` are spelled here. A control
with no range of its own says so rather than being drawn over a guess — which is
every control but a **Faust** parameter, whose `hslider` declares one inside the
DSP and reports it back (and a keyword still wins over it: the control says what
it is, the call says how to draw it).

All three def families answer the same way — `sd["freq"]`, `fd["cutoff"]`,
`gd["mix"]` — so the widget does not care which built it.

And the range is **linear** unless the widget says otherwise. Two keywords say
otherwise, and a control that wants both spells both:

```python
slider(sd["amp"], min=0.0, max=1.0, curve=4.0)        # fine at the bottom
number(label="note", min=0.0, max=127.0, step=1.0)   # whole note numbers
```

`curve` bends the range the handle travels: `0` (the default) is linear,
negative spends most of the range on the first half of the travel and positive
on the last half — the fine-at-the-bottom feel a frequency or an amplitude
control wants. It is the same bend `lincurve` runs and an envelope
segment runs on the audio thread, because the host reads it out of the shared
core rather than deriving one of its own: a control feels the way the value it
produces was computed.

`step` is the grid a **drag** lands on, in the value's own units. It is counted
from `min` and never leaves the range, so a grid that does not divide it
(`0..10` by `3`) stops at `9` rather than on an off-grid `10`. A Faust
parameter arrives with the step its `hslider` declared, like its range. A value
*you* send — `value=`, or `win.widget("note").set(value=...)` — is drawn as sent: the
step is a rule about the hand, not a constraint on what the script may say.

There is deliberately no *named* spec (`spec="freq"` for 20..20000
exponential). These two keywords are what one would be built out of, and a name
that silently drew the wrong curve would be worse than no name at all.

The widget's `name` becomes the control's name, which is what the handle
addresses it by, and what [binding](#values-that-never-come-back-to-the-script) uses to
reach the synth.

### The two switches, and what a press means

A `button` is momentary and a `toggle` latches, and both send a **pair of
values** rather than a boolean: `on` and `off`, `1`/`0` unless you name another
pair. A bypass lives at `0.0`/`0.7` and a mode at `1`/`2`, and neither is a span
a widget could be drawn over — which is why it is a pair and not a `min`/`max`:

```python
toggle(sd["bypass"], on=0.7, off=0.0, label="wet")
```

A button's `mode` says which of the two pointer primitives reaches the server:

```python
gate = control("gate", 0.0)
fire = control("fire", 0.0, rate="tr")

button(gate, label="hold")                  # `on` while held, `off` on release
button(fire, mode="press", label="fire")    # one message, the bang
```

- `"gate"` (the default) sends `on` at the press and `off` when the button is
  let go, so the value lasts exactly as long as the button is held — what an
  `env_gen` gate reads, and what a trigger control ignores the tail of by
  definition.
- `"press"` sends `on` at the press and **nothing** after it.

**A widget cannot make a value instantaneous.** What is sent is held by whoever
receives it, so `"press"` is a bang only against something that returns to zero
on its own: a trigger control (`rate="tr"`), which the server resets after one
block. Over any other control it would leave `on` standing forever, and the
client refuses to build that pair rather than letting you find it by ear. A
button driving no control has no such trouble — it emits a `/gui_event` and one
message *is* an event.

**Press and release are the primitives, and a click is not a mode.** Everything
else a pointer does to a button is composed from the two: a click is a press and
a release that landed inside, a double click is two of those inside a window.
Those are gestures, and what a `mode` says is only which primitive reaches the
server.

### What the hand did, as against what the widget is worth

A button says two things at once, to two audiences. Its **value** is a control
signal — `on`/`off`, which a binding forwards to the audio server without this
script ever seeing it. Its **interface events** are what the hand did, and they
have three verbs of their own:

```python
win["play"].on_click(start)      # the press was completed on the button
win["key"].on_press(sound)       # the pointer went down
win["key"].on_release(silence)   # ...and came up, wherever it came up
```

`on_click` is what a command button wants: press, slide off the button,
release, and **nothing happens** — the cancellation every desktop convention
gives an "Accept", and the one a piano key must not have. `on_press` is the
other side of that: the note is at the down-stroke, no second thoughts.
`on_release` fires for an abandoned press too, which is exactly what makes it
the release and not the click.

They reach the script **whether or not the widget is bound**. A binding forwards
a widget's *value*, and a command is not a value, so one button can drive a
synth's gate and run a script's action at once:

```python
win.bind(synth)                            # the gate goes straight to the server
win["hold"].on_click(lambda: print("!"))   # and the click still arrives here
```

`on_event` is the raw stream and sees everything, these three included (they
arrive as the one-string payload they are), so reading the value and the
command on one widget is two registrations and no filtering. All four survive a
redraw: a callback belongs to the widget the *name* points at, not to the id it
happened to have.

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
something you `open()` on the host you already booted, wrapped in `view(...)`
when the window's own properties matter.

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
view(
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
view(
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
scheduler. When the widgets were [built from the def's
controls](#a-control-widget-is-built-from-the-control-it-drives), each already
knows what it drives, so the whole surface is one verb:

```python
win = view(knob(freq), slider(amp)).open()
win.bind(synth)          # /node_set <synth> freq, /node_set <synth> amp
win.unbind()             # they emit /gui_event here again
```

`win.controls` says what is there (`{"freq": "freq", "amp": "amp"}` — the
widget's name on the left, the control it drives on the right; they need not be
the same string). A window where nothing was built from a control refuses to
bind, since that can only be a mistake.

One at a time is the same thing spelled out, and is what you reach for when the
target is *not* a def control:

```python
win["cutoff"].bind("/node_set", 1000, "freq")
```

Turning that knob sends `/node_set 1000 freq <value>` straight from the host.
To **another widget** — its value lands on that widget's prop:

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
  Nyquist. Raise `fft_size` to zoom further; that is the only thing that adds
  detail, here as anywhere.

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

Most of the time you should not have to choose. A **source** is the samples as
something you hold: hand it to the view instead of a carrier, and it picks one
by size — short stays inline, long spills to a file the host maps.

```python
sig = source(take)                          # or source(buffer=b), source(path=p)
v = view(waveform(name="wave", data=sig), title="a take")
win = v.open()

sig.set(other_samples)     # the view redraws; the window is not rebuilt
```

A source is also the *entry point*, which is what makes it worth holding: one
source in two views is one payload and two references, and `set` reaches the
definition and every window already drawing it. The carrier is fixed when the
source is made — a widget on screen was built around it — so `set` writes
through the same one: inline samples are replaced, and a spilled file is
rewritten where it is and re-read. For samples somebody else owns (a server
buffer, a cache) there is nothing to set: change them where they live and call
`sig.reload()`.

The carriers are still there to name by hand, and a view never carries a
megabyte over OSC if it does not have to. In the host's precedence order:

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

Those two are the primitives, and they are the right call when what you want is
the *file* a `cache=` prop maps. To follow a take without a picture in it —
a headless capture, a test, a read-out — `clausters.data.RecordingStream` owns
the whole conversation: it subscribes, keeps one cache per take, and says how
far each has been reported.

```python
from clausters.data import RecordingStream

stream = RecordingStream.open(server, [take])
stream.on_report(lambda bufnum, s: print(s.written(bufnum), "frames"))
Synth("record_something", {"buf": take.bufnum}, server=server)
```

Each take's cache is allocated at the buffer's **full length** and empty, so the
axis does not move while it fills; `written` is how far the reports have got, and
past it the cache is the silence the buffer is — read up to it and the two stay
apart, which is what a `waveform`'s `fills` does for the picture. Only the
overview arrives: inside one bucket there is one figure, so a script that needs
the detail reads the take back with `Buffer.get_samples` once it is finished.
`stop()` cancels the subscription and leaves the caches readable; `free()` drops
them.

### The structures are held the same way

Samples are one kind of heavy prop; the others are the **structures** — a
`bpf`'s `points`, a roll's `notes` and `osc`, a patcher's `boxes` and `cords`,
a `score`'s `display_list`. They are the same relation and they take the same
object: name the prop the source *is*, and hand it to the builder in place of
the value.

```python
env = source(points=[(0.0, 0.0), (1.0, 1.0, "exp")])
win = view(bpf(name="env", points=env, editable=True)).open()

env.set([(0.0, 1.0), (0.5, 0.3, "exp"), (1.0, 0.0)])   # every view follows
```

A structure has nothing to choose: it rides in its own prop, which is the
carrier. What the source adds is that the payload stays **addressable** after
the definition is written — one source in two views is one payload and two
references, `set` reaches the definition and every window already drawing it,
and the value is normalized on the way out exactly as the builder's own keyword
normalizes it, so both spellings put the same flat list on the wire.

An engraved page is the one that travels two ways, and the source hides it: a
definition carries the display list as its parts (`vb`, `glyphs`, `prims`,
`cursors`, `step`) and a live update carries the whole `display_list`, which is
the host's door for replacing a drawing in place. `page.set(...)` picks the
right one.

```python
engraved = source(display_list=score.display_list())
win = view(notation.score_view(engraved, name="page", editable=True)).open()

# a "transpose" event came back: apply it and hand the page back
score.transpose_to(xml_id, position)
engraved.set(score.display_list())
```

The source takes the display list whole and sends only its drawing layers, so
`page_json` — the same layers as a JSON string — is what a **hand-driven**
`win["page"].set(display_list=…)` still needs, and not something a source asks
you for.

There is no `reload()` for a structure — it holds its own payload, so there is
nowhere for it to have moved.

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
view(
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
from clausters.gui import clip, timeruler, track, view

BEAT = 24_000.0          # samples per beat at 48 kHz, two beats a second

v = view(
    track(clip(name="a", offset=0.0, dur=4 * BEAT, data=take, label="take"),
          clip(name="b", offset=4 * BEAT, dur=2 * BEAT,
               notes=[(0.0, BEAT, 60), (BEAT, BEAT, 67)]),
          label="drums", link=1),
    track(clip(offset=0.0, dur=6 * BEAT,
               points=[(0.0, 0.0), (3 * BEAT, 1.0, "exp"), (6 * BEAT, 0.0)]),
          label="filter", link=1),
    timeruler(link=1, ruler="beats", tempo=2.0, h=22.0),
    title="a multitrack", w=900, h=420, layout="col")

win = v.open()
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
win["a"].on_event(lambda tag, *payload: print(tag, payload))
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

The **typeface** is the host's too, for the same reason, and it may be handed
over at any point:

```python
gui.font(Path("/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf").read_bytes())
```

A raw TrueType/OpenType file, drawn by every window the host has open and every
one it opens later — a face is a property of the host, not of a window, so the
call carries no id. Loading one **relayouts nothing**: the sizing table never
followed the typeface, so the same tree comes up the same size before and after.
What changes is that `text_size` becomes continuous rather than quantized to
half-steps of the cell, which a bitmap glyph's own pixels require. The
launch-time spelling is `GuiProcess(font=...)` (the host's `--font`), for a face
that should be in place before the first window opens, and a host built without
a rasterizer keeps drawing with its embedded bitmap face — the floor every build
draws on, and what a face it cannot read leaves it on.

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
that is a new length for the samples under it and samples belong to whoever
owns them.

## The instrument without the script: a bundle

Everything so far kept Python in the loop. A **bundle** is the other posture:
the instrument written to a directory, and the script gone. The GUI is what
drives it, because its widgets are bound.

The pieces are the ones you have already seen, plus two kinds of hole. A
**symbol** is something the instrument allocates — a bus, a node, a buffer —
and a **parameter** is a value the mount supplies:

```python
from clausters.bundle import Bundle
from clausters.gui import knob, label, meter, panel, scope, toggle, view

b = Bundle("fm-voice")
freq = b.param("freq", float, default=220.0, min=60.0, max=700.0)
node = b.node("graph")        # -> "@graph"
lfo  = b.bus("lfo")           # -> "@lfo"

voice = b.synthdef(fm_voice())          # named "fm-voice.voice"
trem  = b.synthdef(tremolo())
graph = b.graphdef(rig(voice, trem))    # a GraphDef: members wired by buses

b.gui(view(
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
  `window.py` is the "first pixels" one to start from.
- Everything above used the shortcuts, which is what a script reaches for.
  [Building from the model alone](gui/model.md) writes the same kind of window
  without any of them — the four containers, the elements and `node` itself —
  which is both how to say what no shortcut names and what the wire actually
  carries.
