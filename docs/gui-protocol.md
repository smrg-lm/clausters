# The GUI protocol (`/gui_*`)

The GUI host is a **separate peer**, not part of the audio server: it owns the
windows, the widgets and the GPU, and a script drives it over OSC — the same
encoding the audio server speaks, only the vocabulary differs. Its default UDP
port is **57210** (clear of the audio server's 57110/57120).

This page is the wire reference. The *why* behind it — the host's two roles, the
declarative protocol, the GPU substrate, the composition views — is in
[Clients and language bindings](clients.md); its internals and the recipe for
adding a widget are in [Architecture](architecture.md).

## Commands

| Message | Meaning |
|---|---|
| `/gui_def id json [blob…]` | Build a whole widget tree in one message. `json` is the GuiDef document (below); trailing blobs carry bulk data a widget references by index. Re-sending an existing id **redefines** it (the old subtree is freed first), exactly as re-sending a `SynthDef` replaces it. |
| `/gui_set id key value …` | Update one live widget's properties. Types are preserved (an OSC int stays an int). A value that is logically an array (a curve's break-points, a patch's wires) rides as its **JSON string**, since an OSC key/value is a scalar. |
| `/gui_free id` | Destroy a widget and its subtree. Freeing a `window`-rooted def closes its window. |
| `/gui_query id` | Ask for a widget's state. Replies `/gui_info id type key value …`; an **empty type** (`""`) means no such widget — the host answers either way, as the audio server replies even on a miss. |
| `/gui_bind id "server" address prefix…` | Forward this widget's value **straight to the audio server**, bypassing the script: on every change the host sends `address` with the fixed `prefix` arguments followed by the value (e.g. `"/n_set" 1001 "freq"` makes the widget send `/n_set 1001 freq <value>`). A bound widget stops emitting `/gui_event`. |
| `/gui_bind id` | (no target) Remove the binding; the widget emits events again. |
| `/gui_load name` | Instantiate a **persisted** GuiDef by name (the host replays it as its saved `/gui_def`). Needs a data directory. |

There is no save command: a GuiDef whose root carries a `name` prop is
**persisted on `/gui_def`**, the way a named def is persisted on `/d_recv`. That
is what lets a host boot a whole interface with no script attached (the
standalone path).

## The GuiDef document

One tree, one document — mirroring `SynthDef`/`GraphDef`. Every node is:

```json
{"id": 10, "type": "slider", "min": 0.0, "max": 1.0, "label": "gain",
 "children": []}
```

- **`type`** names the widget; every other key is a **property** of it.
- **`id`** addresses the node for `/gui_set`, `/gui_free`, `/gui_query`, bindings
  and events. The root's id is the one given to `/gui_def`.
- **`children`** nests (containers only: `window`, `panel`, `track`).
- **`bind`** as an inline prop registers a binding declaratively, so a saved
  GuiDef carries its own (no separate `/gui_bind` at boot).

The wire form is deliberately generic (`{id, type, props, children}`): a new
widget kind never changes the protocol, and a host that does not know a type lays
it out but does not paint it — old hosts and new scripts still interoperate.

Bulk data (waveform samples, a peak cache) never rides the JSON: a widget names a
local `path`/`cache` the host maps (or fetches, in a browser), a server `buffer`
it pulls over its client leg, or — for small bodies only — a trailing `blob`.

## Events

The host pushes back to the script that built the window:

| Message | Meaning |
|---|---|
| `/gui_event id <value>` | A control changed: a float (`slider`/`knob`/`number`), an int (`toggle` 0/1, `menu` index, `button` press), or a string (`text`). |
| `/gui_event id <tag> <flat values…>` | A view wrote data back. The tag names *what* was edited; the values are flat OSC primitives (never a new address — see below). |
| `/gui_closed id` | The window was closed by the user. |

The **edit-back payloads**:

| Tag | Arguments | Sent by |
|---|---|---|
| `"points"` | `t v shape curve` per break-point | the `bpf` editor, and an automation `clip` — one payload, whichever view drew the curve |
| `"clip"` | `offset dur` (timeline units) | a `clip` moved or resized |
| `"wire"` | `member control bus` (an empty bus = unwired) | a `graph` patch rewired |
| `"locate"` | `position` (timeline units) | a lane's time ruler (or its empty space) clicked — the transport is being seeked there |
| `"selection"` | `start len` (samples) | a selection dragged on a timeline view |
| `"view"` | `start len` (samples) | the shared navigation window zoomed or panned |
| `"view_y"` | `start len` (0..1) | the vertical display window zoomed or panned |

Edited data flows as a **payload, never a new address**: the `/gui_*` family does
not grow per widget.

## The widget catalog

The authoritative per-widget reference — every property, its default and its
meaning — is the [Python client's builder
documentation](https://clausters-python.readthedocs.io/), since that is how a
script actually names these. The catalog itself:

| Type | What it is | Notable properties |
|---|---|---|
| `window` | A top-level window (a GuiDef root) | `title`, `w`, `h`, `layout` |
| `panel` | A nestable container | `layout` |
| `label` | Static text | `text` |
| `knob`, `slider`, `number` | Continuous controls | `min`, `max`, `value`, `label` (`vertical` on a slider) |
| `button`, `toggle` | Momentary / latching | `label`, `value` |
| `text`, `menu` | A string field, a choice | `value` / `options`, `index` |
| `meter` | A control-bus level, read from the server's shared segment | `bus`, `min`, `max` |
| `scope` | An oscilloscope: a control bus, or an audio **tap** | `bus` / `tap`, `trigger`, `hold` |
| `phasescope` | A goniometer (stereo field) over two taps | `tap`, `tap2`, `hold` |
| `spectrum` | A live spectroscope over a tap | `tap`, `fft_size`, `averaging` |
| `nodetree` | The server's node graph, live | `group`, `controls` |
| `waveform` | The editor-grade waveform: multichannel lanes, rulers, selection, playhead, linked navigation | the data (`data`/`blob`/`buffer`/`path`/`cache`), `channels`, `ruler`, `ruler_y`, `sel_*`, `playhead_at`, `link`, `offset` |
| `spectrogram` | The editor-grade spectrogram, the same chrome | the data, `window_size`, `hop`, `freq_scale`, `db_floor`/`db_ceil`, `colormap` |
| `bpf` | A drawable break-point envelope, played by the server's own shape math | `points`, `min`, `max`, `duration`, `exp` |
| `plot` | A static plot of a signal | `data`/`blob`/`path`, `min`, `max` |
| `track` | A multitrack **lane**, holding `clip` children on the window's shared time axis | `label`, `height`, `snap`, `ruler`, `tempo`, `sample_rate`, `playhead_at`, `playhead`, `link` |
| `clip` | A placed rectangle spanning `[offset, offset + dur]` — the graphic unit. Its body is a take, a piano-roll or an automation curve | `offset`, `dur`, the take (`buffer`/`path`/`cache`/`data`/`blob`), `notes`, `points`, `min`, `max`, `label` |
| `graph` | A **patcher** of a bus-wired node graph: member boxes, bus nodes, a wire per connection | `members`, `buses`, `wires`, `label` |
| `canvas` | A script-supplied WGSL shader over the widget area | `shader`, `params`, `buses` |

A timeline view (and a lane) shows the transport two ways, and they are different
things: `playhead_at` **anchors the line to the engine clock** (it is the clock
value at timeline position 0, so the line *sweeps* as the audio runs), while
`playhead` is a **static cursor** — where a located, stopped transport sits. Both
are group-wide, so every lane shows the one cursor; a negative value is none.
Clicking a lane's ruler moves the cursor and sends `"locate"`.

The lanes of a window share **one navigation group**: they zoom, pan and carry a
playhead as one, and the axis spans the composition (the longest clip end). The
same group model links the heavy views — an explicit `link` id joins or splits
it, and a `/gui_set` of `view_*`/`sel_*`/`playhead_at` on any member applies
group-wide.
