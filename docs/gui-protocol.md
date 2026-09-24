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
declarative protocol, the GPU substrate, the editor views — is in
[Clients and language bindings](clients.md); its internals and the recipe for
adding a widget are in [Architecture](architecture.md).

## Commands

| Message | Meaning |
|---|---|
| `/gui_def id json [blob…]` | Build a whole widget tree in one message. `json` is the GuiDef document (below); trailing blobs carry bulk data a widget references by index. Re-sending an existing id **redefines** it (the old subtree is freed first), exactly as re-sending a `SynthDef` replaces it — and `id` names **any** widget, not only a window: a def of a widget inside an open window replaces that subtree **in place**, and everything outside it keeps what it had. That is what a structural edit is for. A widget that appeared has no other channel (there is no insert), and sending the *window* for one rebuilds every widget in it, so an edit in one lane would take the zoom, the scroll and the selection of every other. **A def over a tree the host is already drawing says what to look like, not what to destroy**: the two trees are walked side by side, a widget is matched to the widget it was (by `id` wherever it moved to, by position for the bodies the wire does not address, and only where the type agrees), and what the host itself put on one that survived is carried across — its window on the axis, its selection, which layer is active, what is hidden. The def still wins on every key it states, which is the wire's own rule: **nothing said is nothing written**, so a script that wants to move a view says `view_start` and one that does not leaves the reader's window where it is. The one thing a def must ask for rather than restate is **bulk**: `"data": "keep"` names the samples the host is already drawing (see the source props below). |
| `/gui_set id key value …` | Update one live widget's properties. Types are preserved (an OSC int stays an int). A value that is logically an array (a curve's break-points, a patch's wires) rides as its **JSON string**, since an OSC key/value is a scalar — with one exception: a **blob** value is bulk samples, the same raw little-endian `f32` a `/gui_def`'s trailing blobs carry, and it expands to exactly the array the inline `data` prop would have held. That is how a client past the inline ceiling changes what a live view draws: a native one can rewrite the file it spilled to and re-`reload`, a page has no file. A length that is not whole `f32`s is dropped rather than read short. |
| `/gui_free id` | Free a widget and its subtree. Freeing a `window`-rooted def closes its window. |
| `/gui_query id` | Ask for a widget's state. Replies `/gui_info id type key value …` — **what the widget is now**, which is the def's props with **every edit the user has made since** laid over them: a dragged control's value, a moved clip's `offset`/`dur`, a lane's mute/solo/level, a plane's `view_x`/`view_y`, an edited curve's `points`, a roll's `notes`, a score's `selected`. (A `/gui_set` needs no such correction — it is already the document.) The reply is flat OSC arguments, so it carries **scalars only**: a structural prop nothing edits (`theme`, `boxes`, `data`) is not reported, and asking for one means keeping the tree that was sent — but an **edited** structure is reported as the JSON **string** its own `/gui_set` already accepts (`points`, `notes`, `osc`), so what a query gives back is what a set would take. The `axes` pair is recorded **flat** (`ruler`, `view_start`, `min`, …) precisely so a query can answer it, while the node's `type` is kept as the tree wrote it. An **empty type** (`""`) means no such widget — the host answers either way, as the audio server replies even on a miss. |
| `/gui_bind id "server" address prefix…` | Forward this widget's value **straight to the audio server**, bypassing the script: on every change the host sends `address` with the fixed `prefix` arguments followed by the value (e.g. `"/node_set" 1001 "freq"` makes the widget send `/node_set 1001 freq <value>`). A bound widget stops emitting `/gui_event`. |
| `/gui_ack seq docVersion [source generation…] [reason]` | **Answer the edits this host emitted, up to `seq`.** The reply `/gui_event` never had, and the thing that lets a host draw an edit before it is confirmed without lying about it. There is no success flag: the values the owner decided ride as ordinary `/gui_set`s **in the same bundle**, and *applied*, *applied transformed* and *refused* are one message — a refusal is simply the previous value pushed back. Send it **always**, including when nothing changed. `seq` is monotonic, so one number retires every edit at or below it and a lost acknowledgement is harmless; `docVersion` is the document's version after applying; each `source generation` pair reports samples whose *content* changed while its identity stayed put (a destructive edit), which is the only thing that can tell a reader its copy is stale; `reason` is informational — nothing in the *mechanism* reads it, and the host's **status bar** does (see "The status bar"), which is the whole of what it is for: an edit that springs back with nothing said teaches that it sometimes does not work. |
| `/gui_bind id "widget" target prop` | Apply this widget's value to **another widget's property**, as a `/gui_set target prop <value>` would — a `menu` flipping a `stack`'s `index`, a slider driving a plot's `max`. A multi-value edit-back payload rides as the JSON string the prop already takes. A binding fires an **apply, never another binding**: the target's own binding does not fire from it, so two widgets bound to each other settle instead of cascading (stated, not detected — the chain is one hop by construction). |
| `/gui_bind id` | (no target) Remove the binding; the widget emits events again. |
| `/gui_load name` | Instantiate a **persisted** GuiDef by name (the host replays it as its saved `/gui_def`). Needs a data directory. |
| `/gui_font blob` | Draw text with this typeface from now on — a raw TrueType/OpenType file (no WOFF2). It carries **no id**: a face is a property of the host, not of a window, so every window it has open and every one it opens later draws with it. Loading one **relayouts nothing** (the size table never followed the typeface), which is what makes a late hand-over safe: the same tree comes up the same size, and only `text_size` stops being quantized to half-steps of the bitmap cell. A host built without a rasterizer, or handed bytes it cannot read, says so and keeps drawing with its embedded bitmap face — the floor every build draws on. The launch-time spelling is the host's own `--font <path>`. |
| `/gui_theme json` | Draw the chrome from these colors from now on — a partial `{"role": "#rrggbb[aa]"}` object, the same table a container's `theme` prop takes, scoped to the **host** instead of to a subtree. It carries **no id** for that reason: a look is a property of the host, as a typeface is. It is the base every theme group is resolved over, so every open window re-resolves its groups against the new table and redraws — a group overlays what it *inherits*, so changing the base changes what a group means. Unknown roles and unreadable colors are reported and skipped; a payload that is not a JSON object is ignored whole. The launch-time spelling is `--theme <file.toml>` / the `[gui.theme]` config table. |
| `/gui_headClock which [transport]` | Draw every playhead from this counter from now on — `"device"` or `"transport"`, followed on `transport` by which transport's position (an `int32`, 0 unless given, since a server has several). It carries **no id**, like the typeface and the theme, because it says what the numbers a window is handed *mean*, and one host reads one server. `device` (the default) is the engine's sample clock, which never stops: what a host watching a live server wants, since its meters, scopes and taps are all on that axis. `transport` is the transport's **position** — it holds while the transport is stopped, jumps wherever `/transport_locate` puts it and wraps at a loop's end, all inside the engine. Natively that number is a field of the shared segment; in a page the host polls `/transport_query` for it instead of `/clock_query`, on the same tick. **What it changes for a client**: a window drawing it needs no anchor of its own (`playhead_at` of `0`) and no message per frame, because seeking, looping and pausing are transport commands rather than a line the script keeps in step — which is what lets an editor hand playback to the server and stop computing time. A word the host does not know is reported and ignored, so a typo leaves the line drawing what it was drawing. The launch-time spelling is `--clock <device|transport>`, and `--session` implies `transport`. (The verb is `headClock` and not `clock` because both clients already have a `clock` — their application clock — and one word for two things is how a reader loses an afternoon.) |
| `/gui_metrics json` | Lay out with these sizes from now on — the theme's counterpart for lengths, a partial `{"role": number}` object over the metrics every widget reads its paddings, strips and hit slop from. The reserved `scale` key regenerates the whole set at a density rather than setting one role. Every canvas re-resolves the roles at its own scale and redraws. Same rules for what it does not understand, and the launch-time spelling is the `[gui.metrics]` config table. |

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
  starting at 1000, and a freed subtree returns its ids to the pool, so a long
  live session reuses ids instead of climbing. Hand-picked ids below 1000 never
  collide with assigned ones. A client's **editor** does not lease: it names each
  id after what the widget draws — the structure, its role in the picture, a key
  — so a widget still in the picture keeps its number across every redraw, which
  is what the host matches widgets by when it reconciles a def.
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

And on a **redraw** it names none of them. A def has to describe every widget in
the subtree it redraws, so a lane restated because one clip moved would otherwise
carry every other clip's audio with it — the same failure as freeing a zoom
because a neighbour moved, one order of magnitude up. `"data": "keep"` is the
word for it: the widget is described in full, its bulk is not, and the host
carries the run it is already drawing onto the widget that kept its identity.

It is an answer about a widget the host **has**, so it only means anything where
a widget survived the def — which is what a widget id naming what it draws is
for. A `keep` on a widget that is new, or on an id that now names a different
kind of widget, names bulk nobody holds: the host says so in its log and draws an
empty picture, rather than inventing one. On a `/gui_set` it means "no change",
so the same props map serves both doors.

## Events

The host pushes back to the script that built the window:

| Message | Meaning |
|---|---|
| `/gui_event id seq version <value>` | A control changed: a float (`slider`/`knob`/`number`), an int (`toggle` and `button` — their `on`/`off`, `1`/`0` by default; `menu` index), or a string (`text`). A switch whose pair is not whole numbers reports floats instead: the type follows the number, so the ints every reader already parses stay ints. |
| `/gui_event id seq version <tag> <flat values…>` | A view wrote data back. The tag names *what* was edited; the values are flat OSC primitives (never a new address — see below). |
| `/gui_event id seq version "press" \| "release" \| "click"` | **An interface event**: what the hand did, as against what the widget is worth. One tag and no values, which is what tells it from an edit-back. A `button` reports all three. Unlike a value, it is **never** swallowed or forwarded by `/gui_bind` — a command is not a control signal, so a bound button drives the audio server *and* tells the script it was clicked. |

**`seq` and `version` are the second and third arguments of every event**, before any tag, so one rule reads them all whatever the payload. `seq` is the host's stamp on the edit, and what an acknowledgement names. `version` is the document version the edit was made **against** — the last one an acknowledgement reported to this host — so an owner can tell an edit made against the picture it is looking at from one made against a picture that has since been replaced. Both are zero when the host has nothing to say: an unstamped event is one nobody will acknowledge, and an unstated version is one an owner applies unchecked. See *Answering an event* below.
| `/gui_closed id` | The window was closed by the user. |

**A gesture is one edit, and it leaves when the hand lets go.** A drag moves
what it is holding — a clip, a note, a break-point — and the host draws that as
it goes, because the picture must follow the hand. What it does *not* do is
report a value per frame: the edit-back is emitted **on the release**, whole, in
the owner's own units. Two things go wrong when it is emitted per frame, and
both were found by dragging one envelope: an undo history of a hundred entries
for one bend, and — since every event names the last version an acknowledgement
reported — a hundred round trips that a hand outruns, so every frame after the
first names a version the owner has already moved past and comes back refused.
The picture is then snapped to the answer of the first frame, over and over,
which is a curve trembling under the hand editing it.

What is reported *as it goes* is what is not an edit: a `"view"` pan, a
`"selection"` sweep, a `"layer"` change, and a bound control's value. Those are
state a script follows now, not a document change, and none of them is
versioned.

**A clip's placement arrives under one of three tags, and which one is the
gesture's choice rather than the clip's.** `"clip"` is one clip moved or trimmed
inside its lane; `"clips"` is that same drag once a **selection** exists, sent to
the lane the hand was on and naming every held clip by id; `"lane"` is that same
drag when the clip **crossed the stack**. They do not accompany each other --
each *replaces* the others for that gesture -- and the conditions are the hand's
state, not the widget's, so a reader that handles `"clip"` alone is correct until
the first marquee and silently wrong from then on. **Handle the three or none**:
the failure has no error and no missing pixel, because the host has already moved
the picture; what goes out of step is whatever the script was keeping in its own
head.

The **edit-back payloads**:

