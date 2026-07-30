# The GUI protocol (`/gui_*`)

The GUI host is a **separate peer**, not part of the audio server: it owns the
windows, the widgets and the GPU, and a script drives it over OSC — the same
encoding the audio server speaks, only the vocabulary differs. Its default port
is **57210** (clear of the audio server's 57110/57120), on **UDP and TCP
alike**: like the audio server, the host accepts length-prefixed OSC over TCP
by default (`--no-tcp` disables it, `--max-frame` sets the frame ceiling,
default 16 MiB), and the Python `GuiHost` connects over TCP by default — so a
`/gui_def` tree with its blobs, the largest payload in the system, is not
bounded by a UDP datagram. A third, opt-in carrier is **WebSocket** (`--ws
[port]`, default 57220, the same flag the audio server takes): one OSC packet
per binary message, browser-reachable — the carrier the TypeScript client uses
to drive a *native* host from a page, exactly as it drives a `clausters --ws`
audio server.

This page is the wire reference. The *why* behind it — the host's two roles, the
declarative protocol, the GPU substrate, the composition views — is in
[Clients and language bindings](clients.md); its internals and the recipe for
adding a widget are in [Architecture](architecture.md).

## Commands

| Message | Meaning |
|---|---|
| `/gui_def id json [blob…]` | Build a whole widget tree in one message. `json` is the GuiDef document (below); trailing blobs carry bulk data a widget references by index. Re-sending an existing id **redefines** it (the old subtree is freed first), exactly as re-sending a `SynthDef` replaces it. |
| `/gui_set id key value …` | Update one live widget's properties. Types are preserved (an OSC int stays an int). A value that is logically an array (a curve's break-points, a patch's wires) rides as its **JSON string**, since an OSC key/value is a scalar. |
| `/gui_free id` | Free a widget and its subtree. Freeing a `window`-rooted def closes its window. |
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
  and events. The root's id is the one given to `/gui_def`. Ids live in **one
  namespace per host**, across all windows (exactly like the audio server's node
  ids): a duplicate id is skipped at define time with a warning, so a client must
  keep ids unique host-wide. It is the **client's** job to allocate them — and,
  like node ids, from a **recycling** pool: the Python client assigns a fresh id
  to any widget built without one (and to every window) from a bounded window
  starting at 1000, and a freed subtree returns its ids to the pool (a redraw
  re-defining a window frees the old subtree first), so a long live session
  reuses ids instead of climbing. Hand-picked ids below 1000 never collide with
  assigned ones.
- **`name`** (Python client only) is a **client-side** convenience, never on the
  wire: a widget built with a `name` is bound in the window handle `open` returns,
  so a script addresses it by name (`win["cutoff"].set(…)`) and never writes or
  matches an integer. The name is stripped from the JSON — the host only ever
  sees ids.
- **`children`** nests (containers only: `window`, `panel`, `scroll`, `track`).
- **The place props** — every widget, whatever its type, may carry `w`, `h`,
  `weight`, `x`, `y` (all numbers, **logical pixels**, all live via `/gui_set`).
  In a `row`/`col` the main axis resolves in **one order**: a fixed main-axis
  size (`w` in a row, `h` in a col) is taken as given; else an explicit
  `weight` takes that share of the leftover; else the widget's **natural
  size** — how big that kind of widget wants to be, the host's own number —
  is taken as wanted; else the child shares the leftover at weight 1. The
  cross axis always fills. So a `col` of controls is a stack of control-high
  rows (with the leftover empty under them, if nothing elastic is there to
  take it), a `col` of views splits evenly as it always did, and `weight` is
  what stretches a control past its natural size. Which widgets have one: the
  **content** kinds do (`label` unless it wraps, `button`, `toggle`, `number`,
  `menu`, a single-line `text`, a `slider`'s thickness across its track — its
  groove plus the row its value reads out in, which is why the number never
  sits on the handle — a `knob`'s height, a `timeruler`'s thickness), the **surface** kinds do not
  (`panel`, `scroll`, `patch`, `track`, `plot`, `nodetree`, `canvas`, the
  heavy views, a wrapped `label`, a multiline `text`). A natural size follows
  the host's sizing table and the widget's own `text_size`/`label`, **never
  its data** — a longer string or another thousand samples never move it, so
  a `/gui_set` never relayouts the window. In a `free` container `x`/`y`
  (+ `w`/`h`) position the child absolutely, and a child with none of the four
  overlays the whole area. A container additionally takes `margin` (inset
  before its children, default 6), `gap` (between children, default 6) and
  `cols` (a fixed `grid` column count; default near-square). One pass, no
  measurement and no constraint solver — a container never measures its
  children, so chrome that must hug its content says so with `h`: when a
  layout needs negotiation, the answer is explicit sizes.
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
| `"notes"` | `start dur pitch velocity channel` per note | the `pianoroll` view (and a `clip`'s roll) — MIDI notes edited |
| `"osc"` | `time label` per event | the `pianoroll` view — OSC event markers edited |
| `"note"` | `pitch velocity state channel` (ints; state 1 = press, 0 = release) | the `piano` keyboard played — MIDI-shaped, translatable 1:1 to note-on/note-off |
| `"range"` | `min max` (MIDI notes) | the `piano`'s visible range panned or zoomed |
| `"clip"` | `offset dur` (timeline units) | a `clip` moved or resized |
| `"element"` | `id` (a string: the MEI `xml:id`; empty = the selection was cleared) | a `score` page clicked — the engraved element under the cursor |
| `"transpose"` | `id steps` (the MEI `xml:id`; whole diatonic steps, positive = up the staff) | a `score` element dragged up or down — a pitch edit *requested*, since the host holds no score |
| `"wire"` | `src_box outlet dst_box inlet` (ports by name; a rate mismatch is refused at the gesture) | a `patch` cord drawn `outlet -> inlet` |
| `"move"` | `index x y` (box index; canvas units) | a `patch` box dragged — one payload per moved box, so the driver owns the geometry |
| `"locate"` | `position` (timeline units) | a lane's time ruler (or its empty space) clicked — the transport is being seeked there |
| `"selection"` | `start len` (samples) | a selection dragged on a timeline view |
| `"view"` | `start len` (samples), or `x y zoom` on a `scroll` | the navigation window zoomed or panned — the timeline group's shared window, or a 2D workspace's plane |
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
| `window` | A top-level window (a GuiDef root) | `title`, `w`, `h`, `layout`, `margin`, `gap`, `cols`, `theme` |
| `panel` | A nestable container | `layout`, `margin`, `gap`, `cols`, `theme` |
| `scroll` | The **2D workspace**: a container whose children live in a virtual content area seen through a panning, zooming window. General first — the default is the free plane; the constrained scroll views degenerate from it by configuration | `axis` (`both`/`x`/`y`), `zoom` (0 disables the wheel zoom), `content_w`/`content_h`, `view_x`/`view_y`/`view_zoom`, plus `layout` (default `free` here), `margin`, `gap`, `cols`, `theme` |
| `label` | Static text | `text`, `text_size`, `wrap`, `align` (`start`/`center`/`end`) |
| `knob`, `slider`, `number` | Continuous controls | `min`, `max`, `value`, `label`, `text_size` (`vertical` on a slider) |
| `button`, `toggle` | Momentary / latching | `label`, `value`, `text_size` |
| `text`, `menu` | An editable string field, a choice | `value`, `multiline` / `options`, `index`, `text_size` |
| `meter` | A bus level, read from the server's shared segment | `bus`, `rate` (`audio` default / `control`), `min`, `max` |
| `scope` | An oscilloscope over `channels` adjacent buses from `bus` (trigger searched in the first channel; a lock/free read-out) | `bus`, `rate` (`audio` default / `control`), `channels`, `overlay`, `window_ms`, `trigger`, `hold`, `min`/`max`, `ruler` (ms) / `ruler_y` (value; `"off"` hides) |
| `phasescope` | A goniometer (stereo field) over the audio bus pair `bus` / `bus + 1` | `bus`, `window_ms`, `hold` |
| `spectrum` | A live spectroscope: one color-coded curve per channel over `channels` adjacent audio buses | `bus`, `channels`, `fft_size`, `db_floor`/`db_ceil`, `freq_scale` (`log`/`linear`/`mel`/`bark`; `log_freq` is the legacy boolean alias), `averaging`, `peak_hold`, `ruler` (Hz) / `ruler_y` (dB; `"off"` hides) |
| `nodetree` | The server's node graph, live | `group`, `controls` |
| `waveform` | The editor-grade waveform: multichannel lanes, rulers, selection, playhead, linked navigation | the data (`data`/`blob`/`buffer`/`path`/`cache`), `channels`, `ruler`, `ruler_y`, `sel_*`, `playhead_at`, `playhead_loop_*`, `link`, `offset` |
| `spectrogram` | The editor-grade spectrogram, the same chrome | the data, `window_size`, `hop`, `freq_scale`, `db_floor`/`db_ceil`, `colormap` |
| `bpf` | A drawable break-point envelope, played by the server's own shape math | `points`, `min`, `max`, `duration`, `exp` |
| `pianoroll` | The editor-grade piano-roll: a keyboard, a MIDI-note grid, a velocity lane and an OSC-event lane; the same chrome and navigation as the heavy views | `notes` (`start dur pitch velocity channel` quintuples), `osc` (`time label` pairs), `min`/`max` (pitch window), `snap`, `velocity`, `osc_lane`, `midi_in` (live MIDI painting: the native host opens its virtual input port and paints incoming notes — at the running playhead, or step-entry on the `snap` grid), `ruler`, `sel_*`, `playhead_at`, `playhead_loop_*`, `y_start`/`y_len`, `link` |
| `piano` | The playable virtual keyboard, laid out with real piano proportions; its overview strip pans/zooms the visible MIDI range, and it plays server voices itself when `voice` is set (an `/s_new` per key press, a `gate 0` per release) | `min`/`max` (visible range; min snaps to a white key), `active_min`/`active_max` (keys outside draw grayed and are inert), `pan` (0 freezes all range navigation), `overview`, `velocity` (fixed; unset = from the press height), `channel`, `voice`/`voice_args` (host-managed voices), `label` |
| `plot` | A static plot of a signal: multichannel lanes, x/y rulers, a hover readout, and **views** (`signal`, `spectrum`; the set is extensible) — measurement without navigation | `data`/`blob`/`path`, `channels`, `view`, `overlay`, `sample_rate`, `min`/`max` (omit a side to auto-fit it; the string `"auto"` releases it live), `ruler` (`samples`/`time`/`off`), `ruler_y` (`off` to hide), and for `view: "spectrum"`: `fft_size`, `db_floor`/`db_ceil`, `freq_scale` (`log`/`linear`/`mel`/`bark`) |
| `timeruler` | A **free-standing time ruler**: the shared axis as a strip the *document* places — a DAW's ruler above its tracks. A lane's own `ruler` is reserved out of that lane's height, so ruling a stack meant picking one lane to carry it and to pay for it; this owns its box instead. Joins the group named by `link` and labels its window; its ticks are indented by a lane's header width so they stand over the samples they name. A press **locates**, Shift+drag pans, the wheel zooms | `ruler` (the unit), `sample_rate`, `tempo`/`beat_at`/`quant`, `link`, `h` (its thickness), `theme` |
| `track` | A multitrack **lane**, holding `clip` children on the window's shared time axis | `label`, `height`, `snap`, `ruler`, `tempo`, `sample_rate`, `playhead_at`, `playhead`, `playhead_loop_*`, `link`, `theme` |
| `clip` | A placed rectangle spanning `[offset, offset + dur]` — the graphic unit. Its bodies **layer**: a take, a piano-roll of events, and an automation curve over them | `offset`, `dur`, the take (`buffer`/`path`/`cache`/`data`/`blob`), `notes`, `points` (+ `points_min`/`points_max`, the curve's own value axis), `min`, `max`, `label` |
| `patch` | A **directed, typed patcher**, drawing both levels: boxes with **inlets on top, outlets on the bottom**, a **cord** per `outlet -> inlet` connection, **coloured by rate** — contrasting primaries at one width — audio (`ar`) red, control (`kr`) blue, init (`ir`) yellow and dashed — colour carries the rate. At **level 1** (a `GraphDef`) a cord *is* a server bus (not drawn — the client names it); at **level 2** (a `SynthDef`/`FaustDef`) a cord is an internal UGen wire. A **canvas**: a box with `x`/`y` places freely; a box **without** `x`/`y` takes its slot in the host's **layered (Sugiyama-style) auto-layout** (ranked by longest path to a sink, so inputs sit above their use and sinks at the bottom). Boxes drag (`"move"` flows back), a click or a marquee on empty canvas selects, and inside a `scroll` workspace the whole patch pans and zooms; the labelled panel frames whatever boxes it holds | `boxes` (each `{def, inlets, outlets[, x, y, role]}`; a port is a bare name (audio) or `{name, rate}` with `rate` `"control"`/`"init"`; `role` `"source"`/`"const"` only tags a box for drawing — a `const` value box gets a distinct fill — the layout ranks every box by its cords), `cords` (a flat `[from_box, outlet, to_box, inlet, ...]` list, indices within each box's inlet/outlet lists), `label` |
| `score` | An **engraved music-notation page**. The client engraves a score and sends a *display list* — a glyph-outline table keyed by SMuFL codepoint plus placed glyphs, staff lines, stems, beams, slurs and text in page units — which the host fits into the widget and tessellates into the same triangle mesh as the rest of the chrome. Every primitive carries the MEI `xml:id` it was engraved from, so a click names an element (`"element"`), a drag transposes it (`"transpose"`) and the page shows a playback cursor over its own timemap | `vb` (the `[width, height]` page-unit viewBox), `glyphs` (hex SMuFL codepoint → outline path `d`), `prims` (the placed primitives, each with its `id`), `cursors` (the cursor track: `t` in ms → `x`, `y0`, `y1`), `step` (page units per diatonic step), `display_list` (the whole drawing replaced live, as a JSON string), `playhead`, `playhead_at`, `playhead_loop_*` (ms), `sample_rate`, `selected`, `editable` (opt into pitch editing; off = a read-only view) |
| `canvas` | A script-supplied WGSL shader over the widget area | `shader`, `params`, `buses` |

**A data view names a bus and a rate.** Every live view — `meter`, `scope`,
`phasescope`, `spectrum` — reads from **`bus`** (default `0`, the first
hardware output) at **`rate`** (`"audio"`, the default, or `"control"`), over
`channels` **adjacent** buses where it takes several. A bus is a bus: the rate
says how its values are obtained, not what kind of thing it is. Nothing on the
wire names a recording ring — when a view needs an audio bus's samples, the
**host** asks the audio server to record it (`/tap`, see
[`schemas.md`](schemas.md)) and stops when no open view draws it, and the
server publishes in its segment where those samples landed. A `meter` needs no
recording at all: it reads the per-bus level the engine publishes every block.

**Logical pixels, and the one place that is not.** Every length the wire declares — the place props `w`/`h`/`x`/`y`, a container's `margin`/`gap`, a `window`'s `w`/`h` — is a **logical** pixel: the host multiplies it by the display's scale, so a `h: 28` strip is a 28-pixel-looking strip on an ordinary monitor and a 56-physical-pixel one on a doubled HiDPI screen, and a script never asks what it is running on. `text_size` is logical the same way (it is a glyph scale, so it scales with the rest instead of the font staying tiny). The scale is one number per window, taken from the system natively and from `devicePixelRatio` in a browser, and the sizing table resolves against it **once per change** — never per frame.

The exception is a `scroll` workspace's **content plane**: its `content_w`/`content_h`, its `view_x`/`view_y` pan and its children's place props are content units, and what turns them into pixels is `view_zoom` — physical pixels per content unit, because the plane's pan and zoom are written in the pixels the pointer moves. What the display scale does there is set the **default zoom**: absent a `view_zoom`, a plane starts at the window's scale, so one content unit is one *logical* pixel and a patcher's boxes come up the size they are meant to look. Name a `view_zoom` (in the tree, or by turning the wheel) and it is literal from then on; **`/gui_set view_zoom 0`** (or any non-number) clears it and hands the plane back to its default, which is the only way to ask for a default that has no number of its own — the same shape an empty `theme` uses to drop an overlay. Same split in the heavy views (`waveform`, `spectrogram`), which resolve their signal against physical pixels. So: chrome is logical, a navigable plane is its own — and its zoom is where the two meet.

**Why the default is the density and not a fit to the content.** A plane's content unit is a *display* unit: a patcher box is 96 units wide because that is how wide a box should look. Fitting the zoom to the content instead would make a box's apparent size follow **how many boxes there are** — a three-box graph huge, a fifty-box graph unreadable — and re-zoom the plane on every edit. Zoom-to-fit belongs to a key, not to the default. The distinction generalizes: a view whose content unit is *data* (a `waveform`'s sample, a `pianoroll`'s semitone, a `score`'s staff step) keeps its own window and the display scale must not touch it — a denser screen means more detail over the same span, not a different span.

**Theme groups and the `color` prop.** The host draws every chrome color from one theme — a table of named roles, loaded from `[gui.theme]` or `--theme` (see the configuration chapter) — and the wire customizes it with the same partial table, recursively. A **container** (`window`, `panel`, `scroll`, `track`) may carry a `theme` prop: a JSON object of `"role": "#rrggbb[aa]"` entries overlaying its parent's theme for its whole subtree — a **theme group**, a style scoped to the function of a set of widgets (the transport bar dimmed, the recording strip warm) rather than to any individual part. Groups nest: an inner table overlays the *inherited* one. The leaf case is the single `color` prop on **any** widget — one hex that re-seeds just the roles carrying that widget's function: the accent family (a slider's handle and fill, a button face, a meter's bar), the trace, the first color of the multichannel series cycle, a clip's body. Both are live via `/gui_set` (`theme` rides as its JSON string; an empty value clears), and a `theme` on a GuiDef root persists with the named def, so a standalone bundle ships its look with zero host configuration. Overlays resolve when a def arrives or a set changes them — each widget ends up holding one resolved theme — so the per-frame path pays nothing; there are no selectors and no per-part rules, deliberately.

**Text on the light widgets.** Every text-bearing light widget — `label`, `button`, `toggle`, `text`, `number`, `menu` and the control labels on `slider`/`knob` — takes a `text_size`: a glyph scale over the host's embedded 5x7 bitmap font (default `2.0`, the size everything drew at before the prop existed; clamped to `[1, 16]`). Single-line text that overflows its rect clips with an ellipsis instead of bleeding into the neighbor. `label` additionally takes `wrap` (word wrap on the font's fixed advance, the lines past the label's bottom edge dropped) and `align` (`start`, the default left edge / `center` / `end`, applied per line). All of them are live via `/gui_set`. Inside a `scroll` workspace everything scales with the view's zoom together — the text, the padding, a control's own parts — because a zoom is an enlargement: a box at zoom 2 is the same box twice the size, not a box with oversized text jammed into it.

**The editable `text` field.** A `text` widget is an editable entry: click to focus it and place the caret, type to insert, move the caret with the arrows / `Home` / `End` (word-wise with `Ctrl`), edit with `Backspace`/`Delete`, select by dragging or with `Shift`+arrows (`Ctrl+A` selects all), and cut/copy/paste with `Ctrl+X`/`C`/`V`. The entered string is delivered **the same way a slider's value is — on every edit, never gated on Enter**: an unbound field emits `/gui_event <id> <string>` per keystroke, a bound one (`/gui_bind`) forwards the string straight to the audio server. `multiline: true` allows embedded newlines (`Enter` inserts one; a single-line field ignores it) and a field that grows to its rect; `value` seeds the contents and `/gui_set value` sets them live. The caret and selection are view state, not wire state, so redefining `value` never carries them.

The `scroll` container is **one widget with one gesture path**, and the familiar constrained scroll views are configurations of it rather than separate types: `axis: "y"` with `zoom: 0` *is* a plain vertical scroll view, `axis: "x"` a horizontal strip, and the default is the full 2D plane — drag the empty background to pan both axes, wheel to zoom anchored at the cursor. Its children's place props (`x`/`y`/`w`/`h`) are read in **content units** — physical pixels on the plane, not the logical ones the chrome declares: the content area sizes itself from their extents unless `content_w`/`content_h` name it. `view_x`/`view_y` are the content coordinates at the widget's top-left corner and `view_zoom` is physical pixels per content unit (absent — or set to `0` — the window's display scale, see the units section above); all three are live via `/gui_set` and travel back as the `"view"` payload when a gesture moves them. A widget scrolled outside its container is clipped away — it is neither drawn nor hit.

The two shapes also **bound differently, deliberately**. A constrained scroll view is a bounded document: it clamps to `[0, content - visible]`, so you cannot scroll above its first row or past its last. The free plane is conceptually unbounded — `content_w`/`content_h` only say where its contents happen to sit — so it overscrolls by half a viewport past each edge, which is what keeps every drag direction alive when the plane is sitting at its content's corner (and still little enough that the contents can never be lost off-screen).

**A `pianoroll`'s axis exists before its notes.** A roll is a surface written
*into* — drawn on, or painted from live MIDI — so its time axis is not its
content: an empty roll navigates a grid of sixteen beats read off its own
`tempo`/`sample_rate`, and (like a lane) it can be zoomed out past whatever it
holds. Writing notes into it with `/gui_set notes` therefore does **not** refit
the window onto them — the take grows under a still axis, at the zoom you left
it at — and when the take passes the right edge the axis **pages forward** by
whole windows, so what is on screen holds still while it fills and the writing
continues at the left of the next one. `sel_*`, `view_*` and the wheel/Shift+drag
gestures navigate it as on any other timeline view.

A timeline view (and a lane) shows the transport two ways, and they are different
things: `playhead_at` **anchors the line to the engine clock** (it is the clock
value at timeline position 0, so the line *sweeps* as the audio runs), while
`playhead` is a **static cursor** — where a located, stopped transport sits. Both
are group-wide, so every lane shows the one cursor; a negative value is none.
Clicking a lane's ruler moves the cursor and sends `"locate"`.

A sweep that **repeats** a region — an editor playing a selection on a loop, a
looping clip — sets `playhead_loop_start`/`playhead_loop_len` (the same sample
units, group-wide too): the host folds its own swept position inside
`[start, start + len)`, so the line follows the loop on the **same one anchor**
and a repeating pass still costs one message rather than one per frame. A
non-positive length is the straight pass, which runs on past the region. Anchor
a looped pass at `clock - start`, not `clock`, so the first frame lands where
the reading actually begins. On a `score` the pair is in ms, its own unit.

The lanes of a window share **one navigation group**: they zoom, pan and carry a
playhead as one, and the axis spans the composition (the longest clip end). The
same group model links the heavy views — an explicit `link` id joins or splits
it, and a `/gui_set` of `view_*`/`sel_*`/`playhead_at` on any member applies
group-wide.

**The `score` page carries geometry, not a score.** The host does not read MEI,
MusicXML or any notation format, and never will: the *client* engraves the score
and sends the result as a flat display list, and the host is only the renderer
that fits and tessellates it. That keeps the engraver a purely client-side
dependency (verovio, bundled in the Python wheel) and lets a second client in
another language reuse the same renderer by sending the same display list — and
the engraving itself is shared native code (`clausters-notation` plus
`clausters_core::notation`, over the C ABI), so that second client rebinds it
rather than writing its own. `vb` is the
page's own coordinate system — every primitive is expressed in those units, and
the host scales the page into the widget rect, so the page is
resolution-independent and re-fits on resize with nothing re-sent.

The page shows the transport with the **same two props as a timeline view**, and
they mean the same things: `playhead_at` anchors the cursor to the engine clock
(the clock value at score time 0, so the cursor *sweeps* on its own, one message
per pass), and `playhead` is a static cursor in milliseconds — where a located,
stopped transport sits. Both are negative for none. What turns a time into a
position is the display list's `cursors` track: the score's timemap folded into
geometry by the client (musical time → the page-x of the event sounding then,
and the y-span of its system), so the host interpolates nothing and knows
nothing about music.

Clicking the page emits `"element"` with the MEI `xml:id` of the smallest
primitive under the cursor — a notehead wins over the staff line it sits on — or
an empty id when the press lands on blank paper. A **sounding element owns
everything drawn inside it**: the engraver identifies a note's stem and flag
separately, and the client collapses them onto the note's id, so one note is one
thing to select and drag rather than three. A chord is not collapsed — its notes
nest inside it and each keeps its own id, since one of them can be edited alone. The clicked element is
highlighted, and `selected` sets or clears that highlight from the script.
Because the id is the *client's* own (it engraved it), a driver resolves it
straight back to the note in its own score: nothing but the string crosses the
wire.

**Editing is opt-in (`editable`), and a request, not a result.** A page is a
read-only view by default: a drag does nothing, because the host holds no score
and cannot fulfil an edit the driver will not apply, so a plain plot must not
offer the gesture. `editable: true` turns the drag on (settable live with
`/gui_set <id> editable`). Selection and the `"element"` click are **not** gated
by it — inspecting a page is not editing it. On an editable page, dragging an
element up or down the staff emits `"transpose" <id> <steps>` on release — whole
**diatonic steps**,
counted through the page's own `step` (page units per step, which the engraver
sends because it depends on its unit size, not on the staff scale). Steps rather
than a coordinate: the client's editor moves a note by steps, and a step is
exact where a page position would have to be read back through the engraver's
frame. The host owns no notation, so it cannot apply the edit; the driver does,
re-engraves, and replaces the drawing with a single
`/gui_set <id> display_list <json>` — the drawing layers only (`vb`, `glyphs`,
`prims`, `cursors`, `step`), the same ones the widget was defined with.

The host draws the drag as it happens, displacing the element and everything it
owns by whole steps, and **re-deriving its ledger lines** at the new pitch: they
are drawn per staff rather than inside the note, so they cannot travel with it —
the host reads the staves back out of the engraving (the wide horizontal strokes,
clustered a space apart) and draws what the displaced notehead needs, which is
also how they disappear when a note comes back onto the staff.
That displacement **stands after the release until the new page arrives** —
the answer is one message away, and retiring it first would show the old pitch
for a frame. Replacing the page keeps the widget's own chrome (`playhead`,
`playhead_at`, `sample_rate`, `selected`), so the edited note stays selected
across the round trip; MEI ids survive an edit, so it is still the same id.
