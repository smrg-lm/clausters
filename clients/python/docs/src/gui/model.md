# Building from the model alone

The previous chapter used the shortcuts — `panel`, `waveform`, `track`,
`scope` — because they are what a script reaches for. This one builds the same
kind of thing **without any of them**, from the four containers, the elements
and `node` itself.

Two reasons to spend an afternoon here. The shortcuts each carry **one common
case**: `waveform` is a navigating trace over addressable samples, `plot` the
same trace standing still, `track` a field with lane chrome. When what you want
is a point they do not name, the model is where you say it. And the model *is*
the wire — a tree built this way is, key for key, the JSON that goes out — so
this is also the chapter to read before binding a new language to the host.

Everything below runs against a host you booted the ordinary way:

```python
from clausters.gui import GuiHost
gui = GuiHost().boot()
```

## `node` is the whole protocol

Every builder in the package is a function that returns a `dict`, and every one
of them ends in the same call:

```python
from clausters.gui import knob, node

knob(label="cutoff", min=20.0, max=20000.0, value=800.0)
node("knob", label="cutoff", min=20.0, max=20000.0, value=800.0)   # identical
```

`node(type, *, children=None, id=None, **props)` is the generic node —
`{id, type, ...props, children}` — and it is the escape hatch for anything this
client does not name. A type the *host* does not know is laid out and not
painted, which is exactly what an older host does with a newer script's tree,
so an unknown node is a blank space and never an error.

The typed builders exist for the props: they document what each node takes,
convert what has to be converted (a list of note tuples into the flat quintuple
list the wire carries, an `Env` into break-points, a `bool` into the `1`/`0`
OSC has no type for) and refuse what is wrong before it leaves the process.
Reach for `node` when there is nothing to convert.

## The four containers

```python
from clausters.gui import field, layout, plane, window
```

| Builder | Axes | What it gives its children |
|---|---|---|
| `window` | 0 | a root: a top-level window (or, in a page, a canvas) |
| `layout` | 0 | an arrangement — `flow` is `row`, `col`, `grid`, `free` or `stack` |
| `plane` | 2, locked to one scale | a pannable, zoomable plane in content units |
| `field` | 2, independent | time against whatever the elements on it measure |

A `field` is one container in three uses, and **what is on it** decides which:

```python
field(a, b, label="drums")                 # a lane: it holds other fields
field(offset=0.0, dur=48000.0, data=take)  # a clip: it is placed on an x axis
field(h=22.0, axes={"x": {"unit": "beats"}})   # a bare ruler: nothing on it
```

An empty `field` with lane chrome is still a lane — a multitrack opens those
all the time — so the ruler is the case that has to say what it is: a strip of
a given `h`, nothing placed on it, no lane chrome.

A clip's **bodies are still props** (`data`, `notes`, `points`), not children.
That is the one place the model has not finished moving: the host builds the
bodies from the clip's own props and layers them, and a later milestone turns
them into children a script writes.

## The axes carry the chrome

Anything that describes an axis — its unit, the window you see through it, the
selection, the playhead, the value range — belongs to the **container**, under
one `axes` key:

```python
field(
    axes={"x": {"unit": "beats", "tempo": 2.0, "quant": 4.0, "link": 1,
                "start": 0.0, "len": 96000.0},
          "y": {"unit": "db", "min": -1.0, "max": 1.0}})
```

| Axis | Properties |
|---|---|
| `x` | `unit` (`"time"` / `"samples"` / `"beats"` / `"off"`), `start`, `len`, `tempo`, `beat_at`, `quant`, `sample_rate`, `link`, `sel_start`, `sel_len`, `playhead`, `playhead_at`, `playhead_loop_start`, `playhead_loop_len` |
| `y` | `unit` (`"norm"` / `"db"` / `"bits"` / `"percent"` / `"hz"` / `"off"`), `start`, `len`, `min`, `max`, `bit_depth`, `sel_min`, `sel_max` |

The selection sits on both axes, and each one holds the half it can mean. `sel_start`/`sel_len` are the span, in whole samples; `sel_min`/`sel_max` restrict it to a band of the y axis' own values, which a marquee swept with height sets and reports as two further arguments of the `"selection"` event. An empty or inverted pair — the default — is no restriction, and a view whose vertical measures frequency rather than a value reports none at all.

Inside an axis a property drops the axis marker: `x.start` is the navigation
window's start, `y.unit` is the vertical ruler. It is nested rather than two
bare `x`/`y` objects because those names are already the free-placement props,
and a container that is *placed* and *owns axes* would have no way to say which
it meant.

The pair works on a live update too, which is how you move an axis after the
fact:

```python
win["lane"].set(axes={"x": {"start": 24000.0, "len": 48000.0}})
```

## The elements

```python
from clausters.gui import curve, keys, label, meter, nodes, notes, score, signal
```

| Builder | What it draws | Its own props |
|---|---|---|
| `signal` | every view of a signal | `view`, the source, the capabilities — below |
| `notes` | MIDI notes over a pitch axis, with velocity and OSC lanes | `notes`, `osc`, `snap`, `velocity`, `osc_lane`, `midi_in` |
| `curve` | break-points, played by the server's own shape math | `points`, `duration`, `exp` |
| `keys` | a playable keyboard | `min`/`max` (the visible compass), `active_min`/`active_max`, `voice` |
| `nodes` | the audio server's node graph, live | `group`, `controls` |
| `meter` | a bus level, read from the shared segment | `bus`, `rate`, `min`, `max` |
| `score` | an engraved notation page | `display_list`, `playhead`, `editable` |
| `canvas` | a WGSL shader over the widget area | `shader`, `params`, `buses` |
| `label` | static text | `text`, `text_size`, `wrap`, `align` |