| Tag | Arguments | Sent by |
|---|---|---|
| `"points"` | `t v shape curve` per break-point | the `bpf` editor — one payload, whichever view drew the curve |
| `"notes"` | `start dur pitch velocity channel` per note | the `pianoroll` view — MIDI notes edited |
| `"markers"` | `time label color` per marker | a **ruler** — the labelled points on the time axis, after one was added or removed by hand. The whole list every time, as the lists above do, so the owner replaces what it holds rather than reconciling. The time and the text are what it keeps; the colour is `""` for the theme's |
| `"note"` | `pitch velocity state channel` (ints; state 1 = press, 0 = release) | the `piano` keyboard played — MIDI-shaped, translatable 1:1 to note-on/note-off |
| `"range"` | `min max` (MIDI notes) | the `piano`'s visible range panned or zoomed |
| `"layer"` | `name` (`"placement"`, `"take"`, `"notes"`, `"points"`, and `points:1` for a second layer of one role) | the **edit layer** a press selected on a container that layers its contents. Sent only when it *changed*, so pressing twice on the same curve says it once |
| `"height"` | `h` (logical pixels) | a lane resized with **Ctrl+wheel** — the host applies it to the lane under the cursor and says so, and a driver that wants every lane the same thickness echoes it onto the others. **The view's**: it says nothing about what the multitrack is, and no document carries it |
| `"element"` | `id` (a string: the MEI `xml:id`; empty = the selection was cleared) | a `score` page clicked — the engraved element under the cursor. **A sounding element wins over anything drawn across it** (the ids the page named in `elements`), and only among those does the tightest box decide: a staff line is a hairline the width of the system, so by area alone it takes every note written on a line rather than in a space |
| `"insert"` | `after position staff` (the MEI `xml:id` of the element the new note would **follow** on that staff, empty before everything on it; the whole diatonic staff position pressed; the staff, from the top, counted from zero) | a press on **blank paper** inside a staff of a `score` that took `entry` — an insertion *requested*, since the host holds no score. It names a **place and not a note**: a staff position is not a pitch until something knows the clef and the key, and a duration is a choice nobody made by clicking, so both stay the driver's |
| `"transpose"` | `id position` (the MEI `xml:id`; the whole diatonic staff position reached, from the staff's top line, positive = up) | a `score` element dragged up or down — a pitch edit *requested*, since the host holds no score. **Absolute**, so a resend is harmless and a re-engraved page needs no rebasing |
| `"focus"` | `1|0` (gained / lost) | the keyboard focus moved onto this widget or off it — a press or a Tab (a `/gui_set focus` is not echoed, like every other set). A **notification**, not a value: it is sent even from a bound widget, since a binding says where the widget's *value* goes |
| `"wire"` | `src_box outlet dst_box inlet` (ports by name; a rate mismatch is refused at the gesture) | a patcher cord drawn `outlet -> inlet` |
| `"move"` | `index x y` (box index; canvas units) | a patcher box dragged — one payload per moved box, so the driver owns the geometry |
| `"locate"` | `position` (timeline units) | **the axis clicked** — a lane's time ruler, empty lane space, a roll's grid, or the content drawn on any of them: a window has one cursor, it is placed by a click regardless of what is under the pointer, and the transport is being seeked there. The host has already moved its own cursor (and, while the transport runs, re-anchored the sweep so the line carries on from there), so this says what to play from and not where to draw |
| `"undo"` / `"redo"` | — | a window shortcut (Ctrl+Z, Ctrl+Shift+Z or Ctrl+Y). **Addressed to the window, not to a widget**: the id is the window's, the way `/gui_closed` names one. The host holds no history — the log lives with the document — so this is a *request*, and the owner answers with the state that now holds, exactly as it answers a drag |
| `"selection"` | `start len` (samples, always whole), plus `min max` where the sweep restricted the y axis too | a selection dragged on a timeline view |
| `"sample"` | `channel frame value previous` — the frame as an OSC **long** (a float runs out of integers at 16.7 million, six minutes of audio, and a sample index is exact or it is the wrong sample) | one sample dragged on a navigable trace, under the `sample` gesture step. **Absolute and carrying its own inverse**, so the owner can apply it and undo it without having remembered anything. The host draws the held value over the picture, marked, and lets go when the edit is acknowledged — so an owner acknowledges *after* pushing the samples that now holds, or the old value blinks back |
| `"draw"` | `channel start <values blob> <previous blob>` — the two runs as little-endian `f32` blobs, the bulk convention `/buffer_setRange` and the clipboard already follow | one **stroke** over a navigable trace, under the `draw` gesture step. **One intent per stroke**, not per sample: what the hand did on the way is the pending drawing's business, and the owner gets the run it ended with — plus what it replaced, so the edit is invertible |
| `"cut"` | `start len` (samples) | Ctrl+X over a selection. A cut is a copy and a removal: the **copy is the host's**, so the selected block goes on the clipboard first, exactly as Ctrl+C puts it there, and a view whose samples the host cannot read declines the cut out loud rather than removing what nothing holds. The removal is a **request**: the host owns no data, so the owner cuts and answers with what the document now is, and the length change a cut implies is the owner's to decide |
| `"paste"` | `position kind json` plus one **blob** per bulk payload | Ctrl+V. The clipboard travels *with* the request — it is the host's, so a block copied in one window pastes against an owner that never saw it. `kind` is the clipboard's (`text`/`elements`/`samples`/`spectral`), `json` the whole typed document, and the blobs are the payloads it names, interleaved little-endian `f32`. `position` is on the **timeline's** axis (where the selection starts), so an owner writing onto a clip converts it into the clip's own time |
| `"mix"` | the same as `"paste"` | Ctrl+Shift+V: a paste that **adds** the block onto what is under it rather than putting it in. Same payload, its own word, so an owner that has no mix declines it rather than inserting by mistake |
| `"refused"` | `verb reason` | the host could not do its own half — a copy whose source it cannot read (a mapped overview, a live view), a paste whose payload did not travel, a **stroke where the samples are not drawn one by one**. Said out loud, because a key or a pencil that silently does nothing teaches that it sometimes does not work |
| `"view"` | `start len` (samples), or `x y zoom` on a `plane` | the navigation window zoomed or panned — the timeline group's shared window, or a 2D workspace's plane |
| `"view_y"` | `start len` | the vertical display window zoomed or panned. `0, 1` is the whole axis; an **amplitude** axis may be opened **past** it — down to a start under 0 and a length over 1, up to four times the domain — because a floating-point signal can sit above full scale and a view clamped to ±1 draws the part that matters flat against its own edge. A **frequency** axis cannot: there is nothing above Nyquist to open onto |
| `"view_x"` | `start len` (0..1) | an element's **own** horizontal window zoomed or panned — a navigable `spectrum`'s frequency axis, which is in no navigation group (a group's shared window reports `"view"`) |

**A gesture that moves nothing says nothing.** An axis pressed against a bound — zoomed all the way out, panned to the end, or down at the resolution of what it measures — goes on receiving wheel steps and drag motion, and reports none of them: `"view"`, `"view_x"` and `"view_y"` are emitted when the window actually moved, never once per notch. A script counting events is counting movements.

Edited data flows as a **payload, never a new address**: the `/gui_*` family does
not grow per widget.

### The clipboard, and who may read what

Ctrl+C, Ctrl+X and Ctrl+V over a selection split exactly where the host's
authority does, and the split is worth stating because it is the same one
everything else in this chapter follows.

A **copy is a read**, so the host does it: it takes the selected span out of the
samples it has *mapped* and puts it on its own clipboard, typed, carrying the
rate it was taken at. A source the host cannot read — a mapped peak overview has
no samples behind it, a live view has no addressable past — **declines and says
so** (`"refused" "copy" <reason>`), because a block of silence on the clipboard
is the one answer worse than no.

A **cut, a paste and a mix change data the host does not own**, so they leave as
the `"cut"`, `"paste"` and `"mix"` events above and the owner answers with what the
document now is. A paste carries the clipboard **with** it rather than the
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
what sounds is the server reading the samples it already holds. A copy is for
carrying a block somewhere the samples are *not*.

The clipboard is one **typed document** — `text`, `elements`, `samples` or
`spectral` — and its bulk rides *beside* it as blobs rather than inside it as
base64, which is the same rule every other large payload here follows. A
`samples` block is never resampled in transit: resampling is an edit, and an
edit is something an owner performs and logs.

**A block of notes travels in the `text` kind**, holding the flat
`start dur pitch velocity channel` array a `/gui_set notes` takes — the host's
own vocabulary for a roll, so a block it copied is a block it can describe. That
is also what keeps it portable: a string is the one thing that crosses a system
clipboard on every platform. Pasted, it is written onto the addressed roll as an
ordinary `setmembers` — the very edit a drag on a note makes — so a paste is one
entry on the edit stack and one undo takes the whole block back. **The three
verbs are one mechanism**: what a copy puts on the clipboard is what a paste
places, whether the roll pasted it itself or the window asked the owner to. A
paste onto a view that holds no notes is refused with the reason, as is a
`samples` block, whose owner has to write it.

### Answering an event

An event is a **proposal**, not a fact. The host owns no data — a placement belongs to the arrangement, a sample to a source, a note to whoever holds the timeline — so what it emits is an edit for the owner to apply, and between the gesture and the answer there is a gap the host draws across. `/gui_ack` is what closes it.

The rule is one line on each side. The host stamps every event with a monotonic `seq` and keeps it *pending*. The owner applies what it can, pushes whatever state that left as ordinary `/gui_set`s, and ends the **same bundle** with `/gui_ack seq …`. The host then retires every pending edit at or below that stamp and adopts what arrived.

Three things follow, and they are the whole design:

- **There is no branch for a refusal.** *Applied verbatim*, *applied transformed* and *refused* are one message, because the value pushed is simply what the document now says — and a refusal is the previous value. An owner that snaps a placement to a musical grid, or declines an edit to samples a generator produced, says so by pushing what it actually has.
- **An unanswered edit is one the host waits on forever**, so the acknowledgement is sent even when nothing changed. Silence is not a refusal; it is a hang.
- **The stamp is what tells two gestures apart.** Without it an answer to one edit is indistinguishable from an answer to another on the same widget, which is exactly the case a host with an edit still in flight is in.

**The version answers a different question, and needs both directions.** `seq` says *which of my gestures is this an answer to*; `docVersion` says *are we talking about the same state*. That second one is what catches the document moving by a route that was never a gesture — a script editing the arrangement, a second editor, a re-render — which no record of the host's own edits can see. So the acknowledgement reports the version, the host remembers it, and the host names it back on its next event. An owner that finds an edit made against a superseded version **refuses it as stale and pushes the state that holds**, which needs no new path on either side: the host adopts it exactly as it adopts a snap, and the `reason` is what distinguishes *someone else changed this* from *not here*. Merging the two edits instead is deliberately not done — an edit-back payload is absolute *and* whole (a roll's `"notes"` is the list, not a diff), so applying a stale one would silently drop whatever arrived in between.

**Only a route the host never saw makes an edit stale.** The answers lag by construction — the host names the version it was last *told*, and it is told when an acknowledgement arrives — so an event naming a version the owner has already moved past is the ordinary case, not a collision: a drag reporting as it goes, a second gesture begun inside one round trip, a burst of events on any carrier slower than a hand. Those versions are the owner's own answers to this host, and they are applied. What refuses an edit is the document moving by a route no event produced: a script editing the arrangement, a second editor, a re-derivation, a history step. Both reference clients keep one *floor* — the version at which the last such change landed — and refuse an edit naming anything below it, and nothing else. Refusing on the lag instead is refusing the hand for being faster than a poll loop, and it answers each refused event with a snap back to where the gesture started.

**A def drops what that widget had in flight.** A `/gui_def` over a tree the host already draws is reconciled rather than rebuilt, but an edit still pending against what it names is forgotten all the same, exactly as `/gui_free` forgets one: the def states what the widget now is, so an acknowledgement for an edit made against the old one is never coming, and holding it would keep the outbox open forever. An owner is not expected to acknowledge those.

The acknowledgement is a **verb rather than a property** because it is scoped to the conversation and not to the tree: `seq` is per client, so two clients driving one window would collide on a single prop, and it does not round-trip, which a property here has to. It rides *after* the value pushes in the bundle, so the host never retires an edit before the state that edit produced has arrived.

### The status bar

A host says things nothing was listening for. It refuses a stroke where a pixel
is more than one sample; it refuses a press on a body that draws a rendering
rather than the thing itself; and an owner answers an edit with a `reason` the
mechanism deliberately does not read. All of it used to be said into a log file
nobody has open, so a refused edit sprang back in silence.

The **status bar** is where it lands instead: a band along the bottom of every
window, drawn by the host as chrome. It is not a widget, it is not in the tree,
and **no line of it crosses the wire** — the host already knows what it just did
and what it refused, so asking a client to send that back would be telling the
host something the host said first. What goes in it is every `/gui_event` the
host emits, at the one place an event is stamped, every value that leaves by a
**binding** instead — the one road that never passes through an event, so the
most direct control in a window used to be the one with no history — plus every
`reason` an acknowledgement carries. Consecutive lines from one widget with one
verb replace rather than stack, because a drag emits per motion and a log of
four hundred `"clip"` lines is a log of one.

Closed it is one line: the newest, in the warning colour when it reports a
refusal and quiet otherwise. **Clicking it opens it** into the window's log area
— the recent lines, newest at the bottom, up to half the window — and clicking
again closes it. Open, the **wheel scrolls back** through what it kept, a notch
per line, stopping at the newest line one way and at the oldest still on screen
the other; a line arriving while it is scrolled up leaves the reader where they
were, because a log that slides under the pointer is unreadable exactly while
something is happening in it. Closing returns it to the bottom. The band is
chrome, so the press and the wheel are consumed there and never reach the tree;
and the tree is laid out in the window *minus* the band, so a widget is hit on
the pixels it was drawn on.

A window that wants the pixels back says `status` false. It is **on by default**,
because a bar nobody turns on is a bar nobody hears from, and the refusals it
exists to show were already being said into nothing.

A **debug build of the host** shows a third kind of line there, in a colour of
its own: the host's notes about its own working — a key no element claimed, and
whatever else is instrumented — which are about the machine rather than about
the work. They are compiled out of a release build entirely, so nothing a client
sees or sends depends on them.


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
| `window` | 0 | a root; `title`, `w`, `h`, `flow`, `margin`, `gap`, `cols`, `hug`, `status`, `theme` | `window` |
| `layout` | 0 | children arranged by **`flow`** — `row`, `col`, `grid`, `free` or **`stack`** (one child at a time, the one at `index`) — plus `margin`, `gap`, `cols`, `hug`, `theme` | `panel`, `box`, `stack` |
| `plane` | 2, **locked to one scale** | a pannable, zoomable plane in content units: `axis`, `zoom`, `content_w`/`content_h`, `view_x`/`view_y`/`view_zoom`; with `boxes`/`cords`, the patcher | `scroll`, `patch` |
| `field` | 2, **independent** | the **free-standing time ruler** of a navigation group: an `axes` pair and nothing placed on it. It was three things told apart by what was on them — a lane, a clip, this — and the first two are `multitrack` props now: a lane cannot sit in a void, so it is always inside the view that owns it | `timeruler` |

`flow` is what the catalog spelled `layout`, on **every** container that has
an arrangement: the model spends the word `layout` on the container itself.

`stack` stops being a type because it never was one: a container showing one
child is a layout with a **selection** instead of an arrangement.

A `field` is the free-standing **ruler** of a navigation group, and nothing
else. It used to be told apart by what was on it — a placement made it a clip on
its parent's x axis, lane chrome made it a lane — and both of those are
`multitrack` props now: a lane cannot sit in a void, so it is always inside the
view that owns it, and a clip is a row of that view's `clips`.

**A clip is a view, and what it holds configures it.** Nothing on the wire spells
"audio clip" or "midi clip", and that is the model rather than an omission: the
clip's row states a placement, its contents state what is drawn in it, and the edits
a hand may perform — move, trim, `split`, `join` — belong to the *contents* under
it, not to a type. So a clip over samples and a clip over a timeline of notes take
the same edits, each in the unit of what it measures, and a client that admits one
and refuses the other is disagreeing with the wire, not implementing it. The
clip's `start` says the same thing for both: **the window** its own time zero
reads — a frame of the samples, a beat of the timeline — sent whenever there is
one to state, and reported back with the placement when a drag on an edge trims
it.

### The elements

| Type | Replaces | How the old name is said |
|---|---|---|
| `signal` | `waveform`, `spectrogram`, `plot`, `scope`, `spectrum`, `phasescope` | **`view`** (`trace` default / `spectrum` / `spectrogram` / `phase`) × the **source** (`bus` = forward-only; `data`/`blob`/`buffer`/`path`/`cache` = addressable, and `"data": "keep"` on a redraw = the run the host already holds) × the **capabilities** `navigable`, `selectable`, `editable` × the **layer stack** it draws (`layers`, also spelled `measure`: `peak` / `rms` / `signal` / `momentary` / `short` / `spectrogram`, back to front in one space-separated string or as an array whose entries carry `alpha`, `visible`, `solo` and `y`; the presentation's own by default, see above). `navigable: 0` over addressable samples is the static plot — the whole of it, since a view that does not navigate also resolves its source as the sequence itself rather than as a take, and auto-fits a value axis nobody named. Over a **bus** the missing piece is a past: `retention` (seconds, 0 = none) is the policy that supplies one, so `view: "spectrogram"` + `bus` + `retention` + `navigable` is a **waterfall** — the host keeps that many seconds, analyzes them into columns as they arrive, and the time axis navigates like a file's. It is a policy of the axis, not of the drawing: the same seconds mean the same seconds at any frame rate, `window_size` or `hop`, and a `/gui_set` of it resizes the history live. A live axis **follows the newest until you navigate it**, and then stays where you put it. `navigable` over a **spectrum** means something else, because that view's x is not time but **frequency**: an axis addressable with no retention at all (every bin is there every frame), navigated on a window the element carries alone — `view_start`/`view_len` (`axes.x.start`/`len`) in normalized display units over `[0, Nyquist]`, panned by dragging the axis, zoomed with the wheel under the cursor, reset with `R`, reported as `"view_x"`. It joins no navigation group: nothing else in a window measures in hertz along x. It is opt-in — a bare `spectrum` is the watching spectroscope — which is the one place `navigable` does not default to on. The zoom stops at the **resolution of the analysis**: below a few FFT bins across the whole body the curve is interpolation between two neighbours rather than a measurement, so the floor is derived from `fft_size` and the sample rate (and is therefore not a constant — a bin is a twentieth of a log axis at 500 Hz and a thousandth of it near Nyquist). The floor applies to what is **shown**, not to what is stored: `view_start`/`view_len` are the window that was asked for, from a gesture or from `/gui_set` alike, and the axis opens them wherever they are finer than it resolves. So a scripted window narrower than the bins is drawn — and reported — opened up, and a pan down the axis that has to open the window gives the asked-for one back on the way up rather than spending it |
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

A container that layers editable things — an audio editor's view, a box and the
envelope over it — draws several of them on **one rectangle**, so a press is claimed
by several at once: the placement's move, its edges, a roll's notes, a curve's
points. The **multitrack** is where it happens today: a box draws its contents
read-only and an envelope is drawn over them, editable, so a press inside one
rectangle is claimed by the box and by the curve at once.
The rule that decides between them is one sentence, and it is deliberately not a
list of precedences between kinds of thing:

> **One layer is active at a time, and it is the only one that acts or offers an
> affordance.**

Two props say it on the wire, and they are two questions rather than one:

- **`layer`** — which layer a hand is editing: `"placement"` (the container
  itself — where it sits, how long it is; `"clip"` is accepted as its name on a
  clip) or the role of one of its contents, `"take"`, `"notes"`, `"points"`, with
  `points:1` naming a second layer of the same role. Live via `/gui_set`, and
  moved by a press. **Where a layer has a name of its own it is named by it** —
  a multitrack's curves are the client's own words, so `layer` there is a curve's
  name and the ordinal is what a container whose layers are anonymous falls back
  to.
- **`hidden`** — which layers are **not drawn**, space-separated; empty draws
  them all. What is hidden is not edited either, so hiding the layer in hand
  hands it back to the placement.

**A press selects the layer it lands on**, and what "lands on" means is the
layer's *own samples* — a break-point, the line between two of them, a note —
never the rectangle it shares with the container. That is what leaves the
background, and the affordances drawn on it, to the container: dragging a clip's
empty space moves the clip and takes the hand off whatever was being edited
inside it. The active layer is asked first, so what is already in hand keeps the
pixels it draws on; the change is reported as the `"layer"` payload, once.

**A layer that cannot be edited is never selected by pointing at it** — the
press falls through to the container, which is why a clip whose notes are a
rendering (`notes_editable: false`, or the clip-wide `editable: false`) still
moves and resizes. Activating such a layer
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
| `x` | `autofit`, `unit` (`time`/`samples`/`beats`/`off`; `ruler` is accepted as its old name — like the `y` units, the ruler's **configuration**: `tempo`, `tempo_map`, `beat_at` and `quant` below are props of the ruler, set from the data where the data is in beats and a presentation choice where it is not, never a tempo the data holds), `dir` (`down`/`up` — which side of the ruler strip its marks sit on, see below), `start`, `len`, `tempo` (beats per second), `tempo_map` (the beat-to-second function as the JSON breakpoint list a `TempoMap` writes — `[{"beats": 0, "tempo": 1}, …]`, each entry optionally carrying a `curve`; where it is present it **wins over `tempo` and `beat_at`**, and it is what makes a beat ruler right where the tempo moves: a beat is a logical coordinate, so the ticks are placed by asking the map what second each one falls on rather than by dividing the window, and they crowd through an accelerando and spread through a ritardando over an axis that goes on measuring samples. Only the marks move — the clips do not: a multitrack is placed in seconds, so the map places nothing of it, and a structure in beats converts its placements through the same map before sending them. A map of one segment says exactly what `tempo` says, so send one only where the tempo changes), `beat_at`, `quant` (**beats per bar** — the grid a `bar:beat` label counts on, not a length in samples), `sample_rate`, `link`, `sel_start`, `sel_len`, `cursor`, `playhead`, `playhead_at`, `playhead_loop_start`, `playhead_loop_len` |

**`dir` puts a ruler's marks on the side its content is on.** A tick and the
pixels it names have to touch, and where the strip was placed is the only thing
that says which edge that is: a ruler **above** a stack of lanes draws its ticks
and numbers along its **bottom** (`dir: "down"`), one **below** draws them along
its **top** (`dir: "up"`). Put them on the far edge instead and the label stands
between the tick and the thing the tick points at, which is how a ruler comes to
be read one row off. A strip a view reserves under its own body is always `up`
and says nothing; a free-standing `timeruler` defaults to `down`, since a
document that places a ruler places it above what it rules. It moves the ticks,
the numbers and a marker's arrow together, so the whole row turns over at once.

**There are two cursors on a timeline, and only one of them is placed.**

- the **position cursor** (`cursor`, in samples; negative = none) — where a
  playback starts and where a paste lands. It is placed by a **click that
  landed on nothing**: the time ruler, the slack between boxes, a lane's empty
  tail, a grid nothing is drawn on. A click *on* something — a box, a note, a
  curve, a header control — is that thing's and moves no line, so the mark is
  never a side effect of pointing at what you meant to grab. Playing never
  moves it either, and it stays where it was put. A click that places it
  reports `/gui_event id "locate" position` — which says *the reader is here*,
  not *seek*; what that means for the sound is the owner's, and on a multitrack it
  is normally the position the next play starts from.
- the **playhead** (`playhead_at` swept, `playhead` parked) — the line that
  says where the **music** is. It is never placed: it starts from the position
  cursor and, while the transport runs, it is the engine's own position.

Both are the group's, so every lane of one multitrack shows the same two lines,
and
they are drawn in two roles (`cursor`, `playhead`) because they answer two
questions the eye must not have to read twice.

**`autofit`** is an x-axis property of its own, because what it governs is the window rather than a value: it says whether the view's window **follows its content**. `1` — the default, and what every view did before there was a switch — refits a window that was showing the whole timeline when the content changes, so a view that grows goes on showing all of it: right for a monitor, and for a roll being written into. `0` says the window is the **reader's**: the extent is still registered, so the axis knows how far it can go, and nothing moves it. That is what an *editor* wants, and the reason is that there a content change is mostly the reader's own edit — undoing a trim, splitting a clip, dragging one onto another lane — and an edit that re-frames the view is the window starting over under the hand that made it. It governs the window and never the extent, so nothing is lost by turning it off.

The content reaches the window by several doors, and the switch answers at all of them, because a rule stated once per door is a rule that comes back: an **extent registered** (a lane's clips, a take that loaded, a live axis sliding), a **clip's `offset` set** through `/gui_set`, a **redefine** rebuilding the view, a view **joining another group** through `link`, and the **page-forward** that keeps a take being written inside the window. With `autofit` off none of them moves the window — not to refit it, and not to re-clamp it either: a multitrack that got shorter must not pull a reader back off the empty bars they had deliberately scrolled onto. The clamp happens when the *hand* navigates, which is where it belongs.

**A navigation group is one window, so one member asking to be left alone leaves the whole axis alone**: a reader who pinned a view pinned the axis it shares. That is also what makes the property mean the same thing on each of the five views that carry it — a lane, a roll, a waveform, a spectrogram and a ruler navigate through one group model, so `autofit` is one behaviour and not one per widget.
| `y` | `unit` (`norm`/`db`/`bits`/`percent`/`hz`/`value`/`off`), `start`, `len`, `min`, `max`, `bit_depth`, `sel_min`, `sel_max` |

**`value` is the axis of an element that measures something of its own.** The
other units name what a *signal* is worth — an amplitude, a frequency — and say
so in that vocabulary. A break-point function's values are whatever parameter it
drives, so `value` labels the plain 1-2-5 ladder over the element's own
`min`/`max` and nothing else: a curve over `[30, 90]` beats per minute reads
`30 40 50 …`, the way a `plot` already rules a span it was handed.

**`y.unit` labels the axis; it does not map it.** The picture is linear in
amplitude whichever unit is named, and `db` is a ladder of rungs drawn at the
amplitudes those decibels are — not a logarithmic body. So the value a reading
names at a height and the value an edit writes at that height are one value, and
editing is in linear amplitude and only there.

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

**And the sweep is drawn the same way wherever it is drawn.** The band is one
routine in the host, told only what the view's second axis measures: a stripe
the full height where nothing restricts it (a lane of clips, a spectrogram), and
the rectangle the hand cut out of it where something does — a waveform's values,
a roll's semitones. The edges follow from that answer rather than from a switch:
a full-height band draws its two vertical edges, since its top and bottom are
the view's own, and a restricted one draws all four, because every one of them
is a value the hand chose. A patcher's canvas marquee is the same drawing with
both axes restricted, which is what makes one hand sweeping one rectangle look
like one thing wherever it sweeps.

**A multitrack's second axis is the stack of lanes**, not a value, so a marquee
down it takes the clips of every lane it crossed — the roll's own gesture one
level up, and the same call, since a semitone row and a lane are one
structure. A *span* over those lanes is the navigation group's — it is the loop
region, and every linked view draws it — and a lane draws it too, over its whole
height, whenever something set one: a `select` plan, a `/gui_set`, a linked
view's sweep. What a lane's **own** plain drag sweeps is not that at all: it is
the marquee, whose picture is the rectangle while the hand holds it and the
selected clips afterwards. **The two are different selections of different
things**, and neither is the other's picture.

**And a lane's span does not stop at its last clip.** A view of a signal clamps
a selection to its samples, because after the last one there is nothing to
select — neither played nor cut. A lane holds no data of its own: its extent is
only where its clips happen to end, and the empty bars after them are ordinary
time, a span to paste into or to loop over while writing. So a sweep across
them keeps the span the hand drew.

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

**`layers` is the stack the picture is**, back to front, and it is a factor of
the signal element rather than a composition of widgets. A layer is named by
what it draws: `peak` (the min/max envelope above), `rms` (the symmetric body
about zero at the level the signal held, drawn in the `trace_body` colour role),
`signal` (the band-limited reconstruction between the samples, below),
`momentary` and `short` (the **loudness** curves, below), and `spectrogram` (the
time-frequency texture). `measure` is the **same prop under its older name** —
one field behind two words, as `fft_size`/`window_size` already are — and it is
what a stack of nothing but measures has always been called.

The default is the presentation's own: **`"peak signal"`** for a trace and
**`"spectrogram"`** for the time-frequency view. Both are live on `/gui_set`,
because a picture is read by turning its layers on and off, and `/gui_query`
answers `layers` with the form it was given.

- **Two shapes, one prop.** A space-separated string of names is the stack at
  its defaults, in order (`"peak rms"` draws the level *over* the envelope,
  `"rms peak"` under it — the order is the author's, not the type's). An array
  says the same and lets a layer state its own: `{"draw": "rms", "alpha": 0.5,
  "y": "box", "visible": false, "solo": true}`, where a bare string entry is a
  layer at its defaults. A `/gui_set` carries the array as a JSON string, the
  way every structure on this wire does.
- **`alpha`** is the layer's own weight over the layers under it, `[0, 1]`,
  multiplying the ink its role is drawn in. It is not the node's `opacity`,
  which fades a whole subtree, chrome included: what a translucent stack is for
  is reading one picture *through* another.
- **`visible` and `solo` are how a stack is read while it is built.** A hidden
  layer keeps its place in the order, so showing it again puts it back where it
  was rather than on top; and where any layer solos, only the soloed ones are
  drawn — the mixer's own verb, one word instead of hiding the others one by
  one.
- **`y` is which vertical the layer is read on**, and it is the one thing a
  stack can get wrong. `"axis"` maps through the body's own vertical — the
  amplitude or frequency window a zoom opens — and reports it, so the y ruler
  and the cursor read-out are that layer's; `"box"` normalizes into the
  rectangle with a scale of its own and reports nothing. The default follows
  what a layer measures: amplitudes and the texture are on the axis, a loudness
  reading is in its box. **Two layers claiming the axis for two different
  quantities is refused** — the def fails to build and a `/gui_set` leaves the
  picture as it was — because whichever of them lost would be drawn on a scale
  that is not its own, which is a picture that lies. A spectrogram with a
  waveform over it therefore says `y: "box"` on the wave (or on the texture),
  and the ruler follows whichever kept the axis.
- **A texture under a curve is one element, and the curve is drawn after it.**
  The stack's order *is* the compositing order: the body's field is painted
  once, the texture is sampled where the stack puts it, and the layers after it
  draw over it — including their figures, which stand on the same text plate
  every caption over a picture already gets. A stack that names both draws both
  from the one source: the samples are summarized for the traces and analyzed
  for the texture, which is why such a view reads the **samples** (inline, a
  mapped file, a server buffer) rather than a summary. A source that is a
  summary and nothing else — a peaks cache, a streamed overview — has no
  spectrum to show, and its texture layer draws nothing rather than a picture of
  something else.

- **`signal` is what the waveform did between the samples, and it is a layer
  over them, never a replacement.** Where the samples are separate points it
  draws the reconstruction through them — the filter `clausters_core::resample`
  calls `FINE`, whose sub-sample at each sample is that sample exactly, so the
  curve passes through the dots — in the `trace_signal` colour role, and marks
  every peak between two samples that leaves full scale, with how far past it
  the loudest went written once. Where it draws, the `peak` layer keeps its dots
  and drops only the straight segment the curve stands in for. Zoomed out it
  draws nothing. The reason it layers rather than takes over: the samples'
  envelope sits under the signal's by a fixed amount that is the signal's own
  (a fifth of a decibel for an ordinary sawtooth, three for a tone at a quarter
  of the sample rate) at *every* zoom, so a picture that swapped one for the
  other would jump at the swap. It is the shape iZotope RX draws, the analog
  waveform over the digital samples in a colour apart. `"peak"` alone is the
  bare samples, dots joined by straight lines, as in any editor.

- **The classic editor picture is `"peak rms"`: one body, a drawing per
  layer.** The level is drawn inside the envelope by the same renderer placed
  twice, in the order the names were given — the envelope is the outer shape, so
  it is written first. It is one element and not two because **every view of a
  signal paints its own field before it draws**: two of them on one rectangle
  are not layers, the second hides the first. One element is also one axis, one
  ruler, one selection, one playhead and one upload of the samples.
- **A level is averaged over a fixed 50 ms of the source, not over the pixel
  column.** A root-mean-square is an average over a *duration*, so averaging
  whatever a column happens to cover would make the body's own values follow the
  **zoom** — moving over samples that did not change. The window is the
  signal's (50 ms, the RMS window an audio editor defaults to; at 48 kHz, 2400
  samples), and where a column is narrower than that the reading reaches out to
  it around the column's centre. So the body stands still while the view moves.
  A source whose rate the host does not know (a live bus window, already
  measured in milliseconds) averages each column's own span.
- **And it goes when the envelope has come down onto it — one weight, then
  gone.** The envelope *does* narrow with the zoom, since a column covers less
  of the wave; once it is within a fifth of the level there are no longer two
  readings, so the body is not drawn at all. That is what keeps it from ever
  poking out of the shape that contains it, and it is a **cut** at full weight
  rather than a fade, because a body drawn at a third of its weight reads as a
  quiet passage rather than as a distant one. Past the polyline threshold there
  is no envelope left to be a reading of, and the samples themselves are what
  remain.
- **`momentary` and `short` are the loudness, and they are the one measure
  drawn on an axis of its own.** Peak, level and reconstruction are amplitudes
  and share the picture's vertical; a loudness reading is in LUFS, so the layer
  brings its own scale, its own ruler and its own reference line — which is what
  makes it the concrete case the layer stack's rules are then generalized from.
  What is drawn is what a meter would have read at each point: the 400 ms window
  up to it (`momentary`) or the 3 s one (`short`), as ITU-R BS.1770 and EBU
  R 128 define them, in the `trace_loudness` colour role. Where a column covers
  many readings it is drawn as the band between the quietest and the loudest,
  for the reason the envelope is: sampling one reading out of the hundred a
  pixel spans would make the curve's own shape follow the zoom.

  The scale is **EBU Tech 3341's**, named by its top: `loudness_scale: 9` (the
  default, the document's own) runs from 18 LU under the target to 9 over it,
  `18` from 36 under to 18 over. `loudness_target` is the line the curve is read
  against and the zero of the scale, **-23 LUFS** by default, which is EBU
  R 128's programme loudness. `loudness_ruler` (on by default) draws the layer's
  own ruler on the **right** of the body, labelled in LU relative to the target
  — the left strip being the picture's own axis, and two axes on one strip being
  two numbers where a reader expects one.

  `loudness_stats` (on by default) writes **the numbers** over the picture: the
  gated integrated loudness, the loudness range (EBU Tech 3342), the true peak
  in dBTP and the peak-to-loudness ratio between the last two. They are measured
  over the **selection** where there is one — marked `SEL`, since a figure that
  does not say what it measured is a figure a reader will misread — and over the
  whole take where there is not, which is what an editor's statistics window
  does. An edit under the layer re-measures only the span it touched (widened by
  the filters' memory), so the curve and the numbers follow a stroke without a
  pass over the take.

  Over a **bus** the same measure is a live meter's curve: the host feeds a
  streaming loudness meter from the bus's retained history — the samples, not
  the oscilloscope's overlapping display windows — and draws the readings it has
  taken, at no extra API. It needs the source's **`sample_rate`** to mean
  anything (a 400 ms window is a count of samples), and a view that states none
  draws no curve rather than a curve of the wrong length.

- **A source that cannot measure draws no body.** A peak cache written before
  the format carried the mean square (CLPK v1/v2) has an envelope and no
  energy, and zeros would be a measurement — silence — over samples that is
  not silent.
- **`fills` says the samples are being written as they are drawn**, and it is a
  prop because the host cannot infer it. A take being recorded is samples up
  to the buffer's write frontier and *nothing* past it; a take read from a file
  that one `BufWr` dropped a sample into has a frontier too and is samples
  everywhere. One number, two pictures — so the client that allocated the empty
  buffer is what says which. Set, the view draws up to the frontier and leaves
  the axis past it **empty**, rather than inking the buffer's own zeros (which
  the minimum-ink rule would draw as a flat line across a stretch nothing has
  happened in yet). Live both ways: clear it when the take is finished and the
  whole of the buffer is drawn again.
- **And `fills` is also what makes a host follow the recording**, by whichever
  route it has. A host that **maps** the server's memory reads the write
  frontier out of the shared segment and re-summarizes the frames that
  appeared, needing nothing from the wire. One that does not — a page, or a
  native host on a server with no segment — holds its own copy of the samples
  and cannot: it **subscribes** for the views that asked (`/buffer_stream`) and
  folds the overview the server sends into the picture it holds. One
  subscription covers every such view of every window, so a script sharing the
  connection (a page, where the host and the script are one client) must not
  open its own beside it — the server keeps one per client and the second call
  replaces the first. What arrives is the summary and not the audio, so a take
  that is filling is drawn at the report's bucket wherever nothing finer has
  been read. **Past that it reads**: a view zoomed finer than its summary asks
  for the span it is showing and draws it — so the picture is the same at every
  zoom whichever way the host got it, which is the rule the platform seam is
  judged by. What it asks for has two shapes, and the zoom is what chooses:
  between the summary's bucket and about thirty-two samples a pixel it asks for
  a **finer grid** over that span (`/buffer_peaks` at a finer bucket), which is
  the one min/max pair per pixel column the drawing actually needs and a few
  kilobytes; finer than that it asks for the **samples** (`/buffer_getRange`),
  where a bucket would carry three floats to describe a handful of them and
  where the trace is the polyline through the samples anyway. Neither is on the
  wire as a decision: both are ordinary commands, and which one a host sends is
  its own business.

  This holds **while the take records too**, and the frontier is the whole of
  what `fills` does to it: a span is asked for only as far as `written`, never
  across it. Past the frontier there is nothing to read — the buffer holds the
  zeros it was allocated with, and a run over them would claim measured silence
  over audio that has not arrived — while behind it the frames are final, since
  a recorder writes forward and does not come back and the frontier is what the
  writer says it has already written. So a page zoomed to the sample during a
  recording sees the samples behind the frontier, which is what a host that maps
  the segment has always shown: there the samples *are* the mapped cells, so any
  zoom is current with nothing told to it.
- **What `fills` does *not* decide is how the samples arrive.** A host that
  cannot map them has two routes and keeps both, because they are cheap in
  opposite cases: a **short** buffer is downloaded whole (`/buffer_getRange`
  from end to end) and then every zoom is answered out of what is in hand, with
  no further round trip ever; a **long** one is drawn from its summary
  (`/buffer_peaks`, or the stream while it records) with the run under the eye
  read back as the eye moves, because 230 MB is not downloadable at any zoom.
  The line between them is the buffer's **size**, decided by the host when
  `/buffer_query.reply` first says what the shape is — roughly five seconds of
  stereo, which is as much a count of round trips as of bytes. It used to be `fills`, which meant two views of one finished take,
  one opened while it recorded and one after, took different routes and behaved
  differently under the same hand; the fork is real and worth keeping, the
  criterion was not. Nothing about this is on the wire: a script says what the
  view is *of*, never how to fetch it.

An `axes` pair works on `/gui_def` and on `/gui_set` alike (there it rides as
its JSON string, the `theme` convention). Everything the container does **not**
own stays where it is: an element's source (`buffer`, `path`, `cache`,
`bus`, `rate`, `channels`, `base_bucket`), its presentation's own parameters
(`fft_size`/`window_size`, `hop`, `db_floor`/`db_ceil`, `freq_scale`,
`colormap`), and every place prop (`w`, `h`, `weight`, `x`, `y`).

**`data` is the one source that is also live.** A `/gui_set data` replaces
**inline** samples, which is how an owner that has applied an edit pushes the
samples that now holds — and therefore how a pending edit can be let go of
without the edit disappearing with it. It is refused on a source that names a
file, a cache or a server buffer: those are re-read by resolving the resource
again, and pushing samples at one would leave the picture half from each.

**`reload` is the other half of that sentence, for a source that is mapped.**
`/gui_set reload 1` makes the element forget what it resolved, so the loader
reads its file, cache or server buffer again — the way an owner says *the
the samples are where they always were, and the window moved*. A source with nothing behind it
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
`multitrack`, `timeruler`, … — as **shortcuts** that build a model node with
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
| `window` | A top-level window (a GuiDef root). It carries the host's **status bar** along its bottom edge unless `status` is off — see the section below | `title`, `w`, `h`, `layout`, `margin`, `gap`, `cols`, `hug`, `status`, `theme` |
| `panel` | A nestable container | `layout`, `margin`, `gap`, `cols`, `hug`, `theme` |
| `stack` | A container showing **one child at a time**, the one at `index`: it fills the container, and the hidden pages are neither laid out nor drawn while keeping their place in the tree (so a heavy view keeps its GPU slot and its bus reads across a switch). An `index` outside the children shows nothing — a blank page, not a clamped one. Tabs, a pager and a waveform/spectrogram switch are this plus a control bound to `index` | `index`, `margin`, `hug`, `theme` |
| `scroll` | The **2D workspace**: a container whose children live in a virtual content area seen through a panning, zooming window. General first — the default is the free plane; the constrained scroll views degenerate from it by configuration | `axis` (`both`/`x`/`y`), `zoom` (0 disables the wheel zoom), `content_w`/`content_h`, `view_x`/`view_y`/`view_zoom`, plus `layout` (default `free` here), `margin`, `gap`, `cols`, `theme` |
| `label` | Static text | `text`, `text_size`, `wrap`, `align` (`start`/`center`/`end`) |
| `knob`, `slider`, `number` | Continuous controls | `min`, `max`, `curve`, `step`, `value`, `label`, `text_size` (`vertical` on a slider) |
| `button`, `toggle` | Momentary / latching | `label`, `mode` (`gate` default / `press`, `button` only), `on`, `off`, `value` (`toggle` only), `text_size` |
| `text`, `menu` | An editable string field, a choice | `value`, `multiline` / `options`, `index`, `text_size` |
| `meter` | **A bus level as a meter is read**: the column, the peak mark that waits, the clip lamp that stays and the decibel ladder beside them, over `channels` adjacent buses from `bus` — read from the server's shared segment, one load per channel per frame. The level an audio bus publishes is the **peak of every sample of the block**, held with a decay, so a reader running at a screen's rate sees the transient rather than whatever sample it looked at; at `control` rate the `Meter` UGen puts the same ballistics on any signal. **The scale is decibels** for an audio level (an amplitude is read in decibels or the top 20 dB take nine tenths of the column) and the widget's own `min`..`max` for a control bus carrying something that is not one; `scale` (`"db"`/`"linear"`) settles it either way. A decibel meter bottoms out at `floor_db`, or at the dynamic range of a resolution (`bits`: 16 is -96 dB, 24 is -144, and a 32-bit float takes 24, its significand), and defaults to the 60 dB strip a mix is read on. `ruler` (`"left"` — the default there — `"right"`, or off) is which side the numbers fall on; a meter with no room drops them itself, as it drops the ladder that would eat the column. **A meter is thin**: it asks for one narrow column per channel and its ladder's strip and stays elastic on the height, since a level is read by how far up it goes; `w` widens it like any other widget. The column is read by **where it changes colour** — green to the alignment level (-18 dBFS), amber from -12 and held, red from -6 to the top — so the bands are bands and only the edges between them are ramps. **The mark** is the held peak: the loudest reading, kept `hold` seconds and then falling `decay` decibels per second, drawn as a hairline because it is the same axis read at another moment; `hold` of 0 draws none. **The lamp** over each column latches an over and stays lit until it is **clicked** or the count it watches starts again — a new pass — because clipping is a handful of samples and a reader is not. The lamp takes a **share** of the meter's height rather than a fixed strip — a mark over a column is a proportion of the column, or a tall meter's lamp is a hairline nobody notices and a short one loses a quarter of its scale — and the number it writes grows with it. That number is how far **past full scale** the signal went, which is the question a lit lamp raises: the engine is floating point, so the signal is not lost, it has to come down by that much. It is written **once across the whole strip** and not per column (which channel clipped is worth a lamp of its own; by how much is a question about the signal, and a column is a few pixels wide), and `readout` drops **every number the meter writes** — that figure and the level at its foot. With `ruler` off as well it is a **bare column**, which is a meter and not a degraded one: it is what a strip of them down the edge of a track header is, where there is room for neither and the picture is the whole of what it has to say. What counts as an over is a **run** of samples at full scale, which only something walking samples can see: `clip` names the first of `channels` control buses written by the `ClipCount` UGen and the widget differences the count. With no such bus the lamp lights on the level alone reaching the top, which is exact for exceeding full scale and blind to how many samples did. A click reports **nothing**: the lamp is the reader's state and not the signal's. **`peak`** says what the level is a measurement of: `"sample"` (the default, the block's largest sample — what an audio bus publishes and the `Meter` UGen reads) or `"true"`, the peak of the reconstructed signal a `TruePeak` UGen writes, which is read against **-1 dBTP** rather than full scale. It takes effect with `rate: "control"`: at audio rate the level is the published sample peak, and calling it a true peak would read the lamp against a ceiling the number never meant, so an audio meter stays a sample meter whatever it is told. | `bus`, `rate` (`audio` default / `control`), `channels`, `scale`, `floor_db`, `bits`, `hold`, `decay`, `clip`, `ruler`, `readout`, `peak`, `min`, `max` |
| `scope` | An oscilloscope over `channels` adjacent buses from `bus` (trigger searched in the first channel; a lock/free read-out) | `bus`, `rate` (`audio` default / `control`), `channels`, `overlay`, `window_ms`, `trigger`, `hold`, `min`/`max`, `ruler` (ms) / `ruler_y` (value; `"off"` hides) |
| `phasescope` | A goniometer (stereo field) over the audio bus pair `bus` / `bus + 1` | `bus`, `window_ms`, `hold` |
| `spectrum` | A live spectroscope: one color-coded curve per channel over `channels` adjacent audio buses. With `navigable` its **frequency axis** zooms and pans (`view_start`/`view_len`, reported as `"view_x"`) | `bus`, `channels`, `fft_size`, `db_floor`/`db_ceil`, `freq_scale` (`log`/`linear`/`mel`/`bark`; `log_freq` is the legacy boolean alias), `averaging`, `peak_hold`, `navigable`, `view_start`/`view_len`, `ruler` (Hz) / `ruler_y` (dB; `"off"` hides) |
| `nodetree` | The server's node graph, live | `group`, `controls` |
| `waveform` | The editor-grade waveform: multichannel lanes, rulers, selection, playhead, linked navigation | the data (`data`/`blob`/`buffer`/`path`/`cache`, or `"keep"`), `channels`, `ruler`, `ruler_y`, `sel_*`, `playhead_at`, `playhead`, `playhead_loop_*`, `y_start`/`y_len`, `link`, `offset` |
| `spectrogram` | The editor-grade spectrogram, the same chrome. Over a live `bus` with a `retention` span it is the **waterfall**: the last N seconds, rolling | the data (or `bus` + `retention`), `window_size`, `hop`, `freq_scale` (`log_freq` is the legacy boolean alias), `db_floor`/`db_ceil`, `colormap` |
| `bpf` | A drawable break-point envelope, played by the server's own shape math. It is a **view** as well as a picture: given an axis pair it draws its own field with a time strip under it and a value strip left of it (`y.unit: "value"` is the ladder its values want), and it joins the navigation group `link` names — so a curve stacked with a `timeruler` shares that ruler's window and gutter, and the ruler rules the curve. Both strips default **off**, so a bare `bpf` is the bare envelope it has always been. Drawn as a **clip's body** instead it fills the rectangle the clip drew and rules nothing, which is what `points` on a `clip` has always meant | `points`, `min`, `max`, `duration`, `exp`, `editable`, and the axis pair: `ruler`, `ruler_y`, `sample_rate`, `tempo`/`tempo_map`/`beat_at`/`quant`, `link` |
| `pianoroll` | The editor-grade piano-roll: a keyboard, a MIDI-note grid, a velocity lane and a read-only **markers lane**; the same chrome and navigation as the heavy views. Its block keys are the clip's own, one level down: `q` quantizes, Delete removes, Ctrl+C/X/V move a block through the host-wide clipboard, and **`e` splits and `j` joins** — the same two letters a clip is cut and joined with, over notes, and taken **only over a selection**: a roll drawn as a clip's body shares the letters with the clip they belong to, so with nothing selected the key falls through and cuts the clip, which is what `e` has always meant there. The cut and the paste fall on the **window's cursor** (a key gesture has no pointer to read a position from, and the window has one cursor for exactly that; where the roll is on no axis, step entry's own position stands in), **snapped to `snap` like every other edit here** -- the cursor itself is never quantized (it is a time on the clock, and no content moves it or rounds it), so what the grid governs is the edit and not the position it starts from; `snap` 0 is no grid and the edit lands on the raw time, a joined run is what **touches on one pitch** (a pitch is what makes two notes one voice, the way a lane is what makes two clips joinable), and both leave in the ordinary `"notes"` payload: a roll holds its own notes and edits them, where a clip asks its owner to, because the owner holds the element. **A verb that acts on nothing says why** — `"refused" <verb> <why>`, the same shape a lane's verbs answer with and the same one an unwritable body refuses a press with — because a correct refusal nobody is told about is indistinguishable from a key that does not work | `notes` (`start dur pitch velocity channel` quintuples), `osc` (`time label` marker pairs — **read-only**: a roll edits what has a pitch, and the markers are the timeline's other items shown on the same axis, drawn from a lossy `(time, label)` view of a message whose address the lane has no way to type. A press meant to edit one is refused out loud (`"refused" "osc" <reason>`) and consumed; every other press there goes back to the container, so a sweep still crosses it), `min`/`max` (pitch window), `snap`, `velocity`, `osc_lane`, `midi_in` (live MIDI painting: the native host opens its virtual input port and paints incoming notes — at the running playhead, or step-entry on the `snap` grid), `ruler`, `sel_*`, `playhead_at`, `playhead`, `playhead_loop_*`, `y_start`/`y_len`, `link` |
| `piano` | The playable virtual keyboard, laid out with real piano proportions; its overview strip pans/zooms the visible MIDI range, and it plays server voices itself when `voice` is set (an `/synth_new` per key press, a `gate 0` per release) | `min`/`max` (visible range; min snaps to a white key), `active_min`/`active_max` (keys outside draw grayed and are inert), `pan` (0 freezes all range navigation), `overview`, `velocity` (fixed; unset = from the press height), `channel`, `voice`/`voice_args` (host-managed voices), `label` |
| `plot` | A static plot of a signal: multichannel lanes, x/y rulers, a hover readout, and **views** (`signal`, `spectrum`; the set is extensible) — measurement without navigation | `data`/`blob`/`path`, `channels`, `view`, `overlay`, `sample_rate`, `min`/`max` (omit a side to auto-fit it; the string `"auto"` releases it live), `ruler` (`samples`/`time`/`off`), `ruler_y` (`off` to hide), and for `view: "spectrum"`: `fft_size`, `db_floor`/`db_ceil`, `freq_scale` (`log`/`linear`/`mel`/`bark`) |
| `timeruler` | A **free-standing time ruler**: the shared axis as a strip the *document* places — a DAW's ruler above its tracks. A lane's own `ruler` is reserved out of that lane's height, so ruling a stack meant picking one lane to carry it and to pay for it; this owns its box instead. Joins the group named by `link` — or, with none, the window's own lanes — and labels its window; its ticks are indented by the **group's** gutter (the widest any member asks for) so they stand over the samples they name. **The ruler is where the time range is swept.** Two selections live at once over a stack of lanes or a roll — the **data** one (the clips, the boxes, the notes a rectangle covered: what gets edited) and the **time range** (a span the group keeps, drawn as a band, looped by the transport: what gets played) — and they are told apart by where the gesture began, not by a mode: the body sweeps the first, the ruler the second. So a **drag scrolls** the axis, **Alt+drag sweeps the range**, the wheel zooms, and a **click places the position cursor** — a drag that never left the slop is where the hand pointed. The slack of a stack places it too; what never does is a click *on* something, which is that thing's. On a signal the range is not a second thing: the frames and the span are one selection there, so the ruler is another hand onto the one the view already has. A lane's own `ruler`, a roll's and a signal's are **strips**, not widgets — the press lands on the view — so the rule is read from where it landed: the bottom `ruler_h` of a widget whose editor has a ruler on answers with **this table**, whoever drew it. Over the body of the same widget nothing changes, which is the data selection. **`markers`** are the labelled points on that axis — `time label color` triples, drawn as an **arrow into the ticks** and never a line down the picture (a playhead and a selection band are the two things that draw one): **Ctrl+click** adds one, numbered, or removes the one under the pointer, and a **click** on one locates at the time it was placed at rather than at the pixel. **The arrow is the target** (with the usual `hit_slop` around it), never the label: a name is text on the tick row, and aiming at a word would give a marker called `intro` ten times the reach of one called `2`. The edit-back is a flat `"markers"` event, and a `/gui_query` gives the list back as the JSON string a `/gui_set` takes | `markers`, `ruler` (the unit), `dir` (which side its content is on — `down` by default here, a ruler above the lanes), `sample_rate`, `tempo`/`tempo_map`/`beat_at`/`quant`, `link`, `h` (its thickness), `theme` |
| `multitrack` | **One widget holding a stack of lanes and the clips on them**, on one shared time axis — the `pianoroll`'s shape applied to a multitrack. A roll is one widget holding its notes; this is one widget holding its lanes and its clips, so a client **describes** the multitrack instead of composing a tree of `track` and `clip` widgets, and there is exactly one thing that owns it. A lane is a row of this widget and cannot sit in a void. `lanes` is the flat `name label height mute solo gain curves` septuple array (`curves` being whether this track's automation rows are shown — the header's toggle) and `clips` the flat `name lane offset dur start label source` one, with `offset`/`dur` in timeline samples and `start` in **the source's own frames** (the frame the box's zero reads, which is a question about the samples and not about the timeline), `lane` naming one of the lanes and `source` the **server buffer** the box is a window onto — a **negative** number is a box with no contents, because buffer 0 is a buffer and a zero sentinel would make the first take a script loads the one it cannot draw. The samples never cross this wire: the host maps them out of the shared segment or fetches them over its own leg, so two clips over one take are one download and one pyramid — the same carrier a roll's `notes` rides, and **the name is the identity**: the client's own word, never a widget id, so what is drawn and what is reported are addressed by what the script already calls them. A clip naming a lane that is not there is **kept and drawn nowhere**, so renaming a lane loses nothing. **A box's base view is what its contents are.** A box whose `source` names a buffer draws those samples, in the presentation the widget's `view` names (`"trace"`, the default, or `"spectrogram"` — one word for every box, because every box drawn the same way is the normal case); a box named in `notes` draws them as a **roll**, fitted to its own pitch range and with no keyboard, no strips and no chrome. Both are drawn by the very elements that stand on their own elsewhere, handed the box's own axis through the body door — a box is a **window onto a picture**, never a second implementation of one, which is what keeps a take in a multitrack and a take in a window from drifting. `notes` is the flat `box start dur pitch velocity channel` sextuples, each note naming the box it is in, the way a clip names its lane; a note naming a box that is not there is dropped, where a *clip* naming a missing lane is kept, because a clip is what a report is about and a note is what one holds. It **places, and never edits what a box holds**: a box's contents draw read-only here, because this widget owns *where* things are and not what is inside them. The multitrack edits non-destructively, so a double click on a box is a press like any other and opens nothing; editing what a box reads is another application's, opened on its own. **The curves are the light views, and they are editable.** A break-point automation is one element in two places, and the place is the whole difference: `curves` is the flat `name lane label min max height` sextuples — a **track automation**, which takes a **row of its own under the lane it names** and runs the whole timeline, because a track's gain does not begin and end with a box; `layers` is the flat `name box label min max` quintuples — a **clip envelope**, drawn as a **layer inside the box it names**, over whatever that box draws and lasting exactly as long as it does, which is what a box's own dynamic envelope, its pan and its per-box effect parameters are. A layer carries no height, and that shape is the statement: a layer is as tall as the box it is on. **A box is a window onto a source, and an edge stops where the source does.** `loops` names the boxes whose window **wraps**, space-separated, the way `hidden` names the curves that are not drawn — and it is a name set rather than a field of the septuple because nothing here changes it: whether a box loops is what the *multitrack* says it reads past its source's end, and a report carrying the flag would be reporting a fact this widget cannot edit. What it decides is the two things that follow from it: an edge drag on a box that loops may be pulled anywhere (past the end is the beginning again, and before the start is the tail of the iteration before), one on a box that does not **stops at the last frame**, and a box over samples nobody loaded stops at nothing since there is no length to stop at. The picture follows the same statement: a looping box draws its samples wrapped under it, one that does not draws them once and nothing after. **A box is measured in the timeline's samples and filled with its source's frames**, and those are one number only while the source was written at the rate the timeline measures in. `rates` is what says otherwise: the flat `box rate` pairs, `rate` being how many frames of its source one sample of that box is -- the source's own rate against the timeline's, times the region's playrate. A box not named reads one frame per sample, which is every box of a session recorded at its own rate; a 44.1 kHz take on a 48 kHz timeline reads `0.91875`, and without it that box is drawn 8.8% longer than its samples and its edge stops 8.8% short of them. It is a name list rather than a field of the septuple for the reason `loops` is: it follows from the source and the region, so a hand cannot edit it and a report carrying it would be handing back a number the widget was told. **The sound crosses by the same ratio and does not need telling**: the reader a box plays through takes it off the buffer (`BufRateScale`), so what is here is the drawing's half of one rule. And it reaches the dots: zoomed to the sample a resampled box draws **the samples it plays** -- the reader's own linear reading between the source's frames, on the timeline's grid -- rather than the source's frames where they fall, since those are samples nothing produces. **A join is drawn from the takes it reads.** `segments` is the flat `box source start frames rate` quintuples for a box whose source is a join: the spans it is made of, in the order they play, each the `source` server buffer read from frame `start`, filling `frames` frames **of the join**, at `rate` frames of that take per frame of the join -- one, unless the take was written at another rate, which a join reads through rather than converting (the server's own stitch reads it by the same ratio, off the buffers). A box named there is drawn span by span from those takes, and its own `source` is not fetched: a join owns no samples, so its picture is of the takes it is spans of, which are the ones already on screen, and the box is drawn the moment the join is made rather than once its buffer has been built and downloaded. The fades at a join's seams are not drawn. `points` is the flat `curve time value shape amount` quintuples for **every** curve there is, rows and layers alike, each naming the curve it is on the way a note names its box — one carrier, because a break-point is a break-point wherever the curve hangs. A curve naming a lane or a box that is not there is kept and drawn nowhere; a point naming a curve that is not there is dropped. A press lands on a curve's **own points and the line between them**, never on the rectangle it shares, so an envelope drawn across a box leaves that box draggable by every pixel the line is not on, and the press that takes a curve moves `layer` to that curve's name and says so once. Moving a break-point reports **`"points"`** — the whole list, every curve, the third of the payloads, idempotent and its own inverse like the other two. **Every gesture reports the multitrack, never the gesture.** A clip moved, trimmed or dragged onto another lane leaves as one `"clips"` payload carrying the clips as they now stand — the same flat sextuples the prop takes, so applying what came back is the identity. There is nothing for a reader to choose between and no state a hand can put the widget in where the report changes shape, which is what `"clip"`, `"clips"` and `"lane"` on a `track` could not say. A press that found no box **declines**, so the slack between clips stays the container's — a sweep there starts a marquee, and a click places the **position cursor**, which is how a mark is put down without reaching for the ruler at the top of the window — and **a gesture that changed nothing sends nothing**. **A multitrack selects boxes**, as a lane does: a plain drag on bare stack sweeps a **marquee** and the clips the rectangle covered go into the hand, of every lane it crossed — a selection the stack's sweep made is not one lane's — and **no band is written**, because the second axis here is the stack and not a value. **A click on a clip selects that one, alone** — decided on release, because a press is not yet a gesture: the same movement is a click or a drag depending on what happens next, and collapsing the selection at the press would let go of a block the hand was about to move. **Alt+click adds or removes** one instead. Grabbing a **selected** clip moves the whole block rigidly, across the stack, clamped as one so a block stopped at an edge does not fold against it; grabbing an unselected one lets go of the block and moves that clip alone, and an **edge** is always one clip's. `q` quantizes the held clips onto `snap` and Delete removes them. **A drag also snaps to the edges of the boxes already on the lane**, within a few device pixels: with no quantization a hand never lands one box exactly where another ends, so two of them could not be made to meet at the sample and `j` had nothing to join. It is a snap to **content** and stands beside `snap` rather than replacing it — a grid says where a beat is, this says where the music already is — and a hand that keeps pulling past the tolerance goes on through and **overlaps** them, which is a crossfade and legal. The box under the hand is what snaps (a block travels with it), and an **edge** drag snaps too, which is how a gap is closed by trimming rather than by moving. **A selection is the hand's, not the document's**: nothing on the wire sets or reports it. The header's mute, solo and **level knob** report **`"lanes"`** — the lanes as they now stand, the second of the two payloads, so a level moved does not resend every clip. The level is a **knob** and not a groove because a header is a narrow band beside a lane: a horizontal fader long enough to be read takes the width the name needs, while a dial reads and turns in a square cell like the two toggles beside it. It turns by a **relative** vertical drag (the press itself changes nothing), which is what a dial with no left and right end can mean, and by the same distance every `knob` in this host turns by. Beside them a fourth cell, **`A`**, shows and hides this track's **automation rows**, lit when they are shown. It is offered on **every** track, including one with no automation at all, because the first press is what **makes** one — the same shape a double click on a header has, where the verb makes the track rather than opening a question about it. What it makes is the **gain** curve, flat at unity across the multitrack, and it is heard like any other: hidden is a view's word, so a curve nobody is looking at is still applied. The second press hides what the first made rather than making a second. It rides the same `"lanes"` report the three beside it ride, and the owner answers by saying which curves are visible (`hidden`), which is where that fact lives: a row a multitrack shows is the multitrack's, and the toggle is a control over it rather than a second place for it to be recorded. *(Provisional, and the shape rather than the design: what a track may automate is its own question — a port list, a plugin's parameters — and this is the smallest thing that makes an arrangement with automation editable while that is worked out.)* **And a track shows what it produces**: `meters` is the flat `lane level mark channels` quadruple array — the control buses that lane's meters write, the level run and the run of held peaks, one bus per channel each — and the host reads them itself every frame out of the shared segment, so a level that moves every block costs no message. A prop of its own rather than two more fields on `lanes`, because `lanes` is what a hand *edited* and a bus number in it would be a number the host was expected to hand back unchanged. What it draws is a thin column per channel down the **right edge of the header**, over the amplitude that track is producing after everything has been applied — its clips' own gains, its curves and its fader — in **decibels** against a -60 dB floor (half of unity is -6 dB, which is a tenth of the way down and not half), with the held peak as a hairline across the column. Its width follows the channel count and nothing else, so a moving level never moves the controls under it, and a lane with no entry draws no strip at all, which is what a multitrack nobody is playing looks like. **The header is a surface, not a shelf for three buttons**: the space beside them is where the *track* is addressed, and its **bottom edge** is where the row's own height is — a drag there is the vertical zoom of one track, which is what a hand reaches for when one take needs to be read closely and the rest do not. That height is **screen state**: nothing on the wire sets or reports it, and a `lanes` payload's own `height` is laid under whatever a hand set, so a level moved or a track added does not take a reader's zoom away with it. **The wheel reaches the stack too**, and it is the one gesture an automation row has for its own height, having no header edge to pull: `Shift`+wheel **scrolls the stack**, clamped so a stack that fits does not move and one that does not cannot be pushed past its last row, and `Ctrl`+wheel **zooms the row under the cursor** — a lane or an automation row alike, each on its own. A plain wheel is still the time axis', which is what it is over every timeline view here. **A modifier is taken whether it moves anything or not**: passing an unusable wheel on is the right rule for a *surface* under the pointer and the wrong one for a modifier, which is an address rather than a place — a stack already at its end does nothing, rather than letting the stack zoom under the hand at the moment it reaches the last track. A press on an **automation row's** header addresses no track at all — a curve is drawn in a row of its own under the lane it belongs to, so the band beside it is that row's label and selects, adds and removes nothing. A **click** there selects that track and says so (the wash and edge a held clip is drawn in), which is the second coordinate a paste needs — the position cursor says *when*, the selected track says *where*; a **double click** asks for a track, and the new one lands **after** the row that was pointed at (or at the end, on the band under the last header, where there is no track to point at and nothing else could be meant); and **Delete** with a track selected and no box held removes that track **and everything on it**. All three report `"lanes"`, because a row report is the multitrack's tracks: a name that is no track's id is a track a hand made, a track the report leaves out is gone with its boxes, and the order is the report's. A selected track is the **hand's**, like the box selection: nothing on the wire sets or reports it. **The keys, over the held set and across the stack**: `q` quantizes onto `snap`, **`e` splits and `j` joins** — the same two letters a clip is cut and joined with on a lane, and the same the roll spends on notes — Delete removes, and `Ctrl`+`C`/`X`/`V` move a block through the host-wide clipboard, in the same JSON form a `/gui_set clips` accepts. `e` cuts at the **window's cursor** (a key gesture has no pointer to read a position from), and the window over the contents moves with the cut, so the second half reads on rather than restarting; the host **mints the new clip's name** from the one that was there, the way it numbers a marker it adds, and it comes back in the report like any other. A join is a **lane's**: a lane is what makes two clips joinable, as a pitch is what makes two notes one voice. A paste needs **two** coordinates and has both, and they anchor the block at **two different boxes**: in time it starts at the **earliest** of them, which lands on the cursor, and in rows at the **topmost**, which lands on the **selected track**. Everything else keeps its distance from those two, so the selected track is where the block *begins* and nothing is ever pasted above it — boxes from tracks 2, 4 and 1 pasted onto track 3 are tracks 4, 6 and 3, gaps included. A block pasted onto a track is the same block, so what is kept is its shape and not the row numbers it was cut from. With no track selected the rows are the ones it came from, which is what a paste back into the same multitrack means. A block that would run **past the last track** is refused out loud (`"refused" "paste" …`) rather than flattened onto it: the multitrack gains no track from a paste, since making one is a verb of its own. *(Which keys these are is not settled — an element performing a verb is the wrong place to name it. See `clients/gui/PLAN.md`, "A shortcut is the application's, not the widget's".)* | `lanes`, `clips`, `notes`, `curves`, `layers`, `points`, `layer`, `hidden`, `loops`, `rates`, `meters`, `view`, `gap`, `snap`, `label`, `ruler`, `tempo`, `sample_rate`, `markers`, `sel_*`, `cursor`, `playhead_at`, `playhead`, `playhead_loop_*`, `link`, `theme` |
| `patch` | A **directed, typed patcher**, drawing both levels: boxes with **inlets on top, outlets on the bottom**, a **cord** per `outlet -> inlet` connection, **coloured by rate** — contrasting primaries at one width — audio (`ar`) red, control (`kr`) blue, init (`ir`) yellow and dashed — colour carries the rate. At **level 1** (a `GraphDef`) a cord *is* a server bus (not drawn — the client names it); at **level 2** (a `SynthDef`/`FaustDef`) a cord is an internal UGen wire. A **canvas**: a box with `x`/`y` places freely; a box **without** `x`/`y` takes its slot in the host's **layered (Sugiyama-style) auto-layout** (ranked by longest path to a sink, so inputs sit above their use and sinks at the bottom). Boxes drag (`"move"` flows back) — a box already selected carries the whole set — a click or a marquee on empty canvas selects, through the container's own `select` step (a click is a rectangle of no size, so it lets go), and inside a `scroll` workspace the whole patch pans and zooms; the labelled panel frames whatever boxes it holds | `boxes` (each `{def, inlets, outlets[, x, y, role]}`; a port is a bare name (audio) or `{name, rate}` with `rate` `"control"`/`"init"`; `role` `"source"`/`"const"` only tags a box for drawing — a `const` value box gets a distinct fill — the layout ranks every box by its cords), `cords` (a flat `[from_box, outlet, to_box, inlet, ...]` list, indices within each box's inlet/outlet lists), `label` |
| `score` | An **engraved music-notation page**. The client engraves a score and sends a *display list* — a glyph-outline table keyed by SMuFL codepoint plus placed glyphs, staff lines, stems, beams, slurs and text in page units — which the host fits into the widget and tessellates into the same triangle mesh as the rest of the chrome. Every primitive carries the MEI `xml:id` it was engraved from, so a click names an element (`"element"`), a drag transposes it (`"transpose"`, naming the staff position reached), a press on blank paper names an insertion point (`"insert"`, on a page that took `entry`) and the page shows a playback cursor over its own timemap | `vb` (the `[width, height]` page-unit viewBox), `glyphs` (hex SMuFL codepoint → outline path `d`), `prims` (the placed primitives, each with its `id`), `cursors` (the cursor track: `t` in ms → `x`, `y0`, `y1`), `step` (page units per diatonic step), `elements` (the ids that name a **sounding element**, as against the staff and layer furniture that also carries one — the engraving walk knows, and a renderer cannot re-derive it), `display_list` (the whole drawing replaced live, as a JSON string), `playhead`, `playhead_at`, `playhead_loop_*` (ms), `sample_rate`, `selected`, `editable` (opt into pitch editing; off = a read-only view), `entry` (opt into note entry; off = a press on blank paper only clears the selection) |
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

**A step declines, or it refuses; the two are not the same and the difference
is what a plan is made of.** A step that finds *nothing to act on* where the
picture is perfectly good — the samples are not drawn one by one, the view
measures no second axis — is not that gesture's press to take: it **declines**,
and the plan tries its next step. That is what a composed plan means, and why
`"sample select"` edits where the samples are visible and sweeps where they are
not. A step that finds the *picture wrong* — a view holding no samples at all,
one that cannot hold an edit in flight — has hit a **fault**, and a fault is
said out loud (`"refused" <verb> <reason>`) and **consumes** the press. Falling
through there is how a pencil silently becomes a selection tool: the reader
aims to write, gets a sweep, and learns that the tool sometimes does not work.

So a step's own row below says which of the two each of its dead ends is, and
where two steps look like they disagree — the pencil refuses at a zoom the
`sample` grab declines at — they are answering different questions. There is
nothing to *grab* on a summarized trace, and the plan is welcome to sweep
instead; a *stroke* that fell through would turn a refused edit into a
selection, which is the one thing it must never do.

| Step | What it does |
| --- | --- |
| `element` | Hands the press to whatever is under the cursor — the widget the pointer found, or the clip, note or box the container drew there. It may decline (empty space), and the plan goes on |
| `pan` | Pans the container's axis: time on a `field`, the plane on a `plane` |
| `select` | Sweeps a **time range**: the container's shared selection, the span every linked view draws and the transport loops inside. It is a *state* — the span itself is what is selected, so it outlives the gesture. A view holding contents of its own is also asked what the rectangle covered (a roll's notes, in the band of semitones it reports), because there the span and the notes under it are one hand's one meaning |
| `marquee` | Sweeps a **selection of objects**: the clips of a multitrack, the boxes of a patcher, the notes of a roll — the things the rectangle covered, and no span. It asks both of whoever holds contents: the lanes of the stack it sweeps down, and the element it was begun on. The rectangle is the gesture's own picture and is gone when the hand lets go; what stays is what is selected, drawn as selected. A press is that rectangle at no size, which is how a click lets go of everything. It is the plain drag of a `track`, and a `plane`'s element claims the press for it, because only that element knows where its own paper ends |
| `select_box` | The same sweep **restricted on the second axis**: a rectangle over a view that measures a value, reported as the two further arguments of `"selection"`. It **declines** where the picture has one measured axis, so `"select_box select"` is the plan for a mixed stack — a rectangle where there is one to draw, the plain span where there is not |
| `sample` | Grabs the **sample** under the pointer on a navigable trace and drags it vertically — the smallest destructive edit. It **declines where a sample is not a thing on screen**: below the zoom at which the trace marks each sample with a disc there is nothing to grab, and the plan falls through to its next step, so `"sample select"` edits where the samples are visible and sweeps where they are not — deliberately the opposite of what `draw` does at the same zoom, for the reason above. A **fault** it still refuses out loud and consumes: a view holding no samples at all has nothing to grab for a different reason, and one the reader needs told. One intent leaves, on release, as `"sample"` |
| `draw` | **Draws** over the samples: a press-drag writes the value under the pointer for every sample it passes — the ones *between* two motion events included, by interpolation, or a fast stroke would leave the samples combed with holes — and emits one `"draw"` on release. **Refused until the picture draws its samples one by one** — the trace's own dot threshold, asked rather than restated, so the pencil is allowed exactly where the reader can see which sample they are aiming at — visibly (`"refused" "draw" <reason>`) and consuming the press, so a plan naming a sweep behind it cannot turn a refused stroke into a selection. **Every other way it can fail is refused the same way** — a view holding no samples, a view that cannot hold an edit in flight, a press before the take begins — because past the point where the pointer is inside the trace's own body, the press is the pencil's and a silent fall-through is a pencil that became a selection tool |
| `marker` | Adds a **marker** under the pointer, or removes the one already there, and emits `"markers"`. A new one is *numbered* (`"1"`, `"2"`, …), which is what makes the gesture usable with no text entry in front of it; renaming and recolouring are the owner's, through the `markers` prop. It **declines where there is no ruler** to put one on |
| `locate` | Puts the transport's cursor under the pointer and emits `"locate"` — or, when the pointer is within reach of a **marker's arrow**, at that marker's own time |
| `none` | Nothing |

