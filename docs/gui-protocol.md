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
| `/gui_query id` | Ask for a widget's state. Replies `/gui_info id type key value …` — **what the widget is now**, which is the def's props with **every edit the user has made since** laid over them: a dragged control's value, a moved clip's `offset`/`dur`, a lane's mute/solo/level, a plane's `view_x`/`view_y`, an edited curve's `points`, a roll's `notes`, a score's `selected`. (A `/gui_set` needs no such correction — it is already the document.) The reply is flat OSC arguments, so it carries **scalars only**: a structural prop nothing edits (`theme`, `boxes`, `data`) is not reported, and asking for one means keeping the tree that was sent — but an **edited** structure is reported as the JSON **string** its own `/gui_set` already accepts (`points`, `notes`, `osc`), so what a query gives back is what a set would take. The `axes` pair is recorded **flat** (`ruler`, `view_start`, `min`, …) precisely so a query can answer it, while the node's `type` is kept as the tree wrote it. An **empty type** (`""`) means no such widget — the host answers either way, as the audio server replies even on a miss. |
| `/gui_bind id "server" address prefix…` | Forward this widget's value **straight to the audio server**, bypassing the script: on every change the host sends `address` with the fixed `prefix` arguments followed by the value (e.g. `"/node_set" 1001 "freq"` makes the widget send `/node_set 1001 freq <value>`). A bound widget stops emitting `/gui_event`. |
| `/gui_ack seq docVersion [source generation…] [reason]` | **Answer the edits this host emitted, up to `seq`.** The reply `/gui_event` never had, and the thing that lets a host draw an edit before it is confirmed without lying about it. There is no success flag: the values the owner decided ride as ordinary `/gui_set`s **in the same bundle**, and *applied*, *applied transformed* and *refused* are one message — a refusal is simply the previous value pushed back. Send it **always**, including when nothing changed. `seq` is monotonic, so one number retires every edit at or below it and a lost acknowledgement is harmless; `docVersion` is the document's version after applying; each `source generation` pair reports material whose *content* changed while its identity stayed put (a destructive edit), which is the only thing that can tell a reader its copy is stale; `reason` is informational and read by nothing in the mechanism. |
| `/gui_bind id "widget" target prop` | Apply this widget's value to **another widget's property**, as a `/gui_set target prop <value>` would — a `menu` flipping a `stack`'s `index`, a slider driving a plot's `max`. A multi-value edit-back payload rides as the JSON string the prop already takes. A binding fires an **apply, never another binding**: the target's own binding does not fire from it, so two widgets bound to each other settle instead of cascading (stated, not detected — the chain is one hop by construction). |
| `/gui_bind id` | (no target) Remove the binding; the widget emits events again. |
| `/gui_load name` | Instantiate a **persisted** GuiDef by name (the host replays it as its saved `/gui_def`). Needs a data directory. |

There is no save command: a GuiDef whose root carries a `name` prop is
**persisted on `/gui_def`**, the way a named def is persisted on `/def_send synth`. That
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
- **`children`** nests (containers only: `window`, `layout`, `plane`,
  `field`).
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
  (`layout`, `plane`, `field`, `nodes`, `canvas`, every `signal`, a wrapped
  `label`, a multiline `text`). A natural size follows
  the host's sizing table and the widget's own `text_size`/`label`, **never
  its data** — a longer string or another thousand samples never move it, so
  a `/gui_set` never relayouts the window. In a `free` container `x`/`y`
  (+ `w`/`h`) position the child absolutely, and a child with none of the four
  overlays the whole area. A container additionally takes `margin` (inset
  before its children, default 6), `gap` (between children, default 6) and
  `cols` (a fixed `grid` column count; default near-square) — except a
  `layout` whose `flow` is `stack`, which arranges nothing and so takes only
  the `margin`.
- **`hug`** (`window`, `layout` — including a `stack` — a number or a
  boolean, live via `/gui_set`): the container's own natural size becomes
  **the composition of its children's**, so it wants exactly what it holds
  instead of the share the layout would give it. A `row` adds its children up
  along its axis and takes the largest of them across it, a `col` the other
  way round, a `grid` counts its cells, a `free` container reaches its
  children's placements, and a `stack` takes the largest of **every** page
  (not the shown one, so flipping a pager does not resize it). It is asked of
  the whole subtree, so a plain container nested in a hugging one is measured
  too; an axis a child leaves elastic — a `plane`, a `field`, a `signal` — is
  one the hugging container hands back to the layout. On a `window` root it
  sizes the window itself: the OS window opens as big as its content on the
  axes that settle and keeps the declared `w`/`h` on the others. (In a page
  there is no window to size — the element owns its box — so a mounted GuiDef
  lays out in the box it is given and only the containers inside it hug.) It
  is **off unless asked for**: every def written before it lays out exactly as
  it did.
  **What may size, and what may not.** A hugging container reads the props
  that settle at a *mutation point* — a `label`'s `text`, a `button`'s or a
  `toggle`'s `label`, a `menu`'s `options` — the same place a `theme` resolves.
  A **value** never sizes anything: the option a menu is on, what a `text`
  field holds, what a `number` reads, a `signal`'s samples. So a stream of
  values still cannot relayout a window, hug or no hug, and a control does not
  resize under the gesture writing it. Still one pass, no
  measurement pass and no constraint solver — the composition is a walk over
  numbers each widget already knew; when a layout needs negotiation, the
  answer is still explicit sizes.
- **`bind`** as an inline prop registers a binding declaratively, so a saved
  GuiDef carries its own (no separate `/gui_bind` at boot). It is the
  `/gui_bind` tail as an array: `["widget", 20, "index"]`, `["server",
  "/node_set", 1000, "freq"]`, or the bare address-first `["/node_set", 1000,
  "freq"]`, which is the same server binding with the keyword left out.

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
| `/gui_event id seq version <value>` | A control changed: a float (`slider`/`knob`/`number`), an int (`toggle` 0/1, `menu` index, `button` press), or a string (`text`). |
| `/gui_event id seq version <tag> <flat values…>` | A view wrote data back. The tag names *what* was edited; the values are flat OSC primitives (never a new address — see below). |

**`seq` and `version` are the second and third arguments of every event**, before any tag, so one rule reads them all whatever the payload. `seq` is the host's stamp on the edit, and what an acknowledgement names. `version` is the document version the edit was made **against** — the last one an acknowledgement reported to this host — so an owner can tell an edit made against the picture it is looking at from one made against a picture that has since been replaced. Both are zero when the host has nothing to say: an unstamped event is one nobody will acknowledge, and an unstated version is one an owner applies unchecked. See *Answering an event* below.
| `/gui_closed id` | The window was closed by the user. |

The **edit-back payloads**:

| Tag | Arguments | Sent by |
|---|---|---|
| `"points"` | `t v shape curve` per break-point | the `bpf` editor, and an automation `clip` — one payload, whichever view drew the curve |
| `"notes"` | `start dur pitch velocity channel` per note | the `pianoroll` view (and a `clip`'s roll) — MIDI notes edited |
| `"osc"` | `time label` per event | the `pianoroll` view — OSC event markers edited |
| `"note"` | `pitch velocity state channel` (ints; state 1 = press, 0 = release) | the `piano` keyboard played — MIDI-shaped, translatable 1:1 to note-on/note-off |
| `"range"` | `min max` (MIDI notes) | the `piano`'s visible range panned or zoomed |
| `"clip"` | `offset dur start` (timeline units; `start` is the source frame the clip's own time zero reads) | a `clip` moved or trimmed. A move changes the offset; an edge drag **trims** — the placement and the window over the material move together, which is what makes a trim hide frames instead of compressing them |
| `"layer"` | `name` (`"placement"`, `"take"`, `"notes"`, `"points"`, and `points:1` for a second layer of one role) | the **edit layer** a press selected on a container that layers its contents. Sent only when it *changed*, so pressing twice on the same curve says it once |
| `"split"` | `t` (the clip's own time) | the clip's split verb (`e`). A **request**: the host holds no composition, so it says where the cut falls and the owner answers with the tree that then stands — two windows over one source, on a memory view |
| `"join"` | `id…` (the clips to read as one, in axis order) | the clip's join verb (`j`), over the run of clips that touch the one under the cursor |
| `"mute"` / `"solo"` | `0|1` | a lane header's toggle worked — the tag is the lane prop that changed |
| `"level"` | `f` (0..1) | a lane header's fader dragged |
| `"height"` | `h` (logical pixels) | a lane resized with **Ctrl+wheel** — the host applies it to the lane under the cursor and says so, and a driver that wants every lane the same thickness echoes it onto the others |
| `"element"` | `id` (a string: the MEI `xml:id`; empty = the selection was cleared) | a `score` page clicked — the engraved element under the cursor |
| `"transpose"` | `id position` (the MEI `xml:id`; the whole diatonic staff position reached, from the staff's top line, positive = up) | a `score` element dragged up or down — a pitch edit *requested*, since the host holds no score. **Absolute**, so a resend is harmless and a re-engraved page needs no rebasing |
| `"focus"` | `1|0` (gained / lost) | the keyboard focus moved onto this widget or off it — a press or a Tab (a `/gui_set focus` is not echoed, like every other set). A **notification**, not a value: it is sent even from a bound widget, since a binding says where the widget's *value* goes |
| `"wire"` | `src_box outlet dst_box inlet` (ports by name; a rate mismatch is refused at the gesture) | a patcher cord drawn `outlet -> inlet` |
| `"move"` | `index x y` (box index; canvas units) | a patcher box dragged — one payload per moved box, so the driver owns the geometry |
| `"locate"` | `position` (timeline units) | a lane's time ruler (or its empty space) clicked — the transport is being seeked there |
| `"undo"` / `"redo"` | — | a window shortcut (Ctrl+Z, Ctrl+Shift+Z or Ctrl+Y). **Addressed to the window, not to a widget**: the id is the window's, the way `/gui_closed` names one. The host holds no history — the log lives with the document — so this is a *request*, and the owner answers with the state that now holds, exactly as it answers a drag |
| `"selection"` | `start len` (samples, always whole), plus `min max` where the sweep restricted the y axis too | a selection dragged on a timeline view |
| `"sample"` | `channel frame value previous` — the frame as an OSC **long** (a float runs out of integers at 16.7 million, six minutes of audio, and a sample index is exact or it is the wrong sample) | one sample dragged on a navigable trace, under the `sample` gesture step. **Absolute and carrying its own inverse**, so the owner can apply it and undo it without having remembered anything. The host draws the held value over the picture, marked, and lets go when the edit is acknowledged — so an owner acknowledges *after* pushing the material that now holds, or the old value blinks back |
| `"draw"` | `channel start <values blob> <previous blob>` — the two runs as little-endian `f32` blobs, the bulk convention `/buffer_setRange` and the clipboard already follow | one **stroke** over a navigable trace, under the `draw` gesture step. **One intent per stroke**, not per sample: what the hand did on the way is the pending drawing's business, and the owner gets the run it ended with — plus what it replaced, so the edit is invertible |
| `"cut"` | `start len` (samples) | Ctrl+X over a selection. The host owns no data, so this is a **request**: the owner cuts and answers with what the composition now is, and the length change a cut implies is the owner's to decide |
| `"paste"` | `position kind json` plus one **blob** per bulk payload | Ctrl+V. The clipboard travels *with* the request — it is the host's, so a block copied in one window pastes against an owner that never saw it. `kind` is the clipboard's (`text`/`elements`/`samples`/`spectral`), `json` the whole typed document, and the blobs are the payloads it names, interleaved little-endian `f32` |
| `"refused"` | `verb reason` | the host could not do its own half — a copy whose source it cannot read (a mapped overview, a live view), a paste whose payload did not travel, a **stroke where a pixel is more than one sample**. Said out loud, because a key or a pencil that silently does nothing teaches that it sometimes does not work |
| `"view"` | `start len` (samples), or `x y zoom` on a `plane` | the navigation window zoomed or panned — the timeline group's shared window, or a 2D workspace's plane |
| `"view_y"` | `start len` (0..1) | the vertical display window zoomed or panned |
| `"view_x"` | `start len` (0..1) | an element's **own** horizontal window zoomed or panned — a navigable `spectrum`'s frequency axis, which is in no navigation group (a group's shared window reports `"view"`) |

**A gesture that moves nothing says nothing.** An axis pressed against a bound — zoomed all the way out, panned to the end, or down at the resolution of what it measures — goes on receiving wheel steps and drag motion, and reports none of them: `"view"`, `"view_x"` and `"view_y"` are emitted when the window actually moved, never once per notch. A script counting events is counting movements.

Edited data flows as a **payload, never a new address**: the `/gui_*` family does
not grow per widget.

### The clipboard, and who may read what

Ctrl+C, Ctrl+X and Ctrl+V over a selection split exactly where the host's
authority does, and the split is worth stating because it is the same one
everything else in this chapter follows.

A **copy is a read**, so the host does it: it takes the selected span out of the
material it has *mapped* and puts it on its own clipboard, typed, carrying the
rate it was taken at. A source the host cannot read — a mapped peak overview has
no samples behind it, a live view has no addressable past — **declines and says
so** (`"refused" "copy" <reason>`), because a block of silence on the clipboard
is the one answer worse than no.

A **cut and a paste change data the host does not own**, so they leave as the
`"cut"` and `"paste"` events above and the owner answers with what the
composition now is. A paste carries the clipboard **with** it rather than the
owner keeping one of its own: the clipboard is the host's precisely so that a
block copied in one window can be pasted in another, against an owner that never
saw the copy.

**Which view answers** is the one under the pointer, since a selection is
already where the pointer has been — and, when the pointer is over none of them,
the view carrying the window's most recent selection. That fallback is what a
sweep to the first or last sample needs: it leaves the pointer in the window's
margin, or off the window altogether, with the selection plainly drawn on
screen.

**Playing a selection is not a clipboard operation**, and the distinction is
worth keeping: looping it moves no data at all. The loop region is group state —
`playhead_loop_start`/`playhead_loop_len`, on every widget with a time axis — and
what sounds is the server reading the material it already holds. A copy is for
carrying a block somewhere the material is *not*.

The clipboard is one **typed document** — `text`, `elements`, `samples` or
`spectral` — and its bulk rides *beside* it as blobs rather than inside it as
base64, which is the same rule every other large payload here follows. A
`samples` block is never resampled in transit: resampling is an edit, and an
edit is something an owner performs and logs.

### Answering an event

An event is a **proposal**, not a fact. The host owns no data — a placement belongs to the arrangement, a sample to a source, a note to whoever holds the timeline — so what it emits is an edit for the owner to apply, and between the gesture and the answer there is a gap the host draws across. `/gui_ack` is what closes it.

The rule is one line on each side. The host stamps every event with a monotonic `seq` and keeps it *pending*. The owner applies what it can, pushes whatever state that left as ordinary `/gui_set`s, and ends the **same bundle** with `/gui_ack seq …`. The host then retires every pending edit at or below that stamp and adopts what arrived.

Three things follow, and they are the whole design:

- **There is no branch for a refusal.** *Applied verbatim*, *applied transformed* and *refused* are one message, because the value pushed is simply what the document now says — and a refusal is the previous value. An owner that snaps a placement to a musical grid, or declines an edit to material a generator produced, says so by pushing what it actually has.
- **An unanswered edit is one the host waits on forever**, so the acknowledgement is sent even when nothing changed. Silence is not a refusal; it is a hang.
- **The stamp is what tells two gestures apart.** Without it an answer to one edit is indistinguishable from an answer to another on the same widget, which is exactly the case a host with an edit still in flight is in.

**The version answers a different question, and needs both directions.** `seq` says *which of my gestures is this an answer to*; `docVersion` says *are we talking about the same state*. That second one is what catches the document moving by a route that was never a gesture — a script editing the arrangement, a second editor, a re-render — which no record of the host's own edits can see. So the acknowledgement reports the version, the host remembers it, and the host names it back on its next event. An owner that finds an edit made against a superseded version **refuses it as stale and pushes the state that holds**, which needs no new path on either side: the host adopts it exactly as it adopts a snap, and the `reason` is what distinguishes *someone else changed this* from *not here*. Merging the two edits instead is deliberately not done — an edit-back payload is absolute *and* whole (a roll's `"notes"` is the list, not a diff), so applying a stale one would silently drop whatever arrived in between.

**A redefine drops what that window had in flight.** `/gui_def` on an open window replaces its whole tree, so an edit still pending against the old one has nothing left to resolve to — its widget may be gone, or its id may now belong to something else. The host forgets those pendings itself, exactly as `/gui_free` does, and an owner is not expected to acknowledge them.

The acknowledgement is a **verb rather than a property** because it is scoped to the conversation and not to the tree: `seq` is per client, so two clients driving one window would collide on a single prop, and it does not round-trip, which a property here has to. It rides *after* the value pushes in the bundle, so the host never retires an edit before the state that edit produced has arrived.

## The model: containers, axes and elements

This is what a `type` names. The wire's shape has not changed — a node is
still `{id, type, props, children}` — but the twenty-nine widget names the
catalog had grown are gone: they spelled one idea several ways, and what is
left is the model they were points of.

| Kind | What it is | Types |
|---|---|---|
| **Container** | owns 0, 1 or 2 **axes**, and so a coordinate system its children are placed in | `window`, `layout`, `plane`, `field` |
| **Element** | draws against the axes of the container holding it; owns no navigation | `signal`, `notes`, `curve`, `score`, `keys`, `nodes`, `meter`, `canvas`, `label` |
| **Control** | an element with no axis and a value | `slider`, `knob`, `number`, `button`, `toggle`, `text`, `menu` — **unchanged** |

The controls do not move: a `knob` names what it is, and nothing about it says
the same thing another type also says. What is being replaced is where the
catalog spells one idea several ways.

### The containers

| Type | Axes | Properties | Replaces |
|---|---|---|---|
| `window` | 0 | a root; `title`, `w`, `h`, `flow`, `margin`, `gap`, `cols`, `hug`, `theme` | `window` |
| `layout` | 0 | children arranged by **`flow`** — `row`, `col`, `grid`, `free` or **`stack`** (one child at a time, the one at `index`) — plus `margin`, `gap`, `cols`, `hug`, `theme` | `panel`, `box`, `stack` |
| `plane` | 2, **locked to one scale** | a pannable, zoomable plane in content units: `axis`, `zoom`, `content_w`/`content_h`, `view_x`/`view_y`/`view_zoom`; with `boxes`/`cords`, the patcher | `scroll`, `patch` |
| `field` | 2, **independent** | the time/value container: an `axes` pair, plus lane chrome (`label`, `height`, `header_w`, `mute`, `solo`, `level`) or a placement (`offset`, `dur`) | `track`, `clip`, `timeruler` |

`flow` is what the catalog spelled `layout`, on **every** container that has
an arrangement: the model spends the word `layout` on the container itself.

`stack` stops being a type because it never was one: a container showing one
child is a layout with a **selection** instead of an arrangement.

A `field` is told apart **by what is on it**, in this order: a placement
(`offset`/`dur`) makes it a **clip** on its parent's x axis; a bare strip of a
given thickness `h`, with nothing placed on it and no lane chrome, is the
free-standing **ruler**; anything else is a **lane** — including an empty one,
which a multitrack opens all the time and which must not read as a ruler.

### The elements

| Type | Replaces | How the old name is said |
|---|---|---|
| `signal` | `waveform`, `spectrogram`, `plot`, `scope`, `spectrum`, `phasescope` | **`view`** (`trace` default / `spectrum` / `spectrogram` / `phase`) × the **source** (`bus` = forward-only; `data`/`blob`/`buffer`/`path`/`cache` = addressable) × the **capabilities** `navigable`, `selectable`, `editable` × what it **measures** (`measure`: `peak` default / `rms`, see above). `navigable: 0` over addressable samples is the static plot — the whole of it, since a view that does not navigate also resolves its source as the sequence itself rather than as a take, and auto-fits a value axis nobody named. Over a **bus** the missing piece is a past: `retention` (seconds, 0 = none) is the policy that supplies one, so `view: "spectrogram"` + `bus` + `retention` + `navigable` is a **waterfall** — the host keeps that many seconds, analyzes them into columns as they arrive, and the time axis navigates like a file's. It is a policy of the axis, not of the drawing: the same seconds mean the same seconds at any frame rate, `window_size` or `hop`, and a `/gui_set` of it resizes the history live. A live axis **follows the newest until you navigate it**, and then stays where you put it. `navigable` over a **spectrum** means something else, because that view's x is not time but **frequency**: an axis addressable with no retention at all (every bin is there every frame), navigated on a window the element carries alone — `view_start`/`view_len` (`axes.x.start`/`len`) in normalized display units over `[0, Nyquist]`, panned by dragging the axis, zoomed with the wheel under the cursor, reset with `R`, reported as `"view_x"`. It joins no navigation group: nothing else in a window measures in hertz along x. It is opt-in — a bare `spectrum` is the watching spectroscope — which is the one place `navigable` does not default to on. The zoom stops at the **resolution of the analysis**: below a few FFT bins across the whole body the curve is interpolation between two neighbours rather than a measurement, so the floor is derived from `fft_size` and the sample rate (and is therefore not a constant — a bin is a twentieth of a log axis at 500 Hz and a thousandth of it near Nyquist). The floor applies to what is **shown**, not to what is stored: `view_start`/`view_len` are the window that was asked for, from a gesture or from `/gui_set` alike, and the axis opens them wherever they are finer than it resolves. So a scripted window narrower than the bins is drawn — and reported — opened up, and a pan down the axis that has to open the window gives the asked-for one back on the way up rather than spending it |
| `notes` | `pianoroll` | unchanged properties |
| `curve` | `bpf` | unchanged properties |
| `nodes` | `nodetree` | it is an element, not a widget named after a tree |
| `keys` | `piano` | a keyboard is an element; `piano` reads as an instrument |
| `score`, `meter`, `canvas`, `label` | themselves | already one thing each |

The six signal names were six points of one product — `waveform` is a
navigable trace over addressable samples, `scope` the same trace over a bus —
so a `signal` says the point and the name falls out of it:

```json
{"id": 7, "type": "signal", "view": "trace", "path": "take.f32",
 "navigable": true}
```

### One layer is edited at a time

A container that layers editable things — a `clip` today, an audio editor's
view next — draws several of them on **one rectangle**, so a press is claimed by
several at once: the clip's move, its edges, a roll's notes, a curve's points.
The rule that decides between them is one sentence, and it is deliberately not a
list of precedences between kinds of thing:

> **One layer is active at a time, and it is the only one that acts or offers an
> affordance.**

Two props say it on the wire, and they are two questions rather than one:

- **`layer`** — which layer a hand is editing: `"placement"` (the container
  itself — where it sits, how long it is; `"clip"` is accepted as its name on a
  clip) or the role of one of its contents, `"take"`, `"notes"`, `"points"`, with
  `points:1` naming a second layer of the same role. Live via `/gui_set`, and
  moved by a press.
- **`hidden`** — which layers are **not drawn**, space-separated; empty draws
  them all. What is hidden is not edited either, so hiding the layer in hand
  hands it back to the placement.

**A press selects the layer it lands on**, and what "lands on" means is the
layer's *own material* — a break-point, the line between two of them, a note —
never the rectangle it shares with the container. That is what leaves the
background, and the affordances drawn on it, to the container: dragging a clip's
empty space moves the clip and takes the hand off whatever was being edited
inside it. The active layer is asked first, so what is already in hand keeps the
pixels it draws on; the change is reported as the `"layer"` payload, once.

**A layer that cannot be edited is never selected by pointing at it** — the
press falls through to the container, which is why a clip whose notes are a
rendering (`editable: false`) still moves and resizes. Activating such a layer
with `/gui_set layer` is a different statement, and the element still refuses
the edit itself.

The set of layers is the container's contents, in the order they are drawn, so
nothing here is a list of widget types: a container that grows a fourth kind of
content grows a fourth layer.

### The axes own the chrome

A ruler, a navigation window, a selection, a playhead and a value range describe
the **container's axes**, not each view drawn against them, so they ride under
one `axes` key rather than as flat props of every element. `x`/`y` are already
the free-placement props, which is why the pair is nested instead of bare:

```json
{"id": 3, "type": "field",
 "axes": {"x": {"unit": "beats", "tempo": 2.0, "start": 0.0, "len": 96000.0},
          "y": {"unit": "db", "min": -1.0, "max": 1.0}}}
```

Under an axis a property drops the axis marker — `x.start` is the old
`view_start`, `y.unit` the old `ruler_y`:

| Axis | Properties |
|---|---|
| `x` | `unit` (`time`/`samples`/`beats`/`off`; `ruler` is accepted as its old name), `start`, `len`, `tempo` (beats per second), `beat_at`, `quant` (**beats per bar** — the grid a `bar:beat` label counts on, not a length in samples), `sample_rate`, `link`, `sel_start`, `sel_len`, `playhead`, `playhead_at`, `playhead_loop_start`, `playhead_loop_len` |
| `y` | `unit` (`norm`/`db`/`bits`/`percent`/`hz`/`off`), `start`, `len`, `min`, `max`, `bit_depth`, `sel_min`, `sel_max` |

**A selection is a count of samples.** `sel_len` is how many the selection
holds and `sel_start` is the first, snapped when they are set and when a sweep
writes them, so the `"selection"` event always reports whole samples. Zoomed in
far enough that a pixel is worth a fraction of a sample, an unsnapped selection
would cover the space *between* two samples — a region holding no data, which
can be neither played nor cut. The snap takes the samples the sweep **passed
over**, not the ones it came nearest, so a sample joins when the cursor reaches
it; and the band is drawn from halfway before the first selected sample to
halfway after the last, so the edges fall between what is in and what is out.
The rule belongs to the navigation group rather than to any one view, which is
why it holds for a spectrogram laid over a waveform too: they share the
selection.

**A selection may also be restricted on the y axis** — under the `select_box`
gesture step, never under a plain drag. A sweep with height over a view that
measures a value carries `sel_min`/`sel_max` as well — the band of values it
covered, in the axis' own units, never in pixels — and the event grows by
exactly those two numbers: `"selection" start len min max`. A sweep that stayed
at one height, or one whose plan asked for a span, reports the two numbers it
always did, so a reader of the old form keeps working.

**A plain drag stays a time span**, and that is a decision rather than a
default waiting to be changed: a drag over a waveform means *this stretch of
time* in every editor there has ever been, and what a band of amplitudes is
good for — gate this range, copy only these peaks — is the script's business.
So the script names the step, per modifier, exactly as it names any other. An
empty or inverted pair (`sel_max <= sel_min`, the default) is *no restriction*,
the same convention `sel_len <= 0` uses on the other axis.

Two things follow from what each axis measures, and they are the whole of the
rule. The **rounding** differs because the data does: time is discrete, so a
sweep takes the samples it passed over, while a value axis is continuous and
the range is simply what the hand drew, ordered and clamped to `min`/`max` — an
axis whose values *are* discrete (a `notes` element's pitch) takes the
passed-over rule in its own unit, whole semitones included at both ends. And the
range is **per widget** where the span is per group: linked views share one time
axis but measure different things vertically, so a range held in common would
restrict a spectrogram in hertz by a waveform's amplitudes. A spectral view's
second axis is a band of bins rather than a value, and does not travel here.

**`min`/`max` are the value domain, and every view of a signal is drawn over
it** — the trace of a take, a plot, a live scope, and the navigable waveform,
which used to ignore the pair and pin itself to full-scale amplitude. Omitted, the domain is `[-1, 1]`: audio.

- **A named domain is ruled as a plain value axis.** `db`, `bits` and `percent`
  are units of *full scale* — a rung at -6 dB says nothing over `[20, 20000]` —
  so they apply to the default domain, and an axis with a domain of its own is
  labelled with its own numbers whatever `unit` says.

**How a trace is inked, in every view of a signal.** A column is the **min/max
of what the signal did in that pixel**, and it is never extended to reach the
zero line: the solid body of a zoomed-out waveform is the data filling it, not a
fill the drawing adds. That is why there is no "filled" switch and no zoom at
which one would belong — a subsonic signal has far more samples than the screen
has pixels and is still a curve, while audio crosses the whole span inside one
column and is still a body. Two floors keep it legible: a column is inked at
least one pixel in each direction, so a flat stretch stays visible, and once the
zoom is deep enough that consecutive samples stand three `point_radius` apart,
each sample is **marked with a dot** — the line between them is interpolation,
the dots are the data.

**`measure` is what the picture measures**, and it is a factor of the signal
element rather than a widget of its own: `peak` (the default — the min/max
envelope above), `rms` (the symmetric body about zero at the level the signal
held, drawn in the `trace_body` colour role), or **several at once**, named in
one space-separated string. It is live on `/gui_set`, because a picture is read
by turning its measures on and off.

- **The classic editor picture is `"peak rms"`: one body, a drawing per
  measure.** The level is drawn inside the envelope by the same renderer placed
  twice, and the order is the host's — the envelope is the outer shape, so it
  goes under whatever order the names were given in. It is one element and not
  two because **every view of a signal paints its own field before it draws**:
  two of them on one rectangle are not layers, the second hides the first. One
  element is also one axis, one ruler, one selection, one playhead and one
  upload of the samples.
- **A body fades out at sample zoom instead of switching off.** Past the
  polyline threshold the level over one sample *is* that sample, so the body
  would restate the trace; it follows the zoom out with the same weight the
  level crossfade uses, and the samples themselves are what remain.
- **A source that cannot measure draws no body.** A peak cache written before
  the format carried the mean square (CLPK v1/v2) has an envelope and no
  energy, and zeros would be a measurement — silence — over material that is
  not silent.

An `axes` pair works on `/gui_def` and on `/gui_set` alike (there it rides as
its JSON string, the `theme` convention). Everything the container does **not**
own stays where it is: an element's source (`buffer`, `path`, `cache`,
`bus`, `rate`, `channels`, `base_bucket`), its presentation's own parameters
(`fft_size`/`window_size`, `hop`, `db_floor`/`db_ceil`, `freq_scale`,
`colormap`), and every place prop (`w`, `h`, `weight`, `x`, `y`).

**`data` is the one source that is also live.** A `/gui_set data` replaces
**inline** samples, which is how an owner that has applied an edit pushes the
material that now holds — and therefore how a pending edit can be let go of
without the edit disappearing with it. It is refused on a source that names a
file, a cache or a server buffer: those are re-read by resolving the resource
again, and pushing samples at one would leave the picture half from each.

**`reload` is the other half of that sentence, for a source that is mapped.**
`/gui_set reload 1` makes the element forget what it resolved, so the loader
reads its file, cache or server buffer again — the way an owner says *the
material is where it always was, and it moved*. A source with nothing behind it
ignores it rather than erasing itself. Between the two, an owner that has
applied an edit can always correct the picture, which is what lets the host drop
a pending edit without the edit disappearing with it.

### The builders keep their names; the wire does not

The old type names **no longer parse**: a node saying `type: "waveform"` is an
unknown type, laid out and not painted, like any type this host does not have.
A GuiDef saved in the old spelling (a bundle, a named def in the host's store)
has to be re-saved from a current client.

What did not change is what a script types. Both clients still offer a builder
under each old name — `panel`, `stack`, `scroll`, `waveform`, `plot`, `scope`,
`track`, `clip`, `timeruler`, … — as **shortcuts** that build a model node with
the props of one common case, and `layout`, `plane`, `field` and `signal` sit
beside them for the cases no shortcut names. The catalog below is where a
shortcut's own props are documented; what a client *emits* is always the model.

One name is **unclaimed**: `box`. The catalog spent it on a synonym of `panel`,
and the model wants it for a patcher's box — but a plane's boxes are still its
`boxes` prop, because making them child elements is a change of behavior (ids,
layout, per-box hit-testing and edit-back) rather than of spelling. Until that
lands, `box` names nothing.

## The widget catalog

The names below are **builder** names in both clients, not wire types — each
is a shortcut onto one point of the model above, and this table is where its
own props are documented. The `type` a node carries is always the model's.
The authoritative per-widget reference — every property, its default and its
meaning — is the [Python client's builder
documentation](https://clausters-python.readthedocs.io/), since that is how a
script actually names these. The catalog itself:

| Type | What it is | Notable properties |
|---|---|---|
| `window` | A top-level window (a GuiDef root) | `title`, `w`, `h`, `layout`, `margin`, `gap`, `cols`, `hug`, `theme` |
| `panel` | A nestable container | `layout`, `margin`, `gap`, `cols`, `hug`, `theme` |
| `stack` | A container showing **one child at a time**, the one at `index`: it fills the container, and the hidden pages are neither laid out nor drawn while keeping their place in the tree (so a heavy view keeps its GPU slot and its bus reads across a switch). An `index` outside the children shows nothing — a blank page, not a clamped one. Tabs, a pager and a waveform/spectrogram switch are this plus a control bound to `index` | `index`, `margin`, `hug`, `theme` |
| `scroll` | The **2D workspace**: a container whose children live in a virtual content area seen through a panning, zooming window. General first — the default is the free plane; the constrained scroll views degenerate from it by configuration | `axis` (`both`/`x`/`y`), `zoom` (0 disables the wheel zoom), `content_w`/`content_h`, `view_x`/`view_y`/`view_zoom`, plus `layout` (default `free` here), `margin`, `gap`, `cols`, `theme` |
| `label` | Static text | `text`, `text_size`, `wrap`, `align` (`start`/`center`/`end`) |
| `knob`, `slider`, `number` | Continuous controls | `min`, `max`, `value`, `label`, `text_size` (`vertical` on a slider) |
| `button`, `toggle` | Momentary / latching | `label`, `value`, `text_size` |
| `text`, `menu` | An editable string field, a choice | `value`, `multiline` / `options`, `index`, `text_size` |
| `meter` | A bus level, read from the server's shared segment | `bus`, `rate` (`audio` default / `control`), `min`, `max` |
| `scope` | An oscilloscope over `channels` adjacent buses from `bus` (trigger searched in the first channel; a lock/free read-out) | `bus`, `rate` (`audio` default / `control`), `channels`, `overlay`, `window_ms`, `trigger`, `hold`, `min`/`max`, `ruler` (ms) / `ruler_y` (value; `"off"` hides) |
| `phasescope` | A goniometer (stereo field) over the audio bus pair `bus` / `bus + 1` | `bus`, `window_ms`, `hold` |
| `spectrum` | A live spectroscope: one color-coded curve per channel over `channels` adjacent audio buses. With `navigable` its **frequency axis** zooms and pans (`view_start`/`view_len`, reported as `"view_x"`) | `bus`, `channels`, `fft_size`, `db_floor`/`db_ceil`, `freq_scale` (`log`/`linear`/`mel`/`bark`; `log_freq` is the legacy boolean alias), `averaging`, `peak_hold`, `navigable`, `view_start`/`view_len`, `ruler` (Hz) / `ruler_y` (dB; `"off"` hides) |
| `nodetree` | The server's node graph, live | `group`, `controls` |
| `waveform` | The editor-grade waveform: multichannel lanes, rulers, selection, playhead, linked navigation | the data (`data`/`blob`/`buffer`/`path`/`cache`), `channels`, `ruler`, `ruler_y`, `sel_*`, `playhead_at`, `playhead`, `playhead_loop_*`, `y_start`/`y_len`, `link`, `offset` |
| `spectrogram` | The editor-grade spectrogram, the same chrome. Over a live `bus` with a `retention` span it is the **waterfall**: the last N seconds, rolling | the data (or `bus` + `retention`), `window_size`, `hop`, `freq_scale` (`log_freq` is the legacy boolean alias), `db_floor`/`db_ceil`, `colormap` |
| `bpf` | A drawable break-point envelope, played by the server's own shape math | `points`, `min`, `max`, `duration`, `exp` |
| `pianoroll` | The editor-grade piano-roll: a keyboard, a MIDI-note grid, a velocity lane and an OSC-event lane; the same chrome and navigation as the heavy views | `notes` (`start dur pitch velocity channel` quintuples), `osc` (`time label` pairs), `min`/`max` (pitch window), `snap`, `velocity`, `osc_lane`, `midi_in` (live MIDI painting: the native host opens its virtual input port and paints incoming notes — at the running playhead, or step-entry on the `snap` grid), `ruler`, `sel_*`, `playhead_at`, `playhead`, `playhead_loop_*`, `y_start`/`y_len`, `link` |
| `piano` | The playable virtual keyboard, laid out with real piano proportions; its overview strip pans/zooms the visible MIDI range, and it plays server voices itself when `voice` is set (an `/synth_new` per key press, a `gate 0` per release) | `min`/`max` (visible range; min snaps to a white key), `active_min`/`active_max` (keys outside draw grayed and are inert), `pan` (0 freezes all range navigation), `overview`, `velocity` (fixed; unset = from the press height), `channel`, `voice`/`voice_args` (host-managed voices), `label` |
| `plot` | A static plot of a signal: multichannel lanes, x/y rulers, a hover readout, and **views** (`signal`, `spectrum`; the set is extensible) — measurement without navigation | `data`/`blob`/`path`, `channels`, `view`, `overlay`, `sample_rate`, `min`/`max` (omit a side to auto-fit it; the string `"auto"` releases it live), `ruler` (`samples`/`time`/`off`), `ruler_y` (`off` to hide), and for `view: "spectrum"`: `fft_size`, `db_floor`/`db_ceil`, `freq_scale` (`log`/`linear`/`mel`/`bark`) |
| `timeruler` | A **free-standing time ruler**: the shared axis as a strip the *document* places — a DAW's ruler above its tracks. A lane's own `ruler` is reserved out of that lane's height, so ruling a stack meant picking one lane to carry it and to pay for it; this owns its box instead. Joins the group named by `link` — or, with none, the window's own lanes — and labels its window; its ticks are indented by the **group's** gutter (the widest any member asks for) so they stand over the samples they name. A press **locates**, Shift+drag pans, the wheel zooms | `ruler` (the unit), `sample_rate`, `tempo`/`beat_at`/`quant`, `link`, `h` (its thickness), `theme` |
| `track` | A multitrack **lane**, holding `clip` children on the window's shared time axis. Its **header** (the band left of the axis) carries the name and, when asked for, the lane's controls | `label`, `height`, `snap`, `header_w`, `mute`, `solo`, `level`, `ruler`, `tempo`, `sample_rate`, `playhead_at`, `playhead`, `playhead_loop_*`, `link`, `theme` |
| `clip` | A placed rectangle spanning `[offset, offset + dur]` — the graphic unit — and a **window onto a segment of its material**: `start` is the source frame its own time zero reads and one timeline sample is one source frame, so trimming it hides frames rather than compressing them and opening the window again brings them back. `loop` wraps that window (past the last frame the material begins again; before the first comes its own tail), which is what lets an edge be pulled past the material at all; `fit` draws the material scaled into the span instead — the picture a time stretch would make, and nothing here makes one yet. **Its contents are layers**, drawn back to front: a take, a piano-roll of events, an automation curve over them. **One layer is edited at a time** (`layer`), and it is the only one that acts or offers an affordance — a press picks the layer whose own material is under it (a break-point, a note) and the clip's background belongs to no layer's material, which is what leaves it, and the **grips** with it, to the clip itself. So a clip whose curve is being edited shows no grips, and the pixels that light up are the pixels that act. `hidden` names the layers that are **not drawn**; what is hidden is not edited either. The grip is the placement layer's affordance: `grip_w` wide, a translucent plate with an arrow, drawn **while the pointer is on that strip** on the **topmost** clip under it, and only where that end is on screen — a clip scrolled half out of the window is cut by the window, and an affordance at the pixel the cut landed on would claim the clip ends there. While a trim is in flight the held edge stays lit wherever the pointer got to (a clip moves in `snap` steps and the pointer does not); a clip being *moved* holds no edge and lights none. A clip too narrow to hold two strips keeps **one** — its end, the edge that lengthens it (its start when the end is the one off screen), as wide as the clip and no wider — so a short clip is never left with no affordance at all. **A trim never drags a clip out of existence**: an edge stops **one sample** short of the other whatever the lane's `snap` is (the grid says where an edge lands, not how short a clip may be), it stops where the material does unless the clip loops, and a clip whose span falls under a hairline on screen is still drawn as that hairline — a **line** marking where it is. The line is deliberately not widened to something grabbable: a floor wide enough to aim at would freeze the clip's apparent length, so it would stop narrowing as the reader zooms out and stop widening as they zoom in, and length is the one thing a timeline exists to show. Zooming *in* is what brings a collapsed clip back to a width the hand can take — with its expand grip on the line. The take is drawn in the presentation `view` names — the trace (the default) or the time-frequency `"spectrogram"`, the same signal seen the other way, ending where the clip ends. Two edit verbs of its own, both **requests** the owner answers (the host holds no composition): `e` **splits** it at the time cursor — or at the pointer when no cursor is inside it — into two windows over one source, and `j` **joins** it with the run of clips that touch it on its lane | Its bodies say whether a hand may edit them: **`editable`** (default true) is a statement about the clip, so it reaches every body it carries. Set it false where a body draws a *rendering* rather than the thing itself — the notes of a pattern, a curve this editor cannot write. Such a layer is then **never selected by pointing at it**: the press falls through to the clip, which moves and resizes as it always did, because *where* material sits is the composition's and *what* it holds is the material's. A layer a script activates anyway (`/gui_set layer`) still refuses the edit itself, visibly (`"refused" "notes"|"points" <reason>`) and consuming the press. Live via `/gui_set` | `offset`, `dur`, `start`, `loop`, `fit`, `layer`, `hidden`, the take (`buffer`/`path`/`cache`/`data`/`blob`) and its `view` (+ `window_size`, `hop`, `db_floor`, `db_ceil`, `freq_scale`, `colormap` for the spectral one), `notes`, `points` (+ `points_min`/`points_max`, the curve's own value axis), `min`, `max`, `editable`, `label` |
| `patch` | A **directed, typed patcher**, drawing both levels: boxes with **inlets on top, outlets on the bottom**, a **cord** per `outlet -> inlet` connection, **coloured by rate** — contrasting primaries at one width — audio (`ar`) red, control (`kr`) blue, init (`ir`) yellow and dashed — colour carries the rate. At **level 1** (a `GraphDef`) a cord *is* a server bus (not drawn — the client names it); at **level 2** (a `SynthDef`/`FaustDef`) a cord is an internal UGen wire. A **canvas**: a box with `x`/`y` places freely; a box **without** `x`/`y` takes its slot in the host's **layered (Sugiyama-style) auto-layout** (ranked by longest path to a sink, so inputs sit above their use and sinks at the bottom). Boxes drag (`"move"` flows back), a click or a marquee on empty canvas selects, and inside a `scroll` workspace the whole patch pans and zooms; the labelled panel frames whatever boxes it holds | `boxes` (each `{def, inlets, outlets[, x, y, role]}`; a port is a bare name (audio) or `{name, rate}` with `rate` `"control"`/`"init"`; `role` `"source"`/`"const"` only tags a box for drawing — a `const` value box gets a distinct fill — the layout ranks every box by its cords), `cords` (a flat `[from_box, outlet, to_box, inlet, ...]` list, indices within each box's inlet/outlet lists), `label` |
| `score` | An **engraved music-notation page**. The client engraves a score and sends a *display list* — a glyph-outline table keyed by SMuFL codepoint plus placed glyphs, staff lines, stems, beams, slurs and text in page units — which the host fits into the widget and tessellates into the same triangle mesh as the rest of the chrome. Every primitive carries the MEI `xml:id` it was engraved from, so a click names an element (`"element"`), a drag transposes it (`"transpose"`, naming the staff position reached) and the page shows a playback cursor over its own timemap | `vb` (the `[width, height]` page-unit viewBox), `glyphs` (hex SMuFL codepoint → outline path `d`), `prims` (the placed primitives, each with its `id`), `cursors` (the cursor track: `t` in ms → `x`, `y0`, `y1`), `step` (page units per diatonic step), `display_list` (the whole drawing replaced live, as a JSON string), `playhead`, `playhead_at`, `playhead_loop_*` (ms), `sample_rate`, `selected`, `editable` (opt into pitch editing; off = a read-only view) |
| `canvas` | A script-supplied WGSL shader over the widget area | `shader`, `params`, `buses` |

**A data view names a bus and a rate.** Every live view — `meter`, `scope`,
`phasescope`, `spectrum` — reads from **`bus`** (default `0`, the first
hardware output) at **`rate`** (`"audio"`, the default, or `"control"`), over
`channels` **adjacent** buses where it takes several. A bus is a bus: the rate
says how its values are obtained, not what kind of thing it is. Nothing on the
wire names a recording ring — when a view needs an audio bus's samples, the
**host** asks the audio server to record it (`/bus_tap`, see
[`schemas.md`](schemas.md)) and stops when no open view draws it, and the
server publishes in its segment where those samples landed. A `meter` needs no
recording at all: it reads the per-bus level the engine publishes every block.

**Logical pixels, and the one place that is not.** Every length the wire declares — the place props `w`/`h`/`x`/`y`, a container's `margin`/`gap`, a `window`'s `w`/`h` — is a **logical** pixel: the host multiplies it by the display's scale, so a `h: 28` strip is a 28-pixel-looking strip on an ordinary monitor and a 56-physical-pixel one on a doubled HiDPI screen, and a script never asks what it is running on. `text_size` is logical the same way (it is a glyph scale, so it scales with the rest instead of the font staying tiny). The scale is one number per window, taken from the system natively and from `devicePixelRatio` in a browser, and the sizing table resolves against it **once per change** — never per frame.

The exception is a `plane` workspace's **content plane**: its `content_w`/`content_h`, its `view_x`/`view_y` pan and its children's place props are content units, and what turns them into pixels is `view_zoom` — physical pixels per content unit, because the plane's pan and zoom are written in the pixels the pointer moves. What the display scale does there is set the **default zoom**: absent a `view_zoom`, a plane starts at the window's scale, so one content unit is one *logical* pixel and a patcher's boxes come up the size they are meant to look. Name a `view_zoom` (in the tree, or by turning the wheel) and it is literal from then on; **`/gui_set view_zoom 0`** (or any non-number) clears it and hands the plane back to its default, which is the only way to ask for a default that has no number of its own — the same shape an empty `theme` uses to drop an overlay. Same split in the heavy views (`waveform`, `spectrogram`), which resolve their signal against physical pixels. So: chrome is logical, a navigable plane is its own — and its zoom is where the two meet.

**Why the default is the density and not a fit to the content.** A plane's content unit is a *display* unit: a patcher box is 96 units wide because that is how wide a box should look. Fitting the zoom to the content instead would make a box's apparent size follow **how many boxes there are** — a three-box graph huge, a fifty-box graph unreadable — and re-zoom the plane on every edit. Zoom-to-fit belongs to a key, not to the default. The distinction generalizes: a view whose content unit is *data* (a `waveform`'s sample, a `pianoroll`'s semitone, a `score`'s staff step) keeps its own window and the display scale must not touch it — a denser screen means more detail over the same span, not a different span.

**Theme groups and the `color` prop.** The host draws every chrome color from one theme — a table of named roles, loaded from `[gui.theme]` or `--theme` (see the configuration chapter) — and the wire customizes it with the same partial table, recursively. A **container** (`window`, `layout`, `plane`, `field`) may carry a `theme` prop: a JSON object of `"role": "#rrggbb[aa]"` entries overlaying its parent's theme for its whole subtree — a **theme group**, a style scoped to the function of a set of widgets (the transport bar dimmed, the recording strip warm) rather than to any individual part. Groups nest: an inner table overlays the *inherited* one. The leaf case is the single `color` prop on **any** widget — one hex that re-seeds just the roles carrying that widget's function: the accent family (a slider's handle and fill, a button face, a meter's bar), the trace, the first color of the multichannel series cycle, a clip's body. Both are live via `/gui_set` (`theme` rides as its JSON string; an empty value clears), and a `theme` on a GuiDef root persists with the named def, so a standalone bundle ships its look with zero host configuration. Overlays resolve when a def arrives or a set changes them — each widget ends up holding one resolved theme — so the per-frame path pays nothing; there are no selectors and no per-part rules, deliberately.

**The `opacity` and `radius` props: the two paint capabilities.** Both are declared on **any** widget, both are live via `/gui_set`, and a negative number clears either back to the default — there is no number in either range that means "say nothing", which is the escape an empty `color` is. `opacity` (`0`–`1`) is a **group's** property like a theme group: it multiplies down the whole subtree, so a control at `0.5` inside a panel at `0.5` draws at `0.25`, and it resolves at the mutation point exactly where a theme does. `radius` is the corner radius of the boxes a widget draws, in logical pixels, and it applies to that widget alone — a rounded panel says nothing about the controls in it; each box clamps it to half its shorter side, so a widget's own frame and focus ring round while the hairlines inside it (a divider, a tick, a track edge) keep their shape.

What each one is bounded by is worth knowing before it surprises you. The fade is **per-primitive alpha**, not layer compositing: two overlapping shapes inside a faded widget show through each other, because there is no second target to compose them on. And it fades the flat drawing — chrome, controls, text; a heavy view's picture (a `waveform` trace, a `spectrogram` texture, a `canvas` shader) is drawn by its own pipeline and keeps its own opacity. The radius changes the drawing and not the box: a widget's rectangle is what it is laid out and hit-tested against, corners included.

**Antialiasing is the host's, not the widget's.** Smoothing every edge in a window is one setting of the *host* — `--msaa <n>` / `[gui] msaa` natively, `GuiBridge.msaa(n)` in the browser — because it is the render pass's attachment that is multisampled, not any one widget: it costs one attachment per window and nothing per widget, and a sample count the GPU does not offer for the surface format falls back to `1` with a warning. `1` (the default) is the flat picture the host has always drawn, and is what a signal trace wants.

**The `gestures` table: what a drag on a container does.** Panning, sweeping a
selection and locating the transport are the **container's** gestures, not the
element's: they belong to the coordinate system a container gives its contents,
which is why Shift+drag pans the same way over every `field` — a lane, a clip,
a bare ruler — and over a navigable `signal`, and why a plain drag on a
`plane`'s background pans it. Any container may carry a `gestures` prop — an object keyed by
modifier (`drag` for the plain drag, `shift`, `ctrl`, `alt`), each value a
**plan**: the step names in order, separated by spaces.

| Step | What it does |
| --- | --- |
| `element` | Hands the press to whatever is under the cursor — the widget the pointer found, or the clip, note or box the container drew there. It may decline (empty space), and the plan goes on |
| `pan` | Pans the container's axis: time on a `field`, the plane on a `plane` |
| `select` | Sweeps the container's **shared time selection** on a timeline — a span, whatever the view under it measures. A selection that belongs to *one widget* — a patcher's box marquee, a `notes` element picking the notes inside its rectangle — is not this step: that widget claims the press under `element` and sweeps it itself |
| `select_box` | The same sweep **restricted on the second axis**: a rectangle over a view that measures a value, reported as the two further arguments of `"selection"`. It **declines** where the picture has one measured axis, so `"select_box select"` is the plan for a mixed stack — a rectangle where there is one to draw, the plain span where there is not |
| `sample` | Grabs the **sample** under the pointer on a navigable trace and drags it vertically — the smallest destructive edit. It **declines where a sample is not a thing on screen**: below the zoom at which the trace marks each sample with a disc there is nothing to grab, and the plan falls through to its next step, so `"sample select"` edits where the samples are visible and sweeps where they are not. One intent leaves, on release, as `"sample"` |
| `draw` | **Draws** over the material: a press-drag writes the value under the pointer for every sample it passes — the ones *between* two motion events included, by interpolation, or a fast stroke would leave the material combed with holes — and emits one `"draw"` on release. **Refused where a pixel is more than one sample**, visibly (`"refused" "draw" <reason>`) and consuming the press, so a plan naming a sweep behind it cannot turn a refused stroke into a selection |
| `locate` | Puts the transport's cursor under the pointer and emits `"locate"` |
| `none` | Nothing |

A container may also name **no** step for a modifier, which is not the same as `none`: an empty plan declines and the press walks outward, while `none` consumes it. That is a `clip`'s own table — the plain drag grabs it, every other chord falls through to the lane — because a clip is a container of its *local* axis, so a pan there would mean the wrong window.

The order is the point. `"element locate"` is a lane — grab the clip under the
cursor, and if there is none, locate; `"select"` is a waveform, which has
nothing on its axis to grab; `"select_box select"` is a stack of heavy views,
where the same chord draws a rectangle on the pictures that have two measured
axes and a plain span on the ones that do not. A plan that consumes nothing falls **outward** to
the container around it, which is how Shift+drag on a patcher's empty canvas
pans the workspace the patcher sits in. The defaults are the behavior described
throughout this chapter (`{"drag": "element locate", "shift": "pan"}` on a
lane, `{"drag": "select", "shift": "pan"}` on the heavy views, and so on), so
a table names only what it changes:

```json
{"id": 7, "type": "signal", "view": "trace", "gestures": {"drag": "pan", "shift": "select"}}
```

Live via `/gui_set gestures` (as a JSON string, the `theme` convention), and
each set starts again from the kind's defaults — so the modifiers it does not
name keep them. **Off the lanes.** A multitrack has pixels that are not a lane — the gap between two of them, the slack under the last one, a container's margin — and in a window with **exactly one** navigation group those are that axis with nothing drawn on them. So the axis' own gestures work there: the wheel zooms it, Ctrl+wheel resizes its lanes, Shift+drag pans it. A surface under the pointer that *can* act still wins (a `plane` with somewhere to scroll scrolls); one that cannot passes the gesture on instead of eating it. With two groups in a window there is no such answer, so there is no fallback.

**The wheel needs the pixels to be empty; the drag does not.** The two gestures read the same one axis and mean it differently. The **wheel** falls through only over a container's own surface, a label or a lane's empty space — over an element that draws a picture of its own and simply has no wheel (a goniometer, a meter, a knob) it does nothing, because the reader pointed at that element. **Shift+drag** pans the axis from *anywhere*, over any element: that is the gesture's documented reach, not a fall-through.

Two gestures are **not** in the table, because they are not
ambiguous: a press on a view's vertical strip (a `ruler_y`, a `pianoroll`'s
keyboard gutter) always pans that axis, and the wheel always zooms — the axis
under it, except **Ctrl+wheel over a lane**, which resizes the lane (its `h`)
and emits `"height"`. Time and thickness are different things: a `plane`'s zoom
is uniform over both axes, so growing the lanes with it would stretch the time
axis out from under the ruler.

**The `menu` opens.** A press on a `menu` opens its **option list** over the window — the widget's field grown downward by one row per option, flipped above it near the bottom edge — with the chosen row marked and the row under the cursor highlighted. A press on a row picks that option (the `index`, emitted as the widget's value exactly as any control's change is, or forwarded when the menu is bound); a press anywhere else dismisses the list and picks nothing. An open list is **modal**: it is drawn over everything, including the heavy views, and it is hit-tested before the widget tree, so the press that dismisses it does nothing else. The list is the host's — no script round trip, so a persisted GuiDef's menus work with nobody attached.

**Text on the light widgets.** Every text-bearing light widget — `label`, `button`, `toggle`, `text`, `number`, `menu` and the control labels on `slider`/`knob` — takes a `text_size`: a glyph scale over the host's embedded bitmap font (default `2.0`, the size everything drew at before the prop existed; clamped to `[1, 16]`). The face is a 5-column cell with a 7-row body — the height a line reserves — plus the room a diacritic takes above it and a descender below, so it writes **both cases** and the **Latin-1** letters: a label, a track name or a file path in Spanish, French or German reads as written. A character it does not carry draws as a hollow box. Single-line text that overflows its rect clips with an ellipsis instead of bleeding into the neighbor. Text drawn **over a picture** rather than on chrome — a clip's name over its take, a roll's cursor read-out over its notes — sits on a **text plate**: a translucent, rounded ground (the `plate` color role, the `plate_radius` size role) that keeps the line readable wherever the material under it is dense, without hiding it. It is as wide as the line it grounds, so a truncated caption takes a truncated plate, and a box with no room for a glyph draws neither. `label` additionally takes `wrap` (word wrap on the measured width of the words, the lines past the label's bottom edge dropped) and `align` (`start`, the default left edge / `center` / `end`, applied per line). All of them are live via `/gui_set`. `text_size` lands on **half-steps** of the cell — a bitmap glyph is scaled by repeating its own pixels, and a scale that does not divide the cell evenly makes those pixels unequal — unless the host was built with a rasterizer (its optional `font-atlas` feature, `--font <path>` natively or a face the page pushes in), where the prop is continuous and text draws through a real typeface. That is the **only** place two builds of the host differ, and it changes nothing else a script can see: the sizing table never followed the typeface, so the same document lays out identically either way. Inside a `scroll` workspace everything scales with the view's zoom together — the text, the padding, a control's own parts — because a zoom is an enlargement: a box at zoom 2 is the same box twice the size, not a box with oversized text jammed into it.

**The keyboard, and where it points.** One widget at a time holds the **focus** — the host's, not a window's, since there is one keyboard — and it is the only widget keys reach. A press moves the focus onto a widget that reads a keyboard and off one that does not; **Tab** walks the window's focusable widgets in layout order and **Shift+Tab** back along them; and the focused widget is drawn with a ring in the theme's `focus` role. Every move **the user makes** is reported as `/gui_event <id> "focus" <1|0>` — both ends of it, so a script that mirrors the focus sees the one that lost it too. A script may point the keyboard itself with **`/gui_set <id> focus 1`** (`focus 0` gives it up), and that one is *not* echoed back, exactly as no other `/gui_set` is: the script already knows what it asked for. `focus` is the one key that is not a prop: it says where the keyboard is, so a `/gui_query` does not report it and a widget that reads no keyboard refuses it rather than swallowing it silently.

**Tab past the last widget leaves the tree**, deliberately: a GuiDef mounted in a web page sits *inside a document*, and a ring that wrapped would trap the keyboard in the canvas and make the page around it unreachable. So the ring runs out, the focus clears, and the browser's own tab order carries on. In a desktop window there is nothing outside to hand it to, so nothing is focused and the next Tab enters the ring again. Composition (IME) and the system clipboard stay the **page's**: a canvas cannot host an input method, so the host reads the keys the page forwards it and nothing more.

**The editable `text` field.** A `text` widget is an editable entry, and the one widget that reads a keyboard today: click or Tab to focus it, click to place the caret, type to insert, move the caret with the arrows / `Home` / `End` (word-wise with `Ctrl`), edit with `Backspace`/`Delete` (a whole word with `Ctrl`, one run per press: in `a, b` the first removes `b` and the second the `", "` before it), select by dragging or with `Shift`+arrows (`Ctrl+A` selects all), and cut/copy/paste with `Ctrl+X`/`C`/`V`. The entered string is delivered **the same way a slider's value is — on every edit, never gated on Enter**: an unbound field emits `/gui_event <id> <string>` per keystroke, a bound one (`/gui_bind`) forwards the string straight to the audio server. `multiline: true` allows embedded newlines (`Enter` inserts one; a single-line field ignores it) and a field that grows to its rect; `value` seeds the contents and `/gui_set value` sets them live. The caret and selection are view state, not wire state, so redefining `value` never carries them.

The `plane` container is **one container with one gesture path**, and the familiar constrained scroll views are configurations of it rather than separate types: `axis: "y"` with `zoom: 0` *is* a plain vertical scroll view, `axis: "x"` a horizontal strip, and the default is the full 2D plane — drag the empty background to pan both axes, wheel to zoom anchored at the cursor. Its children's place props (`x`/`y`/`w`/`h`) are read in **content units** — physical pixels on the plane, not the logical ones the chrome declares: the content area sizes itself from their extents unless `content_w`/`content_h` name it. `view_x`/`view_y` are the content coordinates at the widget's top-left corner and `view_zoom` is physical pixels per content unit (absent — or set to `0` — the window's display scale, see the units section above); all three are live via `/gui_set` and travel back as the `"view"` payload when a gesture moves them. A widget scrolled outside its container is clipped away — it is neither drawn nor hit.

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

**A note edited inside a `clip` stops at the clip's edge; on the roll's own view
nothing stops it.** The two placements bound differently because they *show*
differently. The roll's own view spans its own content: drag a note rightwards
and the roll simply reaches further, so the note is one scroll away and nothing
is lost. A roll drawn as a clip's body is clipped to the clip's rectangle, so the
same drag would leave the note out of every pixel the clip owns — still in the
list, still sounding, and findable only by resizing the clip by hand. So inside a
clip a note is clamped **whole** into `[0, dur]`: its tail parks on the far edge
rather than its onset, since the part that would vanish is exactly the part being
dragged. The same edge holds a resize and an `osc` marker, and a block move
clamps as one (the block's last tail stops, the spread intact). What the clamp
does *not* do is grow the clip: a clip's length is what its own edge says, and
content that lengthened its container would make one gesture — nudge a note —
also move the end of the piece. To take a note further, resize the clip first.
Shortening a clip over notes already inside it removes nothing: they stay in the
list, out of the clip's span, and come back if it is stretched again.

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
element up or down the staff emits `"transpose" <id> <position>` on release —
the **diatonic staff position the element reaches**, in whole steps from its
staff's top line, positive upward, counted through the page's own `step` (page
units per step, which the engraver sends because it depends on its unit size,
not on the staff scale).

**A position, not the displacement**, and that is the rule every edit-back
follows: an absolute edit is idempotent, so a resend cannot move the note twice,
and one that arrives after the client has re-engraved the page lands where it
says rather than somewhere relative to a drawing that no longer exists. A
displacement would have to be rebased against the corrected state, and rebasing
is a replay the host has no model to perform. The host derives the position from
the engraving it was sent — the same reading it already does to place ledger
lines — so the client can derive the identical number from the display list it
sent, and the two cannot disagree about what a position means.

The client's editor may well move a note by *steps* (verovio's does); it
subtracts the note's current position to get them, against the engraving it
holds at that moment. The host owns no notation, so it cannot apply the edit; the driver does,
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