The controls — `knob`, `slider`, `number`, `button`, `toggle`, `text`, `menu` —
are elements with a value and no axis, and they did not move: a knob names what
it is.

### `signal` is a product, not a name

```python
signal(view=…, <source>, navigable=…, selectable=…, editable=…)
```

- **`view`** — `"trace"` (the default), `"spectrum"`, `"spectrogram"`,
  `"phase"`.
- **the source** — `bus` (with `rate`) is forward-only; `data`, `blob`,
  `buffer`, `path`, `cache` are addressable, which is what lets a view
  navigate, slice and select.
- **the capabilities** — `navigable`, `selectable`, `editable`.

The six shortcut names are six points of that product:

```python
signal(view="trace", path="take.f32")                     # `waveform`
signal(view="trace", path="take.f32", navigable=False)    # `plot`
signal(view="trace", bus=0, rate="audio")                 # `scope`
signal(view="spectrum", bus=0)                            # `spectrum`
signal(view="spectrogram", path="take.f32")               # `spectrogram`
signal(view="phase", bus=0)                               # `phasescope`
```

`navigable` is the one that is more than a capability. A trace that does not
navigate also resolves its source as the **sequence itself** rather than as a
take — no peak pyramid — and auto-fits a value axis nobody named. That is the
whole difference between the first two lines, and it is why the wire says
`navigable` instead of having two names.

The combinations the shortcuts name are the ones the host draws today. The
model lets you *write* the others, which is the point of having it, but a live
view that navigates or a spectrogram that retains its history are named in the
GUI roadmap and not built — say them and you get the nearest thing the host has.

## A worked window

Here is a small editor built entirely from the model: a picker over two pages,
a two-lane arrangement under a shared ruler, and a patcher beside it. Nothing
below is a shortcut.

```python
from clausters.gui import (curve, field, layout, menu, node, notes, plane,
                           signal, slider, window)

SR, BEAT = 48_000.0, 24_000.0
take = [0.0] * 1024          # whatever you have; a path or a buffer is likelier

axis = {"unit": "beats", "tempo": 2.0, "quant": 4.0,
        "sample_rate": SR, "link": 1}

v = window(
    # -- the chrome: a picker bound to the page stack under it
    layout(menu(["arrangement", "graph"], name="picker"),
           slider(name="scroll", min=0.0, max=8 * BEAT),
           flow="row", h=32.0),

    # -- one page at a time
    layout(
        # page 0: two lanes and a ruler, all on one axis
        layout(
            field(field(offset=0.0, dur=4 * BEAT, data=take, label="take"),
                  field(offset=4 * BEAT, dur=4 * BEAT,
                        notes=[(0.0, BEAT, 60), (BEAT, BEAT, 67)]),
                  name="lane", label="drums", axes={"x": axis}),
            field(field(offset=0.0, dur=8 * BEAT,
                        points=[(0.0, 0.0), (4 * BEAT, 1.0, "exp"), (8 * BEAT, 0.0)]),
                  label="filter", axes={"x": axis}),
            field(h=22.0, axes={"x": axis}),
            flow="col"),

        # page 1: the same graph as a patcher, on a plane
        plane(boxes=[{"def": "source", "inlets": [], "outlets": ["out"]},
                     {"def": "sink", "inlets": ["in"], "outlets": []}],
              cords=[0, 0, 1, 0], axis="both"),

        name="pages", flow="stack", index=0, weight=1.0),

    # -- an element the shortcuts do not name: a still trace that is selectable
    signal(name="detail", view="trace", data=take, navigable=False,
           selectable=True, axes={"y": {"unit": "db"}}, h=120.0),

    title="built from the model", w=900, h=560, flow="col")

win = v.open()

win["picker"].bind_widget(win["pages"], "index")
win["scroll"].bind_widget(win["lane"], "view_start")
```

Two things in there are worth naming.

**One `axis` dict, three containers.** The two lanes and the ruler share the
same `link`, so they share one navigation group: zoom or pan any of them and
all three move, including the one on the page nobody is looking at. The axis is
the group's, not any container's — which is why the slider drives it through
whichever member it is bound to.

**The stack is a flow, not a type.** `layout(..., flow="stack", index=0)` shows
one child; the other is neither laid out nor drawn, and keeps its GPU slot for
when you come back. The `menu` bound to `index` is the tab bar, and the script
never hears the click.

## Seeing what goes out

The last step is the one that makes the chapter concrete. A tree is a `dict`,
and `to_json` is what the client sends:

```python
from clausters.gui.guidef import to_json

print(to_json(field(signal(view="trace", data=[0.0, 1.0], navigable=True),
                    label="drums", axes={"x": {"unit": "beats", "link": 1}})))
```

```json
{"type": "field", "label": "drums", "axes": {"x": {"unit": "beats", "link": 1}},
 "children": [{"type": "signal", "view": "trace", "data": [0.0, 1.0],
               "navigable": 1}]}
```

That is the whole protocol: one JSON document, carried in one OSC argument,
with the node shape it had before any of this and a vocabulary of about twenty
words. A `name` you passed is stripped here — it is the client's index into the
tree, and never travels.

Print the trees you build while you learn them. Everything a widget does is in
that document, and everything the host sends back is a `/gui_event` with a tag
and flat values.

## Where to go next

- The props of each builder, with defaults and event payloads: the
  [API reference](../api.md).
- The same vocabulary as a wire specification, with the edit-back payloads and
  the gesture table: the server guide's
  [GUI protocol chapter](https://clausters.readthedocs.io/en/latest/gui-protocol.html).
- The TypeScript builders emit this same document from a camelCase options
  object: the web client's
  [visual elements chapter](https://clausters-web.readthedocs.io/en/latest/gui.html).