A container may also name **no** step for a modifier, which is not the same as `none`: an empty plan declines and the press walks outward, while `none` consumes it. That is a `clip`'s own table — the plain drag grabs it, every other chord falls through to the lane — because a clip is a container of its *local* axis, so a pan there would mean the wrong window.

The order is the point. `"element marquee"` is a lane — grab the clip under the
cursor, and if there is none, sweep for the clips; `"select"` is a waveform,
which has nothing on its axis to grab and whose selection *is* a span; `"select_box select"` is a stack of heavy views,
where the same chord draws a rectangle on the pictures that have two measured
axes and a plain span on the ones that do not. A plan that consumes nothing falls **outward** to
the container around it, which is how Shift+drag on a patcher's empty canvas
pans the workspace the patcher sits in. The defaults are the behavior described
throughout this chapter (`{"drag": "element marquee", "shift": "pan"}` on a
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

**A control's travel: the curve and the step.** A `knob`, a `slider` and a `number` map the handle's travel onto `min..max` **linearly** unless told otherwise, and two props say otherwise. `curve` bends the axis: `0` is the linear default, negative spends most of the range on the first half of the travel and positive on the last half — the fine-at-the-bottom feel a frequency or an amplitude control wants, and the same number sclang's `lincurve` takes. It is the same bend the clients' `lincurve`/`curvelin` run and an envelope segment runs on the audio thread, because all of them read the shared core's warp family rather than each deriving one — two implementations of one curve is how the same control comes to feel different in two places. `step` is the grid a **drag** lands on, in the value's own units: `1` over `0..127` is the integers a MIDI note number wants, and a Faust parameter arrives with the one its `hslider` declared. It is counted from `min` and never leaves the range, so a grid that does not divide it (`0..10` by `3`) stops at `9` rather than on an off-grid `10`, and a reversed range (`min > max`, a legitimate control) steps from its own `min` downward. The step is a rule about **the hand**: a value the script sends — as `value`, or through `/gui_set` — is drawn as sent, because a control shows what it was told. Both props are live via `/gui_set`. There is deliberately no *named* spec (`"freq"` for 20..20000 exponential): a name that silently drew the wrong curve would be worse than no name, and these two props are what one would be built out of.

**Text on the light widgets.** Every text-bearing light widget — `label`, `button`, `toggle`, `text`, `number`, `menu` and the control labels on `slider`/`knob` — takes a `text_size`: a glyph scale over the host's embedded bitmap font (default `2.0`, the size everything drew at before the prop existed; clamped to `[1, 16]`). The face is a 5-column cell with a 7-row body — the height a line reserves — plus the room a diacritic takes above it and a descender below, so it writes **both cases** and the **Latin-1** letters: a label, a track name or a file path in Spanish, French or German reads as written. A character it does not carry draws as a hollow box. Single-line text that overflows its rect clips with an ellipsis instead of bleeding into the neighbor. Text drawn **over a picture** rather than on chrome — a clip's name over its take, a roll's cursor read-out over its notes — sits on a **text plate**: a translucent, rounded ground (the `plate` color role, the `plate_radius` size role) that keeps the line readable wherever the samples under it is dense, without hiding it. It is as wide as the line it grounds, so a truncated caption takes a truncated plate, and a box with no room for a glyph draws neither. `label` additionally takes `wrap` (word wrap on the measured width of the words, the lines past the label's bottom edge dropped) and `align` (`start`, the default left edge / `center` / `end`, applied per line). All of them are live via `/gui_set`. `text_size` lands on **half-steps** of the cell — a bitmap glyph is scaled by repeating its own pixels, and a scale that does not divide the cell evenly makes those pixels unequal — unless the host was built with a rasterizer (its optional `font-atlas` feature, `--font <path>` natively or a face the page pushes in), where the prop is continuous and text draws through a real typeface. That is the **only** place two builds of the host differ, and it changes nothing else a script can see: the sizing table never followed the typeface, so the same document lays out identically either way. Inside a `scroll` workspace everything scales with the view's zoom together — the text, the padding, a control's own parts — because a zoom is an enlargement: a box at zoom 2 is the same box twice the size, not a box with oversized text jammed into it.

**The keyboard, and where it points.** One widget at a time holds the **focus** — the host's, not a window's, since there is one keyboard — and it is the only widget keys reach. A press moves the focus onto a widget that reads a keyboard and off one that does not — **with one exception: a press on the free-standing ruler of the very axis the focused widget is on leaves the focus where it is.** The position cursor is placed on a ruler and nowhere else, so dropping the focus there would take a view's keys away with every mark a reader puts down (point at a box, place the cursor, split it). A ruler takes no focus of its own — it is the axis' chrome, not a keyboard sink — and it takes none away from what it rules; **Tab** walks the window's focusable widgets in layout order and **Shift+Tab** back along them; and the focused widget is drawn with a ring in the theme's `focus` role. Every move **the user makes** is reported as `/gui_event <id> "focus" <1|0>` — both ends of it, so a script that mirrors the focus sees the one that lost it too. A script may point the keyboard itself with **`/gui_set <id> focus 1`** (`focus 0` gives it up), and that one is *not* echoed back, exactly as no other `/gui_set` is: the script already knows what it asked for. `focus` is the one key that is not a prop: it says where the keyboard is, so a `/gui_query` does not report it and a widget that reads no keyboard refuses it rather than swallowing it silently.

**Tab past the last widget leaves the tree**, deliberately: a GuiDef mounted in a web page sits *inside a document*, and a ring that wrapped would trap the keyboard in the canvas and make the page around it unreachable. So the ring runs out, the focus clears, and the browser's own tab order carries on. In a desktop window there is nothing outside to hand it to, so nothing is focused and the next Tab enters the ring again. Composition (IME) and the system clipboard stay the **page's**: a canvas cannot host an input method, so the host reads the keys the page forwards it and nothing more.

**The two switches, and what a press means.** A `button` is momentary and a `toggle` latches, and both send a **pair of values** rather than a boolean: `on` and `off`, `1`/`0` unless the def names another pair. A bypass lives at `0.0`/`0.7` and a mode at `1`/`2`, and neither is a span a widget could be drawn over — which is why it is a pair and not a `min`/`max`.

A button's `mode` says which of the two pointer primitives reaches the server:

- `gate` (the default) sends `on` at the press and `off` when the button is let go, so the value lasts exactly as long as the button is held — what an envelope's gate reads, and what a trigger control ignores the tail of by definition.
- `press` sends `on` at the press and **nothing** after it: one message, the bang.

**A widget cannot make a value instantaneous.** What is sent is held by whoever receives it, so `press` is a bang only against something that returns to zero on its own — a trigger control (`tr`), which the server resets after one block — or against a script, for which one `/gui_event` message *is* an event. Both clients refuse to build a `press` button over any other control, because it would leave `on` standing forever.

**Press and release are the primitives, and a click is not a mode.** Everything else a pointer does to a button is composed from the two: a click is a press and a release that landed inside, a double click is two of those inside a window. Those are gestures and belong with the gestures; what a `mode` says is only which primitive reaches the server.

**So a button says two things at once, to two audiences.** Its **value** is a control signal — `on`/`off`, which a `/gui_bind` forwards to the audio server without the script ever seeing it. Its **interface events** are what the hand did, and they take a road of their own: `"press"` when the pointer goes down, `"release"` when it comes up — wherever it came up — and `"click"` in addition when the release landed on the button rather than off it. A press the hand slid off before letting go reports the release and no click, which is the cancellation every desktop convention gives a command button and a piano key must not have.

Those three go to the script **bound or not**: a binding forwards a widget's value, and a command is not a value. That is what lets one button be a synth's gate and a panel's command at the same time. The two vocabularies are additive, so a script may read the value, the hand's events, or both on one widget. The host decides whether a release was a click by the widget's own declared shape and the same hit slop a press is filtered through — it is the machine's hit test asked a second time, not something an element computes for itself.

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

**A note is bounded by the view it is drawn in.** The roll's own view spans its
own content: drag a note rightwards and the roll simply reaches further, so the
note is one scroll away and nothing is lost. A roll drawn as a *body* inside a
box is clipped to that box, so the same drag would leave the note out of every
pixel the box owns — still in the list, still sounding, and findable only by
resizing the box by hand. So a body's edit is bounded by its container where its
own view is not, and the rule is the container's rather than the note's.

A timeline view (and a lane) shows the transport two ways, and they are different
things: `playhead_at` **anchors the line to the engine clock** (it is the clock
value at timeline position 0, so the line *sweeps* as the audio runs), while
`playhead` is a **static cursor** — where a located, stopped transport sits. Both
are group-wide, so every lane shows the one cursor; a negative value is none.

**A window has one cursor, and the content never moves it.** Its position is a
time on the axis, and it is placed by a **click** — a press that never left the
`hit_slop` — anywhere on that axis: the ruler, a lane, a roll's grid, and equally
a clip or a note drawn on them, since what took the press is not what names the
time. The host moves the cursor itself and sends `"locate"`, which is what
re-cues whatever sounds; and a click that lands while the transport is *running*
re-anchors `playhead_at` too, so the line carries on from where the hand pointed
instead of running on from where it was. The steps that answer for the pressed
pixel themselves — `locate`, `marker`, `sample`, `draw` — place no second cursor.

**Which clock the line sweeps by is `/gui_headClock`**, and it decides how much of
this a client has to do. On `device` (the default) the counter never stops, so
the client owns the position: it anchors `playhead_at`, re-anchors on every
locate, and parks the static cursor to make a pause look like one. On
`transport` the counter *is* the transport's position — held while the transport is stopped,
moved by `/transport_locate`, wrapped in the engine at a `/transport_loop`'s end
— so the anchor is simply `0` and the three things stop being the client's:
pausing is `/transport_stop`, seeking is `/transport_locateSample`, and a loop
needs neither `playhead_loop_*` nor a message when it wraps. That is the shape a
multitrack wants, where many readers follow one time (`TransportPos`), and it is
why an editor sends a locate and reads a position back instead of computing one.

A sweep that **repeats** a region — an editor playing a selection on a loop, a
looping clip — sets `playhead_loop_start`/`playhead_loop_len` (the same sample
units, group-wide too): the host folds its own swept position inside
`[start, start + len)`, so the line follows the loop on the **same one anchor**
and a repeating pass still costs one message rather than one per frame. A
non-positive length is the straight pass, which runs on past the region. Anchor
a looped pass at `clock - start`, not `clock`, so the first frame lands where
the reading actually begins. On a `score` the pair is in ms, its own unit.

The lanes of a window share **one navigation group**: they zoom, pan and carry a
playhead as one, and the axis spans every lane (the longest clip end). The
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

**Note entry is its own opt-in** (`entry: true`), and deliberately not a second
meaning for `editable`: it takes over a gesture that already does something. On
every other page a press on blank paper clears the selection, so a page that had
not asked for note entry would start reporting an insertion every time a user
dismissed one. With it on, a press that lands on blank paper inside a staff
emits `"insert" <after> <position> <staff>` — the element the note would follow
on that staff, where on the staff the press landed, and which staff.

It names a **place and not a note**, which is the same division every other
score gesture keeps. A staff position is not a pitch until something knows the
clef and the key, and the host knows neither; a duration is not implied by a
click at all. Both stay the driver's, which is where the score is. The host's
whole contribution is the measurement it is the only one able to make — where on
the page the finger went — reported in the ids the client engraved.

The client's editor may well move a note by *steps* (verovio's does); it
subtracts the note's current position to get them, against the engraving it
holds at that moment. The host owns no notation, so it cannot apply the edit; the driver does,
re-engraves, and replaces the drawing with a single
`/gui_set <id> display_list <json>` — the drawing layers only (`vb`, `glyphs`,
`prims`, `cursors`, `step`, `elements`), the same ones the widget was defined
with.

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
