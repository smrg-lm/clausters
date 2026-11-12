# The GUI host: widgets, drawing and views

Part of the [design record](../decisions.md), whose index lists every
entry's title.

## Editor y-axis navigation: gestures on the ruler strip, props in display units

The editor-grade views (waveform, spectrogram) zoom and pan vertically. Two
choices were on the table and both are worth recording:

- **Gesture surface: the y-ruler strip, not a modifier over the body.** The
  wheel over the strip zooms the vertical axis and dragging the strip pans it;
  the wheel over the
  body stays horizontal zoom and plain/Shift drag keep selecting/panning time.
  Spatial separation needs no modifier, leaves every existing body
  gesture untouched, and matches the audio-editor convention (Audacity, most
  DAWs pan/zoom an axis by grabbing its ruler). The strip only exists when the
  ruler is on (`ruler_y != "off"`), which is also the only time the axis has
  visual feedback to navigate against.
- **Prop shape: `y_start`/`y_len` in normalized display units** (0 = axis
  bottom, 1 = top; `0, 1` = no zoom; a non-positive `y_len` resets), on the
  shared `EditorProps` — not amplitude/frequency values. Display coordinates
  make the anchor math linear and unit-independent: the spectrogram's window
  survives switching `freq_scale` between linear/log/mel/bark without moving
  what is on screen (the nonlinearity lives entirely in the shader's
  display→bin mapping, as with the shader uniforms), and the waveform's
  amplitude window composes with `AMP_MARGIN` without leaking it into the
  protocol. Living in the widget tree means `/gui_set` drives it and the
  browser renders it through the same shared frame path with zero extra wiring
  (display + `/gui_set` parity; the drag/wheel gestures, native-only at
  first, later moved into the shared gesture machine both fronts drive).
  Changes emit `/gui_event id "view_y" y_start y_len`, the `"view"` posture.

The y state is deliberately **per widget** while the horizontal view is slated
to move into shared navigation groups (linked views): two lanes of one file
scroll in lockstep in time, but each keeps its own vertical slice.

**The zoom anchor follows what the axis measures**, because that one window is
shared by every *channel* lane the view stacks — a multichannel file draws N
lanes against a single `y_start`/`y_len`:

- **Frequency (the spectrogram): the cursor's height**, which is the frequency
  under it. A shared window says the same thing in every lane — all of them
  show that band — so anchoring at the cursor is both meaningful and what the
  reader wants.
- **Amplitude (the waveform): the window's own centre**, whatever lane the
  cursor is over, so zero stays at the centre of every lane and the trace grows
  and shrinks inside it. This is the audio-editor convention (a vertical zoom
  is a symmetric change of amplitude scale), and it is the only anchor that
  survives multiple lanes: an anchor taken from the cursor's height is
  meaningless for the *other* lanes, and any off-centre window pushes the wave
  out of its lane and clips it — a wheel near the top of channel 2 used to
  shove every channel's wave against the bottom of its lane. Panning the strip
  still reaches an off-centre region when one is wanted.

## Dragging a clip to the edge scrolls the view

A clip drag mapped the cursor through the group window captured at the *press*,
and nothing moved that window while the drag was in flight. So a clip could
never travel further than one visible window's worth per gesture, and holding
the cursor against the lane's edge did nothing at all — a standing pointer
sends no events. Zoomed in far enough to place something precisely, that was a
sliver: the only way to move a clip across the piece was to zoom out (losing
the precision), drag, and zoom back in.

The fix is the audio-editor convention: **held against the edge, the drag pulls
the view along**. Two decisions inside it are worth recording.

**The cursor maps through the current window, not the press-time one.** That is
the whole mechanism — panning the view under a standing cursor moves the sample
beneath it, and the clip follows. The press snapshot stays for what it was for
(`press_sample` is a *timeline* coordinate, so it is unaffected by the pan, and
a clamped edge still cannot drift).

**The scroll rate is a fraction of the visible window per second, not a pixel
rate.** Zoomed in, a fixed pixel rate would crawl — the window is a sliver of
the piece, so a pixel is worth almost nothing; zoomed out it would fly off the
composition. A fraction of the window behaves the same at every zoom, which is
what a gesture has to do. The pan clamps against the group's span, and that
span already grows as the dragged clip extends the content, so the view keeps
making room instead of stopping at the composition's old end.

The tick is the front's frame timer — the one already driving meters, scopes
and the playhead — so both fronts get the behaviour from one shared
`Gestures::tick`, and a window that is otherwise still runs the timer only
while such a drag is in flight.

## The looping playhead: an explicit loop region, not a flag over the selection

The swept playhead is one subtraction in `host::frame`: a view with
`playhead_at >= 0` draws at `sample_clock - playhead_at`. That is the whole
reason following a transport costs **no messages** — the line is redrawn from
Rust every frame at the display's rate, never sent by a client — and it is
exactly right for a straight pass. It had no notion of a loop, so anything
repeating a region (an editor's "play selection", a looping clip) could not be
followed at all: the line ran past the region and off the view. The two
workarounds a client could reach for are both worse than the gap — re-anchoring
per pass puts a message back in the loop and drifts against the audio, and
sending a position per frame gives up the property the anchor exists for.

Two shapes were weighed. The one chosen is an **explicit loop region** —
`playhead_loop_start` / `playhead_loop_len`, with the swept position becoming
`start + ((sample_clock - playhead_at - start) mod len)` and `len <= 0` keeping
the straight sweep. The alternative was a `playhead_loop` **flag** wrapping
within the existing selection, which reads better in the one case that motivated
this and costs a single boolean. The explicit region won because it does not tie
playback to a selection a gesture can change under it: a drag on the view while
a loop plays would silently move what is heard, and a client that loops
something *other* than the selection (a clip, a timeline range) would have no
way to say so. The cost is one extra prop.

It lives on the shared editor chrome, so `waveform`, `spectrogram`, `track`,
`clip` and `pianoroll` get it at once, and it is **group-wide** like the anchor
— linked views must wrap at the same place, or one file's waveform and
spectrogram would draw the line in different spots. The navigation group is
where it is *kept*, and every view reads it from there; a widget's own props
only seed the group when it is first built. The `score` carries the
same pair in **ms**, its own unit, since its cursor rides an engraved timemap
rather than a sample axis. The static `playhead` keeps its meaning, and a client
still sends **one** message per transport state change.

One consequence worth stating, because it is the easy mistake: a looped pass
anchors at `clock - start`, not `clock`. The reading begins at `start`, so
timeline position 0 sits that far before the clock value the client has;
anchoring there makes the swept position `start` on the first frame and the wrap
returns it there on every lap.

## Linked views: explicit groups in the host core, shaped for the multitrack view

The linked editor views (a waveform lane and a spectrogram lane navigating as
one item) settled four decisions:

- **Grouping is explicit — a `link` (int) prop — not implicit by shared
  source.** An editor item legitimately wants *unlinked* views of one file
  (compare two zoom levels), and a link legitimately spans sources (aligned
  takes), so the source is the wrong grouping key on both sides. A negative
  `link` unlinks live, and the widget carries its current view along
  (unlink-to-diverge is the workflow); joining an existing group adopts it.
- **Group events carry the interacted member's id, not a group id.** Every
  `/gui_event` names a widget id; a group id is not a widget and would fork
  the event shape. A gesture emits `"view"`/`"selection"` once — linked
  members repaint but do not re-emit — and a script that cares which lane was
  touched gets that for free.
- **Shared chrome is plain composition, not an automatic strip.** A stack of
  linked lanes that wants one time ruler sets `ruler:"off"` on all but one —
  existing containers plus existing props, no new container kind, no protocol
  change. An automatic shared strip would need cross-widget layout coupling
  for something a prop already expresses.
- **The group state lives in the host core and navigates in session-timeline
  units.** The state (horizontal view + selection + playhead anchor, in
  `host/timeline.rs`) is keyed by group, not by window: membership spans
  windows and fronts, the group's length is the max over its members'
  registered data extents (a shorter take just ends earlier), and both fronts
  drive it through the same `Host` ops — the `/gui_set` interception lives in
  the core dispatch, so the browser gets group semantics with no front code.
  This shape is deliberately the seat of the future multitrack (DAW-style)
  view: a clip item there is a member with a *placement* (a start offset) on
  the same shared timeline — the one designed extension point; today every
  member sits at offset 0. Selection/playhead are mirrored group→members'
  `EditorProps` so everything that reads the widget tree (renderer, playhead
  animation, readouts) keeps working; the group is the single writer, so the
  mirrors cannot drift.

## Edit-back-to-data: flat event payloads, the binding path generalized to lists

The `bpf` envelope editor is the first widget that *writes data back*, and the
pattern it establishes is meant to be reused verbatim by the later drawn-buffer
and automation-lane cases. Four decisions:

- **Edited data flows to the script as new event *payloads*, never new
  addresses.** An edit emits `/gui_event <id> <tag> <flat values...>` — for the
  envelope, `"points"` followed by `t v shape curve` per breakpoint, ints kept
  int and floats float. The `/gui_*` address family does not grow per widget;
  bulk edits (a drawn buffer region) would ride one blob in the existing
  `samples_to_blob` little-endian `f32` layout. The flat-primitives boundary
  rule holds on the way back exactly as on the way in.
- **The server-bound direction is the widget-value binding generalized to a
  list.** A bound editor forwards its whole edited structure straight to the
  audio server — `Binding::message_args`/`Host::forward_args` send
  `addr prefix… values…`, the multi-argument form of the bound knob's
  `addr prefix… value` — bypassing the script exactly as `/gui_bind` promises.
  No new binding vocabulary: the same destination, prefix and prune rules.
- **The host's mapped resources stay read-only.** Edits mutate the host's
  typed tree and flow out over the two channels above; the mmap bulk path is
  never written through. Anything that must land in a shared file or a server
  buffer goes through the server (or the script), so ownership of bulk
  resources stays single-writer.
- **The envelope shape math lives once, in the core.** The editor draws
  segments through `clausters_core::envshape::shape_value` — relocated from
  the server, whose `EnvGen` now delegates to it (the same move the forward
  FFT made) — so what the editor draws is what the server plays by
  construction. Its FFI export is deferred until a client actually evaluates
  envelopes client-side (the tap-reader precedent): the Python leg only maps
  breakpoint lists to/from `Env`, which needs no curve evaluation. The
  breakpoint model, hit-testing and drag clamps are display logic and stay
  gui-side (`host/bpf.rs`).

The widget model itself is deliberately the future automation-lane shape:
values in any `[min, max]` (bipolar, unipolar, on/off lanes via the hold
shape — SC's step jumps to the *target* level at segment start, so a step
segment shows the next point's value), every standard `EnvGen` transition
curve, an optional exponential
display scale for frequency-like ranges, and times over an explicit
`[0, duration]` domain — so a multitrack automation view later composes this
model instead of designing a new one.

## The patcher is a directed, typed graph

The logical side of a composition — members wired to each other — is drawn by the
`patch` widget (the P track). The first design drew it **undirected**: a bus as a
first-class node, a wire as an untyped `(control ↔ bus)` touch, on the argument
that a GraphDef says only that a control *names* a bus and the writing/reading
direction is the server's own topological sort — so a directed view would be a
guess. That was built, tested, and rejected; it lives on the `patcher-bus-as-node`
branch.

**Context — the undirected view could not be read.** On real patches a
bus-as-node, single-edge canvas fails at a patcher's one job, showing signal flow:
ports land on one side and zigzag to bus nodes, an inlet and an outlet meet the
same bus with no visible order, and the picture does not match the running
program. And the premise was wrong. Direction is **not** a guess from a control's
*name* — it is **structural**: a control that feeds an `In` UGen is an input (an
inlet), one that feeds an `Out` is an output (an outlet), one that feeds neither is
a value, not a port. The client that built the def (or reads a UGen descriptor)
knows this exactly — it is the same analysis the server does to order the graph,
not a naming convention.

**Decision — one directed, typed cord patcher, at two scales.** A box has
**inlets** (top) and **outlets** (bottom), each **typed** (`ar` audio, `kr`
control; level 2 adds `ir`), and a cord runs outlet → inlet, a rate mismatch
refused at the gesture. A **cord *is* a bus the user never numbers**: the inlet
defines the bus (its type, and the summing point for the cords into it — several
cords into one inlet *sum*), and the **client names one bus per connected net** and
writes it into the def. The two levels are the same grammar over different objects:

- **Level 1 — visual programming of a `GraphDef`.** Boxes are whole synthesis
  nodes (defs of all families, synths, groups) and buffers; a cord is a **server
  bus**. The graphic is **explicit and restricted** — a signal goes exactly where a
  cord connects it. The power to route arbitrarily is a property of the *server's*
  node/bus architecture (nodes and groups inside a control cycle), built in code,
  never drawn on this canvas.
- **Level 2 — visual programming of a `SynthDef`/`FaustDef`.** Boxes are UGens with
  `In`/`Out`/control edge boxes; a cord is an **internal wire** of the single def
  the patch compiles to.

**Who does what — the server and the client barely change.** The **server already**
allocates a GraphDef's bus numbers and **auto-orders** its members topologically,
considering the buses; the directed patcher adds no server ordering logic and needs
no new verb (the port directions come from the def the client already has). The
**client** contributes one small, language-agnostic pass — directed cords → one bus
named per net, summing fan-in — which lives in `clausters-core` beside the patch
document, so every client shares it. The Python arrangement model (`Aggregate`/
`Generator` → `GraphDef`) is unchanged; the new work is GUI-side (the directed
`patch` widget).

**Consequence.** The picture reads as the program: outlet → inlet, typed, flowing
one way, no bus nodes cluttering the canvas. The honesty *inversion* from the first
design is deliberate — direction and type are **shown** because they are
structural; bus numbering is **hidden** because it is bookkeeping. The one thing the
drawing does not express is **feedback** (a reader ordered before its writer, a
block of latency): the patch graphic is a **DAG**, and a genuine cycle stays a code
construction, not a cord.

**The hardware output is not a box.** A cord *is* a bus, and buses are never
drawn or numbered — so the hardware output, being a bus, cannot be a box either.
There is no `OUT`/hardware node on the canvas and no drawn hardware net; every net
is a private bus (`b0`, `b1`, …). A signal reaches the speakers through a
**terminal def** — a `dac` with an inlet and no outlet, its `Out.ar(0, …)` baked
in — a member like any other, distinguished only by having no outlet to cord
onward. (This corrected an earlier design that kept a special `OUT` box wired to a
drawn hardware bus, which contradicted "a bus is never drawn." Level 2 is
different: there a def genuinely contains `In`/`Out` UGens, so they *are* edge
boxes of the patch.)

## A buffer in the patcher is a data value, not a box that is a def

The level-1 patcher draws **generators that are defs**: a box is a whole
SynthDef/FaustDef, a cord is a signal bus between two of them. A **buffer** is not
one of those. It is *data a def reads* — a `Buffer` control on a playback synth —
exactly as a number is data (a `freq`) or an envelope is (an `env`). So a buffer
in the patch is a **value fed to a box's parameter**, of the general family of
**parameter/value boxes** (a constant, an envelope, a buffer) that feed a def's
parameter inlets. That family is **not built yet**: the level-1 patcher so far
wires def-to-def buses only, and a def's own parameters-as-values are the
value-box work (P5, the consolidated editing surface). Buffers-as-boxes therefore wait for the value-box work,
rather than bolting a "buffer rate" onto the cord→bus pass now — the coherent
order is to introduce parameter boxes first, of which a buffer is one kind.

**Context — why not a third cord type now.** The tempting shortcut is a `buffer`
rate beside `ar`/`kr`, a "buffer outlet" feeding a `buf` inlet, compiled by the
cord→bus pass into a scalar the server resolves. But that pass is about **signal
buses** — connected components summing onto a numbered bus — and a buffer
reference is neither a signal nor a bus; overloading it would make the one clear
rule ("a cord is a bus") lie for one case. A buffer is a *value*, and values are a
different box family than signal-carrying defs.

**Decision.** A buffer in the patch is the arrangement's own `Buffer` (a reference
to a server buffer), kept in the model for **parity**: the patch is a visual
representation of a *part of the arrangement model*, so what it shows a buffer to
be is what the model holds, not a patch-only artifact. Two facilities follow, both
**deferrable and both off the server buffer the model already points at** — no new
model state:

- **Visualize** — to *see* a buffer, fetch its samples from the server and draw
  them the way the audio editor's waveform view already does.
- **Play** — to *hear* a buffer, build a **temporary synthdef** that reads it, the
  same move the editor makes to audition a take.

So P3 ships the def-generator patcher and leaves buffers to follow the
parameter/value box work — both parts of the plan, in their natural order;
recording the shape now keeps that later implementation coherent with the model
instead of a patcher-only bolt-on.

## The piano-roll: OSC items get their own lane, and the notes live once

The editor-grade `pianoroll` draws the two message families a sequence carries —
MIDI notes and OSC items — and it had to place them without lying about what
each is.

**Context.** A note has a pitch, so it maps naturally onto the grid's vertical
axis (pitch × time). An OSC item does not: a `/trig` or a `/cue` is a moment, not
a pitch. Forcing it into the grid means inventing a vertical position for
something that has none — the same kind of lie the patcher refused when it
declined to guess signal direction from a control's name.

**Decision.** MIDI notes draw in the pitch × time grid; OSC items draw as markers
in a **separate lane** below it, on the shared time axis but with no pitch
pretence. Velocity gets its own lane too (the DAW convention), so the grid stays
one clean plane of notes.

**Reuse.** The note primitives — the notes, the pitch/time mapping, the drawing,
the hit-test, the drag clamps — live **once** in `host::pianoroll`, shared by the
dedicated `pianoroll` widget and the multitrack `clip`'s roll body (which now
delegates its drawing there). This is the `bpf::place_point` move again (G22h): a
mapping-free core both the widget-local and the clip-placed editors call, so the
two can never disagree on where a note is drawn or grabbed. The `Note` carries
`velocity`/`channel` (a real MIDI note), and the wire form is the flat quintuple
`start dur pitch velocity channel` — a length that is a multiple of five is read
as quintuples, anything else as a legacy `start dur pitch` triple list, so old
data still parses.

**Placement (the G7b rule).** The one piece of *general* musical knowledge — the
MIDI-note ↔ name / black-key spelling on the keyboard — went to
`clausters_core::scale` (`note_name`/`pitch_class`/`is_black_key`), beside the
perceptual frequency scales; its FFI export is **deferred** until a client
evaluates it client-side (the `envshape`/bus_tap-reader precedent). Everything else is
display-only and stays gui-side. The edit-back stays flat payloads, not new
addresses: `"notes"` (`start dur pitch velocity channel …`) and `"osc"`
(`time label …`) — the fourth use of the one edit-back pattern.

**Multi-note selection: the marquee is the time selection, restricted in
pitch.** The piano-roll needed a multi-note selection without stealing the
grid's one free gesture — a plain drag on empty grid is the heavy views' *time
selection* (the linked group follows it), and a DAW marquee wants that same
drag. The resolution is that there is no second gesture: the marquee **is** the
shared time selection, restricted in pitch. Dragging the empty grid keeps
driving the linked views' time span exactly as before, and the notes inside the
time × pitch rectangle become the selected set (Alt+click toggles one note in
or out; Delete/Backspace removes the set; dragging a selected note moves the
block rigidly, its clamp computed as one so an edge stops the block instead of
folding it; the velocity lane nudges the set relatively, each note saturating
on its own -- the lane went on 2026-09-27, and Shift with a vertical drag on a
note nudges the set the same way, the velocity drawn inside the note; *corrected
2026-11-10*: the lane is back as one of the lanes a roll shows one at a time,
chosen by tabs drawn right over it -- a bar per note at its start, dragged alone or as the
selection, and a drag across it a ramp -- because a fill inside a box shows
one velocity at a time and draws no ramp, while the velocity stays drawn as the
fill and Shift with a vertical drag still nudges it). The set itself is **native view state**, not a wire prop: it lives
in the widget state, clears when the script replaces `notes` (the indices would
dangle), and every block edit reaches the script as the same flat `"notes"`
payload — the wire did not grow, so every client consumes block edits with the
code it already had.

**Edit-back to the data (the Editor's dedicated view).** When the `Editor`
opens an element as a dedicated piano-roll, a per-note edit writes back only to a
**generated element**: a `Track`'s editable `Timeline` is rebuilt from the
`"notes"` payload (times converted to beats, the OSC/MIDI items sharing the
timeline preserved). A generator (`Pbind`/`Routine`) is forward-only — there is
no second note to rewrite until it evaluates again — so its bounced notes are
shown **read-only**, and bouncing it to a `Track` is what makes them editable.
This is not a new rule: it is the generated/generator distinction of the
arrangement model (above) doing its job at the granularity of a single note.
OSC items stay display-only in this view too: the `(time, label)` marker is a
lossy projection of the message, so writing it back would silently drop the
arguments.

## The piano's host voices use explicit node ids from a dedicated high window

> **Superseded (2026-09-13).** The host allocates every node id, bus and
> buffer from the one client id policy (`clausters_core::ids`), with a share a
> launcher gives it (`--id-share`), and takes node ids back on `/node_end`. The
> explicit ids and the reason for them stand; the fixed window does not: it
> knew nothing of the server's size, recycled nothing, and the monitor and the
> piece had grown windows of their own beside it.

Context (the `piano` widget's voice mode): a key press must spawn a server
voice the key *release* can later reach — `/synth_new` on press, `/node_set <id> gate
0` on release. The server-assigned id form (`/synth_new … -1`) was rejected for
exactly that reason: the host would never learn the id it needs to gate, and
adding a reply round-trip for it would put a network latency inside a played
note. So the host sends **explicit positive ids**, which the server accepts
from any client, and allocates them from a dedicated window:

- **Base `0x1000_0000`, wrapping over a `1 << 16` span.** Far above the Python
  client's counter (`1000..`) and the server's own auto-assign range
  (`1000 + 4·max_nodes ..`), so a host voice can never collide with a node a
  script created — the three allocators partition the id space by
  construction, with no coordination protocol.
- **No `/node_end` tracking.** A voice def is required to free itself on release
  (`FREE_SELF` on the gate envelope), so the host's bookkeeping is just the
  live `(pitch, node)` pairs per widget: the release (or a glissando, a
  re-press, a widget free/redefine — all of which gate the old voice) removes
  the entry. A 65536-id window wraps long before any voice from the previous
  lap can still be sounding.

The same reasoning will apply to any future host-managed spawner (an XY pad
playing voices, a drum grid): reuse this window, not a new one per widget.

## The 2D workspace has one uniform scale, and it clips with geometry rather than a scissor

Context (the `scroll` container): the workspace shows a virtual content area
through a panning, zooming window. Two implementation choices went against the
obvious reading of the roadmap, and both are load-bearing enough to record.

**One scale, two offsets — not a `viewport::View` per axis.** The plan said
"one `viewport::View` per axis, so the anchor-preserving zoom/pan math is
reused rather than rewritten". A `View` carries a *start and a length*, so a
pair of them carries two independent scales, and each axis' clamp adjusts its
own length — which silently de-couples them the first time one axis hits the
content edge, and the plane shears. A workspace showing boxes and wires must
never distort, so the state is instead **one uniform `view_zoom` (device pixels
per content unit) plus a pan offset per axis**. The reuse the plan asked for
still happens, just at the right level: the anchor-preserving *pivot math* of
`View::zoom` is applied to the shared scale factor (`host/scroll.rs::zoom_at`),
so the content point under the cursor stays fixed exactly as it does in the
timeline views. The constrained forms then fall out of configuration rather
than of geometry: `axis` gates which offsets a gesture may move, `zoom: 0`
gates the scale — one gesture path, as intended.

**A geometric clip, not a per-widget scissor.** A scrolled widget must not
paint outside its container. The reflex answer is a GPU scissor rect per
widget, but the host's whole light-widget economy rests on the opposite
property: *a whole window is one mesh upload and one draw call*, which is what
lets an application face cost what a panel of sliders costs (the L-track cost
rule). A scissor is pipeline state, so honoring it per widget would split that
batch into one draw per clipped widget. So the clip lives in `paint::Mesh`
instead: a clip rectangle set around each placed widget's geometry, applied by
a Sutherland-Hodgman polygon clip as triangles are emitted (a triangle against
a rectangle yields at most 7 vertices; fully-outside geometry is dropped
before it reaches the buffer). The batch stays single, and the clipping is
identical on both fronts by construction because it happens before the GPU.
The **heavy views keep a real scissor** (`host/frame/mod.rs::apply_scissor`) —
they own their own pipelines and draw through `set_viewport`, which positions
but does not cut, so there is no batch to protect there and nothing else can
do the job.

**A third, found by using it: the plane and the scroll view want different
bounds.** The first cut clamped every axis to `[0, content - visible]` — the
window stays on the content. That is right for a *document*: a scroll view must
not scroll above its first row. Applied to the free plane it is wrong, and
visibly so: a plane sits at its content's top-left corner by default, so half
the drag directions were clamped dead and the whole gesture read as broken. The
bound now follows what the axis *means* — a constrained axis (`axis: "x"`/`"y"`)
keeps the strict clamp, the free plane overscrolls by half a viewport past each
edge (`host/scroll.rs::SLACK`), enough that every direction always moves it and
little enough that the contents can never be lost off-screen. This is the same
general-first rule the milestone states, applied to the bounds themselves: the
strict clamp is the *constrained* behavior, and letting the general case
inherit it gave the plane a restriction only the special case means.

## The editable text field carries an internal clipboard, not an OS-clipboard dependency

The editable `text` widget needs cut/copy/paste, and that is the one editing
facility a GUI toolkit normally reaches the operating system for. We do not: the
native front (winit) exposes no clipboard, so real OS-clipboard interop would
mean a new crate (`arboard`/`copypasta`) — and libwgpu, the only heavy
dependency the host already carries, covers none of it. Against that, the host
already had the precedent from the piano-roll (G24h): a **host-wide in-process
clipboard**, a plain buffer that a copied block travels through between widgets
and windows. The text field reuses exactly that shape — a host-wide `String` —
so cut/copy/paste work with **zero new dependency**, at the one cost that the
native clipboard does not interoperate with other applications.

The browser is the asymmetric case: a page reaches the real OS clipboard through
the DOM (`navigator.clipboard`, the `paste` event) with **no Rust dependency at
all**, so the web front is where OS-clipboard interop is actually free. The first
cut ships the in-page clipboard on both fronts (functional parity within the
host); wiring the web front's copy to `writeText` and its paste to a DOM
`paste`-event listener is the recorded follow-up — an enhancement of one front,
not a dependency the whole workspace takes on.

## Representation before editing: the patcher's Def-views are autonomous, decoded client-side

The patcher track (P) first grew view and editing together, per level: P3
delivered the level-1 `GraphDef` view *and* its edit-back in one arc, driven from
the editor, and the level-2 milestone was scoped as "UGen boxes that compile to a
def" — a surface editable from the start. Two frictions surfaced. First, the
edit-back reached into the arrangement model early (`Group.declare_bus`, the
`Group → GraphPatch` mapping living inside `editor.py`), coupling a still-moving
view to the model before the drawing itself was settled. Second, nothing yet drew
a def's *internal* structure at all — level 2 was planned as an authoring surface
without first being a faithful picture of an existing `SynthDef`/`FaustDef`.

**Context — a patcher is easier to edit once it can be read.** A patch's one job
is to show a structure truthfully (the same finding that turned it directed and
typed — see "The patcher is a directed, typed graph"). Editing affordances —
creating boxes, drawing cords, rewiring — are far easier to get right when they
sit on a faithful, closed drawing of a structure that already exists (the def)
than when the view and the editing grow in the same step and destabilize each
other. And the structure is *already in hand*: a def the client built holds its
whole graph in memory — the DFS that `synthdef_ports` already walks to find the
`In`/`Out` controls is, one step deeper, every UGen and every wire — so a view can
be *decoded* from the def rather than authored from scratch.

**Decision — split the track into representation (phase A) then editing (phase
B), and make each Def-view autonomous.** Level 1 (`GraphDef`) and level 2
(`SynthDef`/`FaustDef`) are both delivered **as read-only views first**, decoded
from the def's own structure by an inverse pass (`GraphPatch.from_graphdef`,
`DefPatch.from_synthdef` — the reverse of the compile that renders them), before a
box can be created or a cord rewired. A Def-view is an **autonomous
visualization**, a peer of the heavy views (waveform, spectrogram, oscilloscope,
timeline, bpf, env): a host widget pure over its data, fed by a headless
decomposition (`defs`/`clausters-core`), openable on its own — one window per def,
the `clausters.plot` posture. The **editor does not own it**: the editor is the
gui-side *representation of the arrangement model* (`clausters.form` is pure and
never imports the GUI, so the dependency runs editor → form, never the reverse),
and it merely *orchestrates* — embedding an autonomous Def-view as a lane when a
composition holds a logical aggregate, the way it embeds the piano-roll. The already
shipped level-1 edit-back (P3d) is reclassified into phase B, ahead of its
representation work, and its `Aggregate → patch` mapping is lifted out of `editor.py`
into `defs`.

This is sharpened by a plain fact about the **current state of development** (not
a premise): no editing is realized yet. The level-1 edit-back (P3d) shipped as
code but is unproven in the window, and nothing else is built, so every surface
the track has today is, in practice, a representation. This is not a claim that
views cannot edit — editing is exactly what phase B adds, driver-side over the
edit-back seam (the widget emits gestures, the driver applies them).
Representation-first is therefore not only the better sequence; it is the honest
description of where the track already is.

**Decision — the internal graph is read client-side; the server is not
extended.** Level-2 representation reads the def object **in memory**, which
covers the authoring case (a def you built or loaded through the client).
Inspecting a def the server holds but no client in this process built would
require the server to report each def's internal UGen graph over `/def_query` —
**deliberately not done**: it would add per-def storage and processing to the
server for something the arrangement model does not do for its other abstractions
either. It stays an explicit later decision, never a prerequisite of this track.

**Consequence.** Phase A is `GraphDef → GraphPatch` (level 1, lifting the mapping
out of the editor) then `SynthDef → DefPatch` (level 2), plus a free-standing
opener; phase B is the editing — **consolidated into one milestone to plan**
(structural creation, parameter/value/buffer boxes, the live patch, and the
level-2 compile) that the shipped P3d edit-back seeds — followed by persistence as
its own milestone. The directed/typed grammar,
the cord→bus pass and the `clausters-core` patch document are unchanged — this is
a re-sequencing and a placement discipline, not a redesign of the model. The
free-standing opener is a **`plot_def()` method on the def classes**
(`graphdef.plot_def()`, `synthdef.plot_def()`, `faustdef.plot_def()`),
deliberately not a global verb and kept distinct from `clausters.plot(def)`, which
renders the def's *sound* rather than its structure.

**What level 2 added, concretely (P4), and what it did *not* add to the core.**
Because the level-2 Def-view is a *read-only decode*, it has **no cord→bus pass** —
a cord is an internal UGen wire, never an allocated bus, so there is nothing to
compile and nothing language-agnostic to place in `clausters-core` beyond the
shared drawing vocabulary. So the core gained exactly one thing: **`Rate::Init`**
(`ir`), the third cord weight (drawn dashed) the level-1 audio/control pair lacked;
the `DefPatch` *model* stays client-side Python, the peer of the client-side
`GraphPatch` (level 1's model is Python too — only its *pass* is core). This
refines the P-track's earlier phrasing that "the patch document schema and the
pass land in `clausters-core`": that holds for the level-1 pass; level 2, having no
pass, keeps its model where `GraphPatch` already lives. Two smaller decisions fell
out of decoding a def **headless** (`plot_def` runs no server): a UGen box's inlet
names come from **the client's own builder signatures** (`ugen_input_names`, the
same callables the `/ugen_query` contrast test pins to the server registry — no new
verb), falling back to positional for the generic op UGens and the wire-misaligned
kinds; and an **unset UGen output rate defaults to audio** — the exact per-kind
default is the server compiler's, not the client's, so the view takes the honest
common-case guess rather than mirroring the rate table. The decode stays faithful
where it must: `DefPatch.to_synthdef` reconstructs the def and reproduces its spec.
Faust decodes the tractable **signal-tree** form node for node; a **box-tree or
source** def is opaque (its internals are the compiler's) and draws as a single
box. Several refinements followed the on-screen reviews. **The node-positioning
is host-side, not client-side:** the decode ships no coordinates — positioning
belongs with the widget that draws, so every client rebinding it gets the layout
for free — and the panel frame hugs whatever boxes it holds rather than clipping a
fixed rect. **The layout is a small layered (Sugiyama-style) graph drawing, not a
tree algorithm:** a def is a **DAG** (fan-in, fan-out, shared sub-graphs, several
`Out` sinks), so a single-root tree layout (Reingold–Tilford) does not fit — it
would have to duplicate shared nodes. `host::patch::auto_layout` instead layers
each box by its **longest path down to a sink** (so an input lands just above
where it is used rather than piling into one top row — the mistake a first
attempt made by pinning all inputs to the top), then orders and places by iterated
**barycenter** relaxation (each box pulled to the mean x of its neighbours, layers
re-sorted and packed) so children line up under their parents; dummy nodes for
long edges are a noted future refinement. **Constants are value boxes, not inline
defaults:** each literal input becomes a small `const` box captioned with the
number and corded into its inlet (so every inlet is wired and the round trip is
uniform), drawn with a distinct `value_fill` so a data box reads as data. **Rate
reads by colour, not weight:** a cord is coloured audio-green / control-blue /
init-amber (init also dashed) — stroke weight alone was too subtle to tell apart.
And the panel **caption is the def *kind*** (`synthdef` / `faustdef` /
`graphdef`), not the def's name — the view names *what* it draws; the window title
carries the name. The per-box **role** the decode still ships (`source` / `const`
/ `object`) is now only a *drawing* tag (the `const` fill), not a layout input.

## GUI widget ids are allocated client-side and recycled; a name is the stable handle

*(Refined since: a widget that draws part of a structure takes an id **named**
after what it draws rather than a lease — see "A widget id is named after what
it draws, not leased" below. What follows still holds for a tree built by hand.)*

Context (the client's GUI ergonomics): a high-level client should not make the
user pick and thread integer widget ids — the audio-server side never does (a
script writes `Synth("beep", {"freq": 440})`, and the client's
`NodeIdAllocator` names the node). The GUI's id handling grew crudely in the
opposite direction: two disjoint monotonic counters (the host client from 1000,
the multitrack editor from 10 000, partitioned only by convention), neither
recycling, and examples that hand-pick ids (`knob(10, …)`) purely so they can
match the `/gui_event` back. Two questions had to be settled: **where** ids are
allocated, and **how** a script refers to a widget without naming an integer.

**Allocation stays client-side, mirroring `NodeIdAllocator`.** The tempting
alternative — the host assigns ids the way `/synth_new … -1` lets the audio server
assign a node id — was rejected for the GUI for the same reason the piano's host
voices reject it (see above): the client needs the id *immediately* (to `set`,
`bind`, wire an edit-back), so a host-assigned id would force a reply round-trip
into the build path, and an async id-resolution step into every `open`. The
audio server's own primary path is client-side too; the `-1` convention is a
secondary path for cases where the *server* generates the node (a GraphDef's
members). The GUI has no such server-generated widgets, so it needs no `-1` path
at all. So a single `GuiIdAllocator` — the GUI sibling of `NodeIdAllocator`, over
the same core `Registry` — owns the one namespace, and the editor draws from it
instead of a second counter. It is **bounded and recycling** (unlike a bare
counter): a freed subtree returns its ids to the pool, and re-defining a window
frees the old subtree first, so the editor's redraw churn reuses ids within a
fixed window instead of climbing forever.

**A widget is named, not numbered.** `open`/`define` return a window handle that
is the window id *and* resolves the tree's `name`d widgets, so a script writes
`win["cutoff"].set(value=800.0)` / `.bind(…)` / `.on_event(fn)` and never touches
an integer — the `WidgetHandle` delegates to the host with the resolved id, the
way `Node.free()` delegates to its `Server`. The `name` is a **client-only** key:
it is stripped from the JSON, so the host still sees only ids and the wire is
unchanged. The deeper reason to prefer a name over a hand-picked id is that the
name is the *stable* identity: an assigned id recycles across redraws, so it is
the wrong thing to hold, whereas the name a script drew a widget with is what an
edit-back or a live `set` should address against. This keeps the wire protocol
and the host untouched — the whole change is the client growing an allocator and
a handle layer, no `ABI` counter moves — which is the "when a feature could live
in the client or the wire, keep it where the system already keeps it" rule.

## Signal logic moves into the core when a second process draws it

The GUI host was, for a long time, the only thing that read the server's live
data: a `meter` names a control bus, a `scope` an audio tap, and the host
subscribes, aligns, transforms and paints on its own. So the algorithms behind
those views lived inside the host crate — the oscilloscope's trigger alignment
in `host/oscil.rs`, the spectrum's decibel scaling inside `host/spectrum.rs` —
and that was the right place while it lasted. Both are pure and both were
already shared by the host's two fronts, native and browser, so nothing about
wasm forced a move: the browser front computes them fine.

What forced it is the web client's data paths (W10), which let a **script** read
the same bus, the same tap and the same buffer and draw its own canvas. That
makes a page a second drawer of the same trace, and a second drawer is exactly
the condition the shared core exists for. The alternative was worse in two ways:
a re-implementation of the trigger in TypeScript would drift from the host's
silently (the failure mode is not a crash but two subtly different pictures of
one signal), and exporting the host's internals from its own wasm bundle would
make a script that draws its own canvas download the whole GPU host — 5.3 MB
against the core's 438 KB — and turn the host crate into a host *and* a function
library.

So `oscil` and `spectrum` are now `clausters-core` modules, the host consumes
them from there, and the browser reaches them through `clausters-core-web`
alongside the peak pyramid and the stereo-field measurements. The precedent was
already in the file next door: the spectrum's FFT and Hann window had made the
same trip earlier, for the same reason (the spectrogram and the spectrum view
had to agree bin for bin).

The rule this settles, stated as a test rather than a taste: **signal logic
belongs to the host only while the host is the only one computing it.** The
count of consumers is what decides placement — not whether the code is "display
math", and not which targets it compiles to. What stays in the host is what has
no second consumer by nature: geometry, hit-testing, chrome, and the state that
is a *look* rather than a measurement — a spectrum's averaging across frames, a
scope's history depth. Those are decisions about how a picture should feel, and
two views are entitled to disagree about them.

**The test was wrong, and the move it justified was right** *(2026-08-20)*. The
second drawer is gone: a client draws nothing at all now, and the trigger, the
decibel curve and the pixel row left the web client with the doors onto them
(`clients/web/PLAN.md`, W26). By the rule above, `oscil` and `spectrum` would go
back into the host — and they are staying where they are, which says the count
of consumers was never the reason. They are functions of a **signal**: a
headless client measuring one, a test asserting one, a future analysis path
would each want the same numbers, and the core is where a thing that has an
answer independent of any screen belongs. What the host keeps is what only a
renderer does. The rule as it now stands is the placement rule in
`architecture.md`: **everything drawn is drawn by the host**, and the arithmetic
of a picture goes into the host or into the core for the host to read — never
into a client, where a second implementation would be reachable from one
language only.

## A widget's own size is a function of the table, and the layout still measures nothing

Until now a GuiDef declared *every* size that was not an even split: a control
in a column got half the window unless the script wrote `h=28`, and the numbers
in the examples were guesses that happened to look right. Giving each widget
kind a **natural size** — how big it wants to be — removes the guessing, and the
question is what that size is allowed to depend on.

It depends on two things: the host's sizing table and the widget's own
presentation props (`text_size`, whether it carries a label, whether it wraps).
It deliberately does **not** depend on the widget's data. A label's width does
not follow its string, a scope's height does not follow its sample count, a
menu's does not follow its options. The reason is not purity: a size that reads
the data turns `/gui_set` — the incremental update the whole protocol is built
on, arriving many times a second — into a relayout of the window, which is both
a per-message cost and a visible jump under a live value. A widget with more
content than room clips or scrolls; it does not grow. That is also what keeps
the layout **one pass with no measurement**: a container never asks its children
how big they are, it asks each kind what it wants, which is a table lookup.

Two consequences worth stating, because both look like omissions:

- **An explicit `weight` beats the natural size**, and comes before it in the
  resolution order (fixed `w`/`h` → `weight` → natural → a share of the
  leftover). Without that, "stretch this button over the pane" would have become
  inexpressible the day buttons learned their own height — and it is the *only*
  thing a single-control window (a bound knob, a standalone bundle) wants.
- **A container does not hug its content.** A row of menus does not shrink its
  panel to their height, because that is the measurement pass this layout does
  not have. Chrome that must be thin still says `h`. What changed is that the
  number inside the strip is no longer a guess: the strip's children size
  themselves, and the strip states its own extent.

  *(Superseded, and by composition rather than by measurement: a container that
  asks for it with `hug` is now the composition of its children's own natural
  sizes, and a window that asks for it is the composition of its content. Both
  bullets above still hold everywhere the prop is absent, which is everywhere it
  is not written — see "A size may read a prop, never a value" below.)*

Which kinds have one follows the content/surface split: content whose extent the
widget itself knows (a label's line, a button, a toggle, a number, a menu, a
single-line field, a slider's thickness across its track, a knob's height, a
ruler strip's) versus a surface whose extent is the caller's (a panel, a scroll,
a patch canvas, a track, the heavy views, a plot, a node tree, a canvas). Mixed
is ordinary rather than exceptional — a slider is intrinsic across its track and
elastic along it — and a knob is intrinsic only in height, because its disc
sizes itself to the shorter side of its body and centres there: extra width is
slack it absorbs, so a row of knobs still spreads instead of packing left.

This redrew every existing GuiDef, which was accepted deliberately: the windows
that improved are the ones where a caption or a control had been taking half the
pane from the view beside it.

## The wire is logical, the plane is physical, and the shell is the one that knows the scale

The sizes a GuiDef declares — `w`/`h`/`x`/`y`, a container's `margin`/`gap`, a
widget's `text_size` — were physical pixels, which meant a window was as small
as the display was dense: on a doubled HiDPI screen the tree drew at half its
apparent size while the window itself (winit's `LogicalSize`) came out at the
size the script asked for. The wire is now **logical** and the host resolves it.

**One scale per window, resolved once.** The size table (`host/metrics.rs`) that
L6 centralized is logical too, and each window holds its own resolution of it —
`Metrics::resolved`, every role scaled and re-quantized by its family (extents
onto the 2-px grid, hairlines onto whole pixels, glyph scales onto half-steps).
That runs on a **scale change**, not on a frame: layout and painting stayed the
code they already were, reading one table through `Host::metrics_for(def_id)`, so
the per-frame cost of HiDPI is zero — which is what matters on a page holding
forty canvases. Snapping is part of resolving rather than a pass afterwards,
because the chrome *is* hairlines: a divider, a track edge and a font pixel are
one unit each, and a fractional position turns a crisp line into a two-pixel
grey smear.

**The scale is an input the shell writes, never something the core detects.** The
platform-agnostic host may not ask a window manager or a DOM what density it is
on — that is the seam `check-wasm.sh` enforces — so `Host::set_ui_scale` is a
door the shells push through: natively from winit's `scale_factor`, re-armed on
`ScaleFactorChanged` (whose `inner_size_writer` is *answered*, with the same
logical extent the window had, so a 800x600 shell stays a 800x600 shell across
the move); in the browser from the page's `devicePixelRatio`.

The browser needs its own *re-arm* for the same reason, and it is not the one a
page would reach for first: a `ResizeObserver` watches the **CSS** box, so
browser zoom or a drag onto a monitor of another density moves the ratio while
the box stays exactly as it was — no callback, and the host goes on resolving
against a scale that is no longer true. So the page watches a media query on the
current ratio (`(resolution: 2dppx)`) and re-arms on the new one each time it
fires (`onScaleChange`). Two triggers, because the box and the density are two
facts.

That last one reshaped the browser binding rather than porting anything. The host
was told its canvas size in device pixels only — the page multiplied its element
box by `devicePixelRatio` and handed over the product — so **the ratio was
destroyed at the boundary** and no arithmetic recovers it. `resize` now carries
it (`resize(defId, width, height, scale)`), and `canvasBox` reports the box and
the ratio side by side instead of collapsing them. The pair does not collapse in
the other direction either, which is why the binding takes device pixels and the
ratio rather than the logical box: the `<canvas>` backing store is an integer the
page has already fixed, and a host that recomputed it from logical times scale
could land a pixel off it — surface and element misaligned, exactly the blur this
milestone exists to remove.

**A zoomable view's default zoom belongs to whatever its content unit is.** The
plane's own units left one thing unanswered: on a doubled display a patcher's
boxes came up half the apparent size of the chrome around them. The fix is not to
scale the plane's geometry (that would put a second multiplier inside a space
that already has one) but to start the plane at the density it is drawn on: a
`scroll`'s `view_zoom` — physical pixels per content unit — **defaults to the
window's UI scale**, so one content unit is one *logical* pixel, and a named zoom
(from the tree or from the wheel) is literal from then on.

A default nobody can name needs a way back, which is why `/gui_set view_zoom 0`
(or any non-number) **clears** the zoom rather than setting one: a script that
wants "the plane as it opened" has no number to ask for — naming `1.0` pins one
physical pixel per content unit, which on a doubled display is half the size the
plane came up at. The shape is borrowed from `theme`, where an empty value drops
the overlay.

Fitting the zoom to the content was rejected as the default: a plane's content
unit is a *display* unit (a box is 96 units wide because that is how wide a box
should look), so fitting would make a box's apparent size follow **how many boxes
there are** and re-zoom the plane on every edit — the "size follows data" failure
the natural sizes were built to avoid. Zoom-to-fit is a command.

The rule sorts the other navigable views without a second thought, because their
content unit is *data*: a `waveform`'s and a `spectrogram`'s is the sample, a
`pianoroll`'s vertical is the semitone, a `score`'s is the engraved staff step.
None of them wants the display scale anywhere near its window — a denser screen
means the same span at finer resolution, which is exactly "never resolve the
signal finer than the screen" read from the other side. Their defaults (the whole
buffer, the pitch window fitted to the lane, the page fitted to the rect) are
already fits to their content, and stay untouched.

**A zoom is an enlargement, so a placement's scale picks its size table.** The
default exposed a second thing the plane had never really done: only *text*
scaled with the zoom, while every metric role (padding, a slider's track, a
knob's diameter, the gap between rows) stayed at the window's table — the
"patcher posture" that was tolerable when the zoom was 1 by default and reads as
broken proportions the moment a box is drawn at another scale. So a placement
carries the table it is drawn with (`Metrics::at`, resolved at the placement's
scale), and the layout measures with the same one: inside a scrolled box the
declared lengths, the roles and the text all carry one factor. The layout
distinguishes three spaces to make that exact — the window, a plane's *content
units*, and the inside of a scrolled child at the accumulated zoom — which is
also what stopped a plane's default margin from being counted twice.

Two more bugs surfaced with that default and were older than it, both from mixing
two spaces inside one expression: a plane's content extent was compared against
the pane's *pixels* (identical at zoom 1, off by the zoom at any other, which
pushed a self-centring graph into the corner), and a widget's natural size was
measured at its `text_size` while its drawing uses `text_size * scale` — so
inside a zoomed plane a box promised room for a 14-pixel line and drew a 28-pixel
one, and a knob, whose height is label strip + disc + read-out strip, gave the
disc away to the text and rendered as a dot. Both are the same lesson: **an
expression may not mix a content unit with a pixel, or a declared text size with
the size it draws at.**

A third had the same shape and cost the wheel its anchor: the content extent of a
graph-sized plane is "the graph, but never smaller than the viewport", and once
the viewport was converted with the *live* zoom the content shrank as the zoom
grew — so the plane the pivot math holds a point in was itself moving, and the
graph slid out from under the cursor. The conversion uses the plane's **natural**
scale, which makes the extent constant under zooming.

**Two pixel spaces coexist, and that is the invariant to state.** Chrome is
logical; a *navigable plane* is physical. A `scroll` workspace's content plane
keeps its own units — its `content_w`/`content_h`, its `view_x`/`view_y` and its
children's placements — because it carries a zoom of its own and its pan is
written in the pixels the pointer moved, so folding a second multiplier into it
would make a drag and a declared position disagree. The heavy views say the same
thing for the same reason: `render_width_px` is physical and "never resolve the
signal finer than the screen" is untouched. What follows from this, and is
deliberate: a widget's own interlocking structural geometry (the patcher's
box/port series, the roll's key gutter and lanes, the score's staff step) is
module-local by L6's rule and lives in the physical space too, so a plane's
interior does not grow with the display's density — it grows with its own zoom.

## A binding fires an apply, never another binding

A widget's value could already leave for the audio server without the script
(`/gui_bind … "server" …`). Pointing it at **another widget** — a menu flipping a
`stack`'s page, a slider driving a plot's `max` — is what makes a persisted
GuiDef an application rather than a picture: the pages flip inside the host,
with no client attached at all. But two widgets bound to each other is a cycle
the moment an apply can trigger a delivery, and a GUI that live-locks on a click
is worse than one that cannot switch tabs.

The rule is **one hop, by construction**: what a widget binding fires is the
mutation `/gui_set` performs (`Host::set_props`, extracted for exactly this) and
nothing else. That method applies the prop, mirrors it into the registry and
asks for a repaint; it never enters the delivery path, so the target's own
binding does not fire from it. There is no cycle detector and no depth counter,
because there is no chain to walk — and the alternative, letting a binding
cascade and then policing it, would add a feature nobody asked for and a class
of hangs with it.

- **The destination keyword was already on the wire**, which is the whole reason
  this was additive: `/gui_bind id "server" …` spelled out a destination when
  there was only one. So the binding became an enum of destinations rather than
  an address with a keyword in front of it, and a widget binding carries no
  address, no prefix and no server to be missing.
- **An apply can ask for a repaint where a forward could only ask for a
  datagram**, so the delivery seam had to take a `&mut Host` and an effect sink.
  The gesture layer translates the host's effects into its own rather than
  sharing a vector: what a binding's apply may legitimately produce is a
  redraw, and anything else (a window opening behind a knob turn) is a bug worth
  seeing in the log.
- **A multi-value payload rides as its JSON string** — the scalar carrier the
  array-valued props (`points`, `notes`) already accept from `/gui_set` — so one
  curve drives another curve without a second encoding being invented for the
  binding path.

## A widget type names a container, an element or a control — nothing narrower

*(decided while landing the vocabulary migration)*

The `/gui_*` catalog had grown to twenty-nine type names over a model of three
things, and the same idea was spelled several ways: `waveform`, `plot`,
`scope`, `spectrum`, `spectrogram` and `phasescope` were six points of one
product (a presentation × a source kind × the capabilities over it); `panel`,
`box` and `stack` were one container with three arrangements; `scroll` and
`patch` one plane with and without boxes; `track`, `clip` and `timeruler` one
time/value container told apart by what is placed on it. So the wire now names
the **model**: a container owning 0, 1 or 2 axes (`window`, `layout`, `plane`,
`field`), an element drawn against them (`signal`, `notes`, `curve`, `score`,
`keys`, `nodes`, `meter`, `canvas`, `label`), and a control, which is an element
with a value and no axis (unchanged — a `knob` names what it is).

**Why the wire and not a set of presets over it.** Keeping the old names as
wire names would have cost no migration at all, and was rejected: the reference
would document twenty-nine types over a model of six concepts, and a reader
would never discover that a plot is a trace that does not navigate, because
"plot" reads as a different thing. The model's whole value is that the general
form is what a script learns; hiding it behind the names defeats the refactor.
That argument is about the **wire**. It does not reach a client's own builder
names, which is why `waveform()` and `track()` survive as shortcuts onto the
model — a shortcut re-documents nothing, and every existing call site kept
working through the migration because of them.

**The chrome belongs to the axes, not to each element.** A ruler, a navigation
window, a selection, a playhead and a value range describe the container's axes;
they were props of every view that drew against them. They now ride nested,
under `axes: {x, y}` — nested rather than bare `x`/`y` because those are already
the free-placement props, and a container that is placed *and* owns axes would
have no way to say which it meant. Inside the host they stay flat, and a def's
pair is flattened at the door: an OSC reply is flat arguments, so a structural
prop cannot be answered at all, and nesting them would have made `/gui_query`
stop reporting the ruler, the window and the range a script reads back.

**What the discriminations cost.** Collapsing names means the props have to say
which construction is meant, and three of those rules were only settled by
writing them:

- `navigable: 0` over addressable samples is the whole static-plot
  construction, not a capability switched off: a view that does not navigate
  also holds the sequence itself rather than a take (no peak pyramid) and
  auto-fits a value axis nobody named.
- A `field` with nothing on it is a **lane**, not a ruler. The first rule said
  "children, or nothing at all" — but a multitrack opens empty lanes constantly,
  so the ruler is the case that must declare itself: a bare strip of a given
  thickness `h`, nothing placed on it, no lane chrome.
- The arrangement is `flow`, on every container that has one, because the model
  spends the word `layout` on the container type itself.

**What it does not reach, on purpose.** `box` is the one name the migration
could not take: the catalog spent it on a synonym of `panel`, the model wants it
for a patcher's box, and turning a plane's `boxes` prop into child elements
changes behavior (ids, layout, per-box hit-testing and edit-back), not spelling.
The name is now unclaimed and the prop unchanged, waiting for the element rather
than for a rename.

## A frequency axis is navigated by the element, not by a group

*(decided while making `navigable` mean something over a spectrum)*

Retention had just made a live *time* axis navigable, and the acceptance line it
came with also asked for a zoomable live **spectrum** — which turned out not to
be the same mechanism at all. Everything navigable in the host navigates time: a
navigation group is keyed by `link` over a shared time axis, the group's window
is measured in samples, and joining one is what `is_timeline` answers. A
spectrum's x is **frequency**, a domain that is addressable with no retention
whatsoever — every bin is there every frame — so the capability parsed and had
nowhere to go, and the code said so honestly by dropping it.

Two shapes were open. A **second axis kind** beside the time one (a coordinate
system carrying its unit, so frequency axes group with frequency axes and never
with a lane) generalizes to a pair of spectra comparing two buses, and to an
arbitrary-domain plot later. Or the **element's own window**, no group at all,
in the shape the vertical axis already has: a normalized `[start, len]` the
element carries alone.

**It is the element's own window**, and the argument that settled it is not the
size of the diff. *Frequency is already navigated this way in this host.* The
spectrogram's frequency axis is its vertical one, and that axis is a normalized
per-element window with no group behind it — because a display axis over
`[0, Nyquist]` on a log/mel/bark scale is a display coordinate, inverted through
one shared mapping, and nothing else in a window measures in hertz to share it
with. Making the curve's frequency axis a *grouped* axis would have given one
domain two mechanisms depending on which way it happened to be drawn, and left
the reader unable to guess which. So the horizontal window is the vertical one's
sibling in every respect: same normalized units, same clamp, same read-at-use
validation, and a `"view_x"` event beside `"view_y"`.

**What it costs, if the pair of spectra ever wants a group.** Nothing that is
lost: the per-element window remains where the answer is stored, and a group
would be a layer that writes it, exactly as a timeline group writes its members'
view today. That is the reason this order is safe — the general case can still
arrive, and it will not have to undo this.

Two smaller things fell out of it, both worth stating because they are the kind
that surprise the next reader:

- **The wire keys are the x axis' own `view_start`/`view_len`** (`axes.x.start`
  / `len`) rather than a new pair. It is the same question a timeline member's
  window answers — what slice of the x axis is visible — and only the owner of
  the answer differs. Exactly one reading is live for a given widget: on a
  member of a navigation group those keys never reach the element at all, the
  group model takes them, in samples.
- **`navigable` is off by default here, and only here.** Everywhere else the
  prop defaults to on, because the views the catalog grew as editors navigate.
  A spectrum is a meter-like view that has never moved under a drag, and every
  existing `spectrum` on the wire says nothing about the capability — so
  defaulting it on would have changed what shipped defs do, to no one's
  request.

## A navigable axis stops at the resolution of what it measures

*2026-08-10, from a by-eye pass: zooming a live spectrum far enough left the
widget showing a flat line that no longer answered to the sound.*

The display axes have always been floored by a constant fraction of their own
extent (`viewport::MIN_SPAN`), which is a number about the **screen**: it says
how small a window may get before a view stops being drawable. That is the right
question for a plane you pan, and the wrong one for an axis over a **measured**
domain, which has a resolution of its own. A spectrum's x is frequency, and its
frequency is an FFT of a given size: at `fft_size` 2048 and 48 kHz a bin is
23.4 Hz, and the constant floor let a window shrink to a *fifth* of one — the
whole widget then drawing the interpolation between two neighbouring bins, a
straight line that no zoom, and no signal, will ever change.

**The floor is derived from the analysis**, not from the display: the window may
not go below the display width of a handful of bins. It cannot be a constant,
because a bin is not one — on a log axis a bin is a twentieth of the visible
axis at 500 Hz and a thousandth of it near Nyquist, so any fixed fraction is far
too coarse at the bottom and far too fine at the top. It is measured through the
very display↔hertz mapping the curve and the ruler are drawn with, which is also
what keeps the three from disagreeing.

Two consequences worth stating:

- **The floor belongs to the axis, not to the gesture.** No path can put the
  axis somewhere another path forbids: the wheel, the drag, the `R` reset and a
  script's `/gui_set` all land inside the same bound. It is applied where the
  vertical window's own clamping already lives — at the **read**, not in
  `apply` — which is also why one `/gui_set` carrying both keys cannot depend on
  their order. The *zoom* is the one place that needs it as a number beforehand:
  clamping afterwards would keep the anchor a narrower window computed, so each
  further step at the bottom would slide the picture sideways instead of
  standing still.
- **What is stored is the request; what is shown is the request opened up.** The
  floor is a function of where the window sits — at 12 kHz four bins are a
  thousandth of a log axis, at 20 Hz they are a quarter of it — so a window that
  exists up the axis cannot exist down it. Writing the opening back would make a
  *pan* spend the zoom: the way down would widen the window, and the way back up
  would arrive somewhere the reader never asked to be, one gesture no longer
  undoing itself. So the pair on the element is what was last asked for, from a
  gesture or from the wire alike, and everything that looks at the axis — the
  frame, the gesture's anchor, the `"view_x"` event — asks it for the window it
  can actually show. The two agree everywhere but at the bottom of the axis,
  which is the whole point. A corollary: a request that would show exactly what
  is already on the screen is not written down, so the wheel at the floor does
  not quietly discard the zoom the reader is still going to want back.
- **The gesture needs the sample rate**, which the fronts knew and the gesture
  context did not. It is one field on `GestureCtx`, filled from the same source
  the frame draws with — because a gesture that resolved a different hertz than
  the frame drew would anchor a zoom where the reader is not pointing.
- **A floor read off the axis is not a floor.** Because it is measured forward
  from the window's left edge, the floor is a function of where the window is —
  and a pan hands over an edge that is *off* the axis, which is exactly what
  dragging past the end means, the write clamping it a step later. Measured from
  there, the overshoot is charged to the floor: the window comes back widened by
  how far the drag went, the next step of the same drag reads that wider window
  and goes further still, and a gesture asking to move sideways rushes the
  picture out to the whole axis. The edge is clamped onto the axis before the
  bins are counted from it.

**And a gesture that moves nothing says nothing.** The same pass found the other
half: an axis pressed against a bound goes on receiving wheel notches, and every
one of them re-emitted the window it already had. The view events (`"view"`,
`"view_x"`, `"view_y"`) now report movement rather than input. The comparison
carries a small epsilon, and that is not a fudge: a bound that is itself a
function of the window's position converges to it by last bits rather than
landing on it, and a billionth of a normalized axis is a millionth of a pixel.

## The Rust builder is generic, so it never becomes a fourth widget catalog

*2026-08-10, opening the Rust door into the host (`clausters_gui::tree`).*

A program that links the crate needed a way to build a widget tree without
serializing a JSON document against itself and parsing it straight back, and the
obvious shape for it — one typed constructor per widget, `knob(min, max)`,
`meter(bus)` — is the shape the Python builder already has and the one to
refuse here.

Every widget's props are declared in **three** surfaces today (the host, the
Python builder, the web builder), and `docs/gui-props.md` plus
`clients/python/tests/test_gui_props.py` exist precisely because three surfaces
drift: the test reads all of them and fails on a divergence nobody declared. A
typed Rust mirror would be a fourth, with no test holding it and no client
asking for it — the Python client is the reference surface for the catalog by
the project's own rule, and the reader who wants a knob's props reads its page,
not a Rust signature.

The second reason is the one that makes it structural rather than a matter of
taste: a **registered element** (the `Element` trait) has props no catalog
inside this crate can know, and instantiating one is a first-class use of the
builder. So the open door — a node named by its wire type, props set by key —
has to exist whatever else is built on top of it. Once it does, a typed twin of
the catalog checks spelling rather than safety, at the cost of a surface that
rots.

What *is* typed is what a hand-written document actually loses: a prop is a Rust
value, so an integer stays an integer and a continuous value stays continuous —
the int/float distinction the host reads props by, kept by construction instead
of by remembering to type `2.0`.

## The keyboard focus can leave the tree, because a GuiDef is not always the whole screen

*2026-08-10, generalizing the host's focus from "the focused text field" to a
tab ring (`host/gestures/focus.rs`).*

Every toolkit's tab ring **wraps**: Tab off the last control returns to the
first, because the window is the world. That is true of a desktop window and
false of the host's other target, where a GuiDef is mounted **inside a
document** — a `<canvas>` among the page's own headings, links and forms. A
canvas must carry a `tabindex` to receive keys at all, and the host's front
prevents the browser's default on every key it handles, so a wrapping ring there
would be a **keyboard trap**: once Tab entered the canvas, nothing else on the
page could ever be reached again. That is a worse regression than shipping no
keyboard at all, and it would land on the page author rather than on us.

So the ring **runs out** instead of wrapping. Tab past its last stop clears the
focus and reports it to the front (`GestureEffect::FocusOut`), which is the one
thing the two platforms answer differently: the browser shell blurs its canvas
and the document's own tab order carries on from there, while a desktop window
— having nothing outside to hand the keyboard to — simply has nothing focused,
and the next Tab enters the ring again. The core decides *that* focus left; only
the shell knows what that means locally, which is the same seam every other
platform difference in this crate goes through.

**The desktop does not wrap**, and that was asked and answered rather than
overlooked: the shell could perfectly well re-enter the ring on `FocusOut`, and
every desktop toolkit does. It does not, because the platform that *cannot*
wrap sets the rule for both — a window and a canvas behave the same way, so
there is one behaviour to learn, one to document and one to test, rather than a
divergence whose only reward is saving a keystroke on one of the two targets.
The empty step is not a hole either: "nothing is focused" is a state the user
reaches anyway by clicking on empty space, and seeing the ring go is what says
the keyboard left.

Two smaller calls follow it and are recorded here so they are not reopened.
**`focus` is not a prop.** It rides on `/gui_set` because that is the wire's one
mutation verb, but it says where the keyboard is pointing — host state, not a
widget's — so it is taken out before the document is written: a `/gui_query`
does not report it, and reloading a persisted def does not restore a focus
nobody asked for. It is also not echoed: the `"focus"` event reports what the
*user* did, the way every other event on that stream does, and a `/gui_set` has
never announced itself back to the script that sent it. And **composition (IME) stays the page's**: a canvas cannot
host an input method, so the host reads the keys it is handed and never pretends
to compose them. Text that needs composing is not entered through a host field,
which is a stated limit rather than a bug to find.

## The crate embeds no typeface, because a face is licensed, heavy and already installed

> **Reversed 2026-10-05** -- see "A host draws with a face it carries, and
> antialiased", at the end. What follows is the record of the first decision.

A GUI host built with a rasterizer draws with a face, and the obvious way to
ship one is to embed it: `include_bytes!`, one file, nothing to configure. That
is not what happened, and the reason is worth keeping so the question is not
reopened as an oversight.

A face is **three costs at once**. It is hundreds of kilobytes in a repository
that carries no other binary asset, in a wasm bundle whose whole argument is
that a page may hold forty canvases, and in a wheel that already carries a
JIT compiler. It carries a **license of its own** — its own attribution and
redistribution terms — inside a GPL crate, which is a bookkeeping obligation for
every downstream that copies the artifact. And it answers a question the
platform has already answered: every desktop has faces installed, and every page
can fetch one from the origin it was served from.

So the face arrives through a **seam**, the fifth platform trait
(`FontSource`): a file natively (`--font <path>`, `[gui] font`, or one of the
system's own when neither names one) and fetched bytes in the page
(`GuiBridge::font`). What the feature compiles in is the *rasterizer*, not a
typeface.

**And no face is an ordinary state, not a failure.** The embedded 5x7 bitmap is
the floor this crate always draws on, so a host with the feature and nothing
loaded renders exactly what a host built without it renders — which is the same
degradation rule the protocol already has for a widget an older host does not
know. That is what makes the runtime half of the seam safe to leave optional:
there is no error path to design, no boot to block on a font, and a machine with
no fonts at all still draws every label.

## The cell is declared and the face is fitted to it, in both faces

The bitmap face landed under one rule — the layout declares a cell
(`metrics::CELL`) and the glyphs are drawn to fit it, never the reverse — and
the atlas keeps it rather than inheriting a typeface's own metrics. A scale
rasterizes at the pixel size whose **cap height** is the body box, so a capital
through the atlas is exactly as tall as the capital the bitmap drew.

The property it delivers is the one the sizing model paid for: the metrics table
is constant data resolved once per scale change, so **changing the typeface
never relayouts a window**. Loading a face mid-session is a redraw, not a
re-measure; a document laid out by a host with a face and one without is the
same document.

What legitimately *does* follow the face is the width of a **string**, and it is
asked for where a string changes — a label's text, a field's line, a wrapped
paragraph — never in a layout pass. That is the seam the bitmap face already
left when it separated its own nominal advance (a property of the face, which
the size roles that reserve room for N characters ask for) from the measurement
of a run of characters. Making that measurement proportional was the whole of
the port: the caret, the selection band, the click-to-column hit-test, the
ellipsis cut and the word wrap were all counting cells, and each of them now
measures the glyphs it is actually drawing — which is the same arithmetic under
a fixed-pitch face, and correct under either.

One consequence is deliberately visible and is the **only** place two builds of
this host differ: `text_size` quantizes to half-steps of the cell without the
feature, because a bitmap glyph is scaled by repeating its own pixels and an
uneven scale makes them ragged, and is continuous with it, because an outline is
rasterized at whatever size is asked for. A user who wants larger type still
asks for it where density lives — the `scale` key of `[gui.metrics]`, resolved
once — never by choosing a different face.

## A size may read a prop, never a value — and only where a container asked

`hug` lets a container want what it holds instead of the share the layout would give it, which reopens the question L7 closed by fiat: *may a size read the widget's content?* Its answer was "never" — a scope's height must not follow its sample count, a label's width must not follow its string — and it was right about the failure it was avoiding and too coarse about the reason.

The line that actually holds is **where the value is resolved**, not what it is. A prop settles at a *mutation point* — a `/gui_def`, a `/gui_set` — which is exactly where the theme overlay already resolves and where a relayout costs one message. A **value** streams: a bound knob writes one per pointer move, a meter one per frame, a field one per keystroke. So a menu's `options` may size it and the option it is *on* may not; a label's `text` may and a `number`'s reading may not. The failure L7 was avoiding is entirely on the value side: a window that jumps while a control is being turned, and a per-message relayout cost. Sizing to a caption has neither.

Two mechanisms keep that honest rather than a convention. The question is asked by a **different function** (`Element::hug`, beside `natural`), so the ordinary layout pass cannot read content even by accident, and every element that does not implement it answers its data-free natural size. And it is only ever called **under a container carrying `hug`** — a container that has asked for its size to follow what it holds. A def that does not use the prop lays out byte-for-byte as it did, which is why this could land without the one-time break L7 needed.

The composition itself stays inside the boundary L2 fixed and L7 restated: one bottom-up walk over functions that were already pure, no measurement pass, no constraint solver, no negotiation between a parent and a child. `None` (elastic) propagates, so a container holding a plane or a heavy view still hands the axis back rather than inventing one — a hugging container is not a promise that every subtree can be measured.

The one asymmetry between the two builds is the **window** case, and it is a platform truth rather than a fork: `hug` on a root sizes the OS window to its content, and in a page there is no window to size — the element owns its box and reports its pixels, which is the rule that keeps the host from ever reading the DOM. The composition inside is identical on both; only the outermost rectangle has a different owner.

## The paint capabilities are the ones that keep the batch — and antialiasing is not a widget's

The host's chrome is one batch of flat triangles: everything a window draws goes into one vertex list and one draw call, and that is the crate's whole performance argument. It is also why the chrome had no rounded corners, no fades and hard edges, which is right for an oscilloscope and an absolute ceiling for application chrome. Widening it therefore had one criterion, and only one: **what can we add that does not split the batch?**

Three things could. Rounded corners are *geometry* — the arcs tessellate into the same mesh, the way the score's outlines already do. Opacity is *already there* — the vertices carry RGBA, so a fade needs a prop and a resolution rule, not a pipeline. Antialiasing is *the attachment* — MSAA is a property of the render pass, so it changes no drawing code at all. What stayed out is what would split it, and is recorded so it is not re-argued: textures beyond the glyph atlas, gradients, drop shadows, and per-layer compositing.

**Both props ride the mesh, not the draw functions.** They apply to a *run* of triangles — everything one widget contributes — exactly as the clip rectangle does, so the frame sets one `Ink` per placement and no draw function grows a parameter, no element is told it is being faded, and every element written before this milestone honours both without knowing they exist. The corner radius reaches only the **box** primitive (`Mesh::rect` and the border built from it), never a line, a disc or a glyph, and each box clamps it to half its shorter side — which is what lets one number ride a whole widget: a divider, a tick and a track edge have no room for an arc and come out unchanged, while the widget's own frame rounds.

**The two inherit differently, and that is the design.** Opacity composes down the subtree like a theme group — a control at 0.5 inside a panel at 0.5 draws at 0.25 — because a fade is a statement about a *group* of widgets ("this pane is inactive"). A radius does not inherit at all: a rounded panel says nothing about the controls in it, since rounding is a statement about one box's shape. Both settle at a mutation point, never on a value, so the rule "a size may read a prop, never a value" has a sibling here: a *stream* of values can no more fade a window than it can resize one.

The fade's bound is worth stating rather than discovering: it is **per-primitive alpha, not layer compositing**. Two overlapping shapes inside a faded widget show through each other, because compositing a subtree means rendering it to a second target — which is exactly the batch we refused to split. And it fades what the mesh carries: the chrome, the controls and the text. A heavy view's picture is drawn by its own pipeline against its own uniforms and keeps its opacity; fading it would be a uniform on the waveform, the spectrogram and every user `canvas` shader, i.e. three pipelines paying for one prop.

**Antialiasing is a host setting and not a prop**, and that follows from the same fact rather than from taste: it is the pass's attachment that is multisampled, not a widget. Offering it per widget would promise something the hardware does not do at that granularity — a widget cannot have its own sample count without its own pass — so it is one number per window (`--msaa`, `[gui] msaa`, `GuiBridge.msaa`), clamped at bring-up to what the adapter reports for the surface format, falling back to 1 with a warning rather than refusing to open. The default is 1: a signal trace is drawn at the resolution of the screen on purpose, and smoothing is what a chrome-heavy document opts into.

## A waveform column is its own envelope: there is no fill, and no zoom at which one belongs

Three renderers drew a signal against time — the GPU pipeline of a navigable view, the shared mesh (a plot, a clip's take) and the live oscilloscope's own loop — and they agreed on the columns and on nothing else. The most visible disagreement: the GPU path clamped every column to include zero (`lo.min(0.0)`, `hi.max(0.0)`) and the two mesh paths did not, so a signal offset from zero was a body filled from the baseline in one view and a floating band in another of the same samples.

The first attempt closed it the other way — one shared rule, "fill when the value domain straddles zero" — and an eye pass killed it in one screenshot. At a zoom where cycles are legible a column covers two or three samples that all sit near +0.6, and filling to zero **inks a band the signal was never in**. So the question became *at which zoom does a fill belong*, and that question has no answer: a **subsonic** signal — a 1 Hz LFO, a control curve, a long envelope — has far more samples than the screen has pixels at any zoom showing a whole cycle, so every "fill once the samples stop fitting" rule fills it, and a cycle a second is precisely a curve that a fill destroys. `samples_per_px` is not the criterion, and neither is anything else about the zoom.

What separates a body from a curve is whether the signal **crosses the span inside one column** — and the min/max already answers that, measured, per column, at no cost. So the rule is that there is no rule: **a column is drawn as it is measured, everywhere.** The solid body of a zoomed-out waveform is the data filling it (audio crosses zero many times per column at overview zoom), the curve of a slow signal is the data not filling it, and the drawing never changes its mind about which it is looking at. That also removes a threshold nobody could have named and a prop nobody has to learn.

Two floors keep it legible, and they are the mesh renderer's rules applied to the pipeline that had neither. A column is inked **at least one pixel in each direction**, so a flat stretch — the sustain of an envelope, the tail of a decay — stays visible instead of collapsing to nothing. And once the zoom is deep enough that consecutive samples stand **three `point_radius` apart**, each sample is **marked with a dot**: the polyline between samples is an interpolation the drawing invents, and the dot is what says which points of it are data. The dot is sized as a curve's break-point deliberately — it is the affordance sample-level editing will take hold of, so the two read as the same kind of target the day the second one becomes draggable.

## `min`/`max` are one value domain, and the amplitude axis is its default

The same audit found the navigable trace ignoring `min`/`max` outright: its geometry was pinned to ±1 through `AMP_MARGIN` while the take in a clip, the plot and the live scope all mapped through the declared pair. One prop meaning something in four of an element's presentations and nothing in the fifth is the per-arm divergence the E track exists to remove, so the pipeline takes the domain now, `[-1, 1]` by default — which is what every view that names none draws at, byte for byte.

The consequence is on the **ruler**, and it is the one policy call here. `db`, `bits` and `percent` are units of *full scale* — a rung at -6 dB or at 2^15 says nothing over `[20, 20000]` — so the amplitude ladders belong to the default domain, and an axis whose element named a domain of its own is ruled as a plain **value** axis over the slice its window shows. Stated the other way round: **the amplitude axis *is* the full-scale domain**, not a separate mechanism, which is why the switch is a comparison against the default and needs no flag on the wire.

## A signal against time is drawn in one place, and the waveform's pipeline was the second

The navigable waveform used to build its own vertex buffer through a dedicated
`wgpu` pipeline (a triangle list for min/max columns, a line strip for the raw
polyline, one WGSL module), while every other drawing of a signal — a clip's
take, a plot's series, a meter's history — went through the shared triangle mesh
in `host::graphics::signal::trace`. Two implementations of one picture, and the
measurement that had once justified keeping them (`tests/gpu_slot_cost.rs`) said
the opposite of what it was read to say: the per-column *computation* — the
peak-pyramid reads — dominates so completely that where the vertices land is
noise. The pipeline was not paying for itself; it was only a second place to
write the same thing.

So it drifted, in the ways duplicated arithmetic drifts. The pipeline's polyline
took the sample before the window's left edge but not the one past its right
edge, so at the deepest zoom a trace arrived at a sample and **stopped dead**
where the data continued — visible the moment someone zoomed a bulk file in far
enough. It marked samples with squares where the mesh marks discs. It split the
two regimes on the opposite side of the threshold (`<=` against `<`), and floored
a column's ink at one physical pixel where the mesh floors it at the trace's
weight. Each of those is a one-line fix and none of them stays fixed.

The pipeline is gone. `WaveformData` is data (raw samples plus a peak pyramid per
channel) and `WaveformView` is the vertical navigation state of a view over it;
the picture is `trace::draw_channel` into the window's mesh, exactly like every
widget's. What was lost with it is worth naming: a waveform no longer draws
through `set_viewport`, so the `Framing` trick that cuts an off-screen lane at a
fixed size is the spectrogram's alone — the mesh's clip rectangle does that job
for triangles, and does it per widget rather than per pass.

The spectrogram keeps its pipeline, and the difference is the load-bearing one:
its picture is a **texture sampled once per pixel**, so the GPU's own filtering
is what resolves it at any zoom. That is work triangles cannot do. A waveform's
columns were folded on the CPU either way.

## A drawing that overshoots its rect is the one that bounds it

A clip masks what it holds, and that is right: a clip is a coordinate system,
and one that does not bound its contents is a rectangle they happen to start in.
But the mask was put there to stop a specific overshoot — the trace reads the
sample before its left edge and the one after its right, or the line would start
and end inside the box — and that overshoot belongs to the *trace*, not to the
clip. So it came back the moment the same drawing stood on its own: a
free-standing `waveform` is nobody's content, no container mask is in force, and
the sample discs landed on the vertical ruler beside the view.

Bounding it per container is a rule every future holder of a trace has to
re-learn. So `trace::draw_channel` narrows the mesh's clip to the lane it was
handed and restores what it found — narrowing only, so a clip's mask and a
scroll's still hold — and every destination is bounded at once, on both axes:
the vertical overflow of a value outside the amplitude window was relying on the
same absent mask.

The cost is small and worth naming: a column at either end of a rect is widened
to the trace's weight like every other, and now the lane shaves that widening
instead of letting it hang over the edge. That is the same treatment a value
just outside the vertical window gets, which is what makes it consistent rather
than a special case.

## A hit-test is the shape that was drawn, and it compares squared distances

The GUI host answered every pointer question with `Rect::contains`, because
nearly everything it draws is a rectangle or a stroke. The exceptions read as
their bounding box, which is not a small error: the corner of a square around a
circle is a quarter of the box that never belonged to the shape, and at the
radius a knob's dial or a notehead is actually drawn at, that quarter is most of
where the pointer tends to be. A knob's cell holds a label strip over the dial
over a read-out, so a press on the name or the number turned the value; a
notehead's box, on a dense page, overlaps the stem, the beam and the note a
line away.

`host::graphics::shape` holds the round shapes — the disc, the ellipse
inscribed in a box, and the squared distance both are tested with. It is one
module rather than a helper per model because the round things are spread
across the catalog and each had either reinvented the arithmetic or skipped it.

**Squared, never rooted.** A distance in a hit-test is compared and never
reported: against a radius, and against the best candidate so far. Squaring
preserves the order of non-negative numbers, so both comparisons answer off
`dx² + dy²` and the square root is work with no reader. The ellipse is the same
test with each axis divided by its own radius before squaring.

The rule is wider than the circles that exposed it: **a hit-test is the shape
that was drawn**. A slider's track area is its cell minus the label and the
read-out, and what is drawn in it is a groove a few pixels thick with a short
grip riding it; a toggle stretched across a row is a small box with a word
beside it and a great deal of air after that. Both were acting on presses that
landed on blank space the layout had left around them.

So the filter belongs to the **trait**, not to each leaf. `Element::hit_area`
returns the shape the element answers on — the placement rectangle by default,
a disc or an ellipse otherwise — and `gestures::element::press` is the one place
it is applied, adding the metrics' hit slop so no element decides for itself how
much air a small target deserves. A point off the shape reads exactly as a
decline, so the press falls back to the chain like any other. Declared rather
than tested is what keeps it general: adding a leaf drawn smaller than its cell
is one method, not three lines every leaf must remember, and a leaf that says
nothing keeps the rectangle it always had.

What that leaves is a pass rather than a design: each remaining light widget
states its drawn shape, and each is checked by eye on the example that shows it.
Two questions belong to that pass and not to this record — whether hover and the
wheel should read the same shape as the press (they still read the placement
rectangle), and what a hugging container should do when its child answers on
less than it was given.

The one route that skips the filter is the **overlay**: a modal is offered the
press because it is outside as often as because it is inside — clicking the
window is how a menu's list closes — so "is this widget's drawing under the
pointer" is the tree's question, not a modal's.

Two consequences worth stating, because they are what makes the pattern
generalize. The shape is carried **per hit entry**, not decided at the test: the
score's index measures extents, so everything arrives as a box, and only the
indexer knows that this one was measured around a notehead (a codepoint in
SMuFL's Noteheads range) rather than around a beam. And an element whose press
lands outside its own shape **declines**, which hands the press back to the
chain — the pixels beside a dial go on meaning whatever the window means by
them, rather than becoming dead space around every round control.

## An intent states the whole value, so absence is a value

The rule the document is built on has an edge nothing had written down, and
three independent implementations got it wrong in the same way. An intent is
**absolute**: it states the value the edit results in. A `place` therefore
states the *whole* placement, and a placement that carries no `dur` is a
placement with **no length** — the member takes the element's own. That is not
a shorthand for "leave the length as it is", and the difference only becomes
visible in an inverse: the undo of the *first* resize of a clip has no `dur` to
carry, because before that resize there was none.

Every projection of an intent onto instantiated data read that absence as
"unchanged", because that is what the convenience method underneath does —
`Aggregate.move(member, offset, dur=None)` leaves the length alone, and so did
the host's own adoption (`if let Some(dur) = dur`). The result was a document
that stepped back correctly while the picture and the objects the script holds
kept the size the hand had given them: `undo` answers *true*, the log is right,
and the clip does not move. It reads as a dead button, and it was reported as
one.

So the rule is stated as a rule, and it holds wherever an intent is written
onto something instantiated — the arrangement objects a client holds, the
widgets a host draws, a leaf's configuration:

- **A field the intent does not carry is that field's absence, and absence is
  written.** A `place` with no `dur` writes *no length*; a member whose node
  carries no `config` is a leaf configured as it was made, so the empty table is
  written rather than skipped (an undone trim that skipped it left the window
  over the samples where the trim had put it, under a clip that had gone back to
  its old size).
- **What a picture shows is one rule, in one place.** The length a clip is drawn
  at — the placement's, else the element's own, else the samples', else a beat —
  is asked by the draw *and* by every path that puts a placement back. Two
  implementations of it is how a picture and a model come to disagree without
  anything failing.
- **An intent over an aggregate moves every member it states.** A trim, a split
  and a join are one `setmembers` over a lane, and answering with only the lane's
  own widget left each clip drawn as the hand had left it.

The samples are the same rule seen from further down, and they already followed
it: a destructive edit's inverse carries the values it painted over, and the
host replays them onto the buffer it drew from. That is why an undone stroke has
always come back while an undone resize did not — not because samples are
special, but because a payload of values has no way to express "unchanged" and a
placement did.

## A placement is a prop; a widget that was not there is a redefine

The acknowledgement can carry any *value* a widget already has — a placement, a
length, a curve, a note list, the window over a take's samples — and that is the
cheap channel: the host adopts it without rebuilding anything and without losing
what it has in flight. It cannot carry a widget. So a **structural** edit — a
split, a join, a cut, a paste, a cord drawn in a patcher: anything that changes
*which members exist* — has exactly one channel, and it is a whole-tree
`/gui_def`.

Who owes it is the editor that drew the window. The host emits the verb and
holds nothing ("it answers with the tree that now stands, exactly as it answers
a drag"), so an owner that only applied the intent left the document and the
objects the script holds with two clips while the picture had one, until
something happened to redraw. Nothing did: no example calls `update()`, and
none should have to.

The rule is therefore *both* halves, and the second is what keeps it honest:

- **A structural edit redefines**, forward and backward alike. An undo of a
  split takes a clip away, which is as unsayable in props as adding one.
- **A placement edit does not.** A redefine rebuilds every widget and drops what
  the host had in flight, which is exactly wrong for a drag and would turn every
  frame of one into a new tree.

Two things this pulled in with it, both of which had been quietly wrong:

**A redefine moves the version, and the document has to move with it.** The
crate refuses an edit whose `against` version is not the document's — ahead of
it as loudly as behind, since the two would then not be talking about the same
piece — so a version bumped on the client's side alone answered every gesture
after a redefine with a refusal nobody asked for, and with no reason to show:
the clip simply stopped moving. The redefine now re-derives the document at the
version it is drawing, which is what `refresh` already did for the case where a
script edits the tree.

**The node index is added to, not replaced.** An element that has left the tree
is still named by the inverses in the log, and putting it back means placing
*that object* again — a rebuilt one is a different identity to every widget and
every pending edit. The Python client kept stale entries and worked; the port
cleared the map on each re-derivation and lost the restore, so an undo of a cut
and a redo of a split quietly did nothing. One of those was a rule and the other
was a habit, and finding out which is the whole of what a port is for.

## The acknowledgement is a stamped state push, not a reply code

A GUI host holds no data, so every edit it produces is a **proposal**. Between
the gesture and the answer there is a gap the host has to draw across, and the
shape of the answer is what decides how much machinery that costs.

The answer is: the owner pushes the state that now holds, stamped with the
sequence number of the last edit it processed. That collapses three outcomes
into one message — *applied verbatim* (the value equals what the host drew),
*applied transformed* (the value is the effective one, post-snap or post-clamp)
and *refused* (the value is the previous one, unchanged). The host needs no
branch for them: its whole rule is **drop every pending edit at or below the
stamp, and adopt what arrived**. A refusal needs no error path either, because
"the state after your gesture is the state you already had" is a state push like
any other. It is Ardour's `rdiff()` seen over a wire: what reports the change is
the model, never the hand.

Three things follow, and each was a decision.

**The acknowledgement is sent always**, including when nothing changed. Silence
is not a refusal; it is a hang.

**It is a verb (`/gui_ack`) rather than a widget property.** The project's rule
against new `/gui_*` addresses exists to stop a widget or a prop from becoming
an address, and its escape hatch is a new payload on `/gui_event` — which is the
*host-to-client* direction. This is the other one, and it is the reply
`/gui_event` never had. It cannot be a property because it is scoped to the
conversation rather than to the tree: the sequence is per client, so two clients
driving one window would collide on a single prop, and it does not round-trip,
which a property has to.

**Intents are absolute, which is what makes replay unnecessary.** An edit states
the resulting value (`"note" id pitch`), never the increment. A pending edit
simply stays drawn over whatever authoritative state arrives, with nothing to
recompute, and a resend over a lossy leg is harmless. The rejected alternative
is worse than it looks: relative intents must be rebased against a corrected
state, and rebasing is exactly the netcode replay that would require the host to
hold an executable copy of the document.

**The answers lag, and staleness is measured against a floor rather than
against the lag.** The host stamps every event with the version it was last
*told*, and it is told only when an acknowledgement arrives — so on any carrier
an event naming a version the owner has already moved past is the ordinary case,
not a collision. The page's is the extreme: the host's outbox is drained on a
33 ms interval and the answer goes back through the event loop's proxy, so a
round trip is two queues wide and a hand crosses it easily. Making the ack path
same-turn is not available on either platform (native is a socket, the page is a
queue in each direction), and the alternative — telling a host that is behind to
stop emitting — trades a refused edit for a stalled hand, which is the wrong way
round for a gesture that has to track the pointer. So the mechanism is built to
be correct with the answers arbitrarily late: the owner keeps a **floor**, the
version at which the document last moved by a route no event produced (a script,
a second editor, a re-derivation, a history step), and refuses only an edit
naming something below it. Every other older version is one of the owner's own
answers in flight. The earlier rule — *a run of edits from one widget is one
gesture* — was this same insight scoped too narrowly: it saved the drag it was
written for and still refused the next gesture whenever two began inside one
round trip.

What this replaces is worth recording, because it is the honest reason the
acknowledgement never seemed necessary. Consistency was being maintained by
**duplicating the owner's rule in the view**: the lane's snap grid travels in
the GuiDef and the same snap is implemented on both sides, so the round trip
closed because both sides did the same arithmetic. That works for a grid and
generalizes to nothing — a bus allocation, a normalize, a user-written function
cannot be shipped down — which is why the places it already failed were so
quiet.

## A selection's second axis belongs to the view that measures it

A marquee swept with height over a waveform selects a **band of amplitudes** as
well as a span of time. The span goes to the navigation group, where selections
have always lived; the band does not, and where it goes is the decision.

**The group is one axis, and it is the time one.** What makes a linked view
linked is that a waveform lane and a spectrogram lane show the same stretch of
the same sound — one window, one selection, one playhead. Vertically they show
nothing in common: one measures amplitude over a domain the element declared,
the other frequency over the Nyquist, a piano roll semitones. A range held in
the group would therefore have to mean four things at once, and the first
concrete consequence is absurd — a sweep between −0.5 and 0.25 on a waveform
would restrict the spectrogram beside it to a quarter of a hertz.

So the range lives on the **widget**, beside its vertical window, and the event
that reports it already names which widget it came from. That is the same split
the piano roll made when its marquee was built: the time span left as the
group's, the notes it enclosed stayed the roll's own state.

**Rounding follows what the axis measures, not what the gesture did.** The time
axis snaps to whole samples because a selection between two samples holds no
data — it can be neither played nor cut — and the snap takes the samples the
sweep *passed over* rather than the nearest, so a sample joins when the cursor
reaches it. A value axis has no such grid: every height inside a lane is a value
the signal can take, so the range is what the hand drew, ordered and clamped to
the element's domain. The rule that covers both is *snap to the axis' quantum
where it has one, clamp to its domain always* — and the quantum is not
hypothetical, because a roll's pitch axis has one: its band is the whole
semitones the sweep covered, both ends included.

**A spectral view's second axis is not this one.** Frequency bins are a separate
field of the selection, deliberately, because an operation that understands a
band of values need not understand a region of frames × bins — and a band drawn
in hertz that some reader took for amplitudes is exactly the confusion the
separate fields prevent. A time-frequency view therefore reports no value range
at all rather than reporting its frequencies as values.

The wire consequence is small and worth stating: the two numbers are **appended**
to the `"selection"` event and sent only when there are two, so a script reading
the two-number form it has always had keeps working, and an empty or inverted
pair means *no restriction* — the same convention a non-positive length already
uses on the time axis.

**And it is a gesture step, not what a plain drag does.** This was shipped the
other way round for exactly one afternoon, and an eye pass killed it: with the
band on the plain drag, sweeping a waveform came back as a rectangle while the
spectrogram below it — the picture that actually has two measured axes — came
back as a stripe, which reads as the two views having been swapped. Nothing was
swapped. A drag over a waveform means *this stretch of time* in every editor
there has ever been, and what a band of amplitudes is *for* — gate this range,
copy only these peaks — is the script's business rather than the host's guess.

So `select` sweeps the span and `select_box` sweeps the rectangle, and the
second **declines** where the picture measures only time: one binding
(`"select_box select"`) draws a rectangle where there is one to draw and the
plain span where there is not, and it keeps working unchanged the day a
spectrogram answers it with a range of bins. That a spectrogram wants *both*
selections — the temporal sweep and the spectral rectangle — is the case a
two-step plan per modifier already expresses, which is what makes this the
right shape rather than a workaround for the missing half.

The general rule it leaves behind is worth more than the fix: **a capability
whose sibling does not exist yet must not be the default gesture.** Until the
pair is complete, the half that shipped reads as belonging to the other view.

## A paste carries the clipboard, because the clipboard is the host's

The host owns no data, so a cut and a paste leave it as requests for whoever
does. That much follows from the premise. What does not follow, and had to be
decided, is **where the thing being pasted comes from**.

The alternative is that the owner keeps the clipboard: the host reports that a
copy happened and the owner remembers what was copied. It is tempting because it
keeps the wire small, and it is wrong for the reason the clipboard exists. A
clipboard is host-wide so that a block copied in one window pastes into another
— against a different owner, a different def, or a process that never saw the
copy. An owner-side clipboard makes that case impossible while looking like it
works in the single-window one, which is the worst shape a design can have.

So the paste event carries the whole typed document plus one blob per bulk
payload it names. The framing is the one `/gui_def` already uses — JSON as the
payload, OSC as the framing, bulk beside it rather than base64 inside it — and
it is also the **only** path a browser has, which settles the choice between
this and writing the block to a mapped file: one path both fronts share beats
two that drift. A mapped file remains available the day a clipboard is large
enough to want one; nothing about the event's shape would change.

**Copy stays on the host's side of the line**, and that is not an exception to
the premise but the premise read carefully: a copy is a *read*, and the host
already holds what it draws. What it may honestly copy is exactly what it has
mapped, which is why a source it cannot read — a peak overview with no samples
behind it, a live view with no addressable past — refuses out loud instead of
handing over a block of silence. The refusal is a payload of its own
(`"refused" verb reason`) for the reason every other refusal here carries one: a
key that silently does nothing teaches that it sometimes does not work.

**The rate travels and nothing resamples it.** That is the crate's rule for the
clipboard and it holds at both ends of the wire: resampling is an edit, an edit
is something an owner performs and logs, and a paste that quietly converted
would change data nobody asked it to change in a step nothing records.

## The pyramid stores energy, and a cache that never measured it says so

The peak cache grew a third statistic beside each bucket's min and max, and two
of the three decisions it forced are about honesty rather than about DSP.

**It is stored as mean square, never as RMS.** The reason is that a level of the
pyramid is *built from the level below it*, and mean squares combine while roots
do not: the energy of two buckets is the mean of their energies weighted by the
samples each holds, so a bucket at any level equals the direct mean square of
its samples exactly, and a renderer cross-fading between two adjacent levels
blends two numbers that mean the same thing. Storing the root instead would make
every level above the first an approximation of the one below it, and the error
would be largest exactly where a view is zoomed out and reading the top. The
square root costs one operation at the moment of display, which is where it
belongs — and it is also what lets the same array answer a question that is not
an amplitude at all.

**The weights are derived, not stored.** A bucket holds `bucket` samples except
at the ragged tail, where it holds the remainder — and that count is a function
of the buffer's length, the bucket size and the index, so combining exactly
needs no count array and the cache does not grow a fourth plane. The one place
it bites is the odd sibling: min and max may read the last entry twice (both are
idempotent), while an energy averaged with itself would weigh the tail as if it
held twice its samples, so the sibling is an option there rather than a clamped
index.

**A cache that predates the measure reports its absence instead of zeros.** The
format went to CLPK v3 and the two older layouts still parse, which leaves a
pyramid that has min/max and no energy. Filling that gap with zeros would be a
measurement — silence — and a measured layer drawn from it would be a flat line
across samples that is not flat. So the measure is an option that rides with
the data, `Pyramid::has_mean_square` reports it, and a view's honest answer to an
old cache is to draw no measured layer at all rather than a wrong one.

**And a mono cache is now one channel rather than a second format.** v1 was
mono, v2 added the channel count, and v3 could have been "v2 plus energy" with
the mono layout carried alongside — two writers, two sizers and two readers for
one picture. Instead both writers emit v3 and a mono cache is the one-channel
case of it, so `cache_size` is `multi_cache_size` at one channel and the layouts
cannot drift apart. Reading is where the leniency stays: v1 and v2 still load,
and the mono reader takes a one-channel v3 cache while **refusing** a wider one
rather than silently narrowing it to its first channel.

## A measure is a factor of the signal element, and a picture may carry several

The RMS body an editor draws inside a waveform's peaks could have been a widget
of its own. It is not: `measure` says what the element's columns measure —
`peak`, the min/max envelope, or `rms`, the symmetric body at the level the
signal held — and the classic editor picture is one view naming **both**
(`measure: "peak rms"`), drawn by the one renderer placed once per measure.

**That generality is what the signal element was factored for.** The
element is already a presentation × a source × its capabilities; a measure is
one more factor of that product, so it costs no name in the catalog and it
means the same thing everywhere the element goes — over a file it is an offline
reading, inside a clip a second body, over a bus a live one.

**The stack is inside the element because a picture owns its field.** This
first shipped as a *placement*: two signal elements handed one rectangle by a
lane, neither knowing the other was there, which is composition at its
cheapest and reads beautifully. It does not work. Every view of a signal paints
its field before it draws anything — a heavy view's `view_field`, a plot's
`track`, both opaque — so of two pictures on one rectangle the second is not a
layer over the first, it is a lid on it. The tests that let it through were
looking at the wrong things: one asserted the two placements had equal rects
(true, and irrelevant), another that the mesh grew (true of a covered drawing).
Making the fields translucent would have traded one picture's contrast for
another's, and each layer would still have brought its own ruler, gutter,
selection and upload of the same samples. So the layering is a **set on the
element** — one body, one axis, one ruler, one selection, one playhead, one
upload — and the order is the type's rather than the composition's: the
envelope is the outer shape, so it goes under, whatever order the names arrive
in.

**The body is drawn where the envelope is and nowhere else.** The measure stays
exact at any span; what a short span stops being is *informative*. Below a
cycle, root-mean-square and peak converge — measured on the example's own bounce
at 88 samples a column, the ratio runs 0.6 to 0.9, so the body simply retraces
the envelope — and on the way there it reads the wave's **phase**, beating
against the period in a lattice nobody can interpret. Editors answer this the
same way: Audacity's RMS "will disappear" as you zoom in, "because there are not
enough samples to provide a meaningful average in the region being displayed".

**The window is fixed, and what ends the body is the envelope** (settled
2026-08-19, by eye, after three answers that were not). Two rules, one picture:

- **A level is averaged over a fixed 50 ms of the source** (`BODY_WINDOW_SECS`;
  2400 samples at 48 kHz), reaching out around a pixel column's centre wherever
  the column is narrower than that. A root-mean-square is an average over a
  *duration*, so a window that followed the column would make the body's own
  values follow the **zoom** — the level moving over samples that did not
  change, which is the defect that decided this. 50 ms is WaveLab's default RMS
  window (adjustable to 999 ms); it sits below the ear's own integration —
  energy integrates over something like 200 ms, a VU meter's window is 300 ms —
  so it is the floor of what still reads as a level rather than the window a
  meter would pick. A caller with no rate (a live bus window, already measured
  in milliseconds) averages each column's own span, having no zoom of its own to
  be wrong about.
- **The body goes when the envelope has come down onto it** (`BODY_MERGE_RATIO`
  = 0.8, over the span on screen, weighted by the peak so the loud part decides).
  The envelope *does* narrow with the zoom, since a column covers less of the
  wave; once it is within a fifth of the level there are no longer two readings.
  This is also what keeps a fixed-window level from ever poking **outside** the
  envelope, which is the one thing a body must never do: one weight throughout
  and then a cut, the manual's own word again.

**Three answers were tried first, and each is recorded because each was wrong in
its own way.** The original ramped **opacity linearly in samples-per-pixel** down
to `RMS_FLOOR = 256` samples a column: the floor was the pyramid's summarizing
bucket and the order of a cycle of the lowest musical pitches — reasoning, not
evidence — and the ramp was measured against nothing, so the body's weight
tracked the *zoom* and a body at a third of its weight could be read as a quiet
passage. The second cut it where the level **met** the envelope but kept
averaging the column, which is measured and does track the signal, yet still let
the level's values move under a zoom — and watching them climb toward the peaks
is itself the artefact. The third made the threshold a **duration of the column**
(50 ms, then 10 ms): right about what the quantity is made of, and an order of
magnitude coarser than a working zoom — six seconds across a window is under
seven milliseconds a column, so any floor that reads as perceptual removes the
body from the place it is looked at. Only the fourth separates the two roles the
duration was being asked to play at once: it is the **averaging window**, and the
thing that ends the layer is the envelope.

**A source that cannot measure draws nothing.** A peak cache written before the
format carried the mean square has an envelope and no energy, and the layer is
absent rather than flat: zeros are a measurement — silence — and drawing them
over samples that is not silent is the one picture worse than no picture.

## A buffer's samples are one thing, and every picture of it takes the same write

A destructive edit in a standalone editor has to land in two places at once: the
server's buffer — the copy that sounds, and the one a save writes — and whatever
the window is drawing, because refetching a take to show a stroke that is
already on screen would be a round trip per gesture.

**The picture and the samples are one thing on purpose.** A session's take is
read into a **server buffer** rather than mapped as a file (see the session
sources note): the editing verbs address a buffer, so a host that drew a file
and edited a buffer would show one copy and write another. The same argument
carries one level down, inside the host.

**A take is drawn more than once, and the views hold it in different forms.** A
session window shows the clip in its lane *and* the take's editor under the
tracks — two widgets, one samples. Worse, they do not hold it alike: a
navigable view keeps a **peak pyramid** (a take is minutes of audio and reaches
the host summarized), while a clip's take body keeps the **samples** it was
handed. A host that patched the pyramid of the widget under the pointer
therefore left the clip drawing samples that no longer existed anywhere, and the
undo — which addresses the node, not the widget the hand happened to be over —
reported that it could not restore samples it was looking straight at.

So the write is split the way the knowledge is:

- **The element writes its own samples** (`Element::write_samples`), because
  only it knows which form it is in — it patches the pyramid, the samples, or
  both, recomputing only the columns over the span
  (`peaks::update_range`).
- **The host decides which elements**, and the answer is the **buffer number**:
  two widgets are two pictures of one samples exactly when they name the same
  buffer. Nothing else in the tree relates them — a clip and an editor of the
  same take are not parent and child, and they are not even in the same lane.

Both are replaced rather than mutated: the pyramid is shared with whatever slot
is drawing it (one `Arc`, so keeping it costs a pointer), and patching it in
place would rewrite a picture under a renderer that never asked. The element
marks its slot dirty instead, and the next frame refills it — keeping the view's
navigation, since where the eye is is not part of what was edited.

**Amended for the refresh, once the sharing was measured: the holder lets go
first.** Replacing means the element clones the pyramid before it patches it,
and that clone is proportional to the **take** while the patch is proportional
to the **span**. A stroke can afford it — one gesture, one copy — but a picture
that *follows* a recording re-summarizes on every step, so it paid for the whole
take each time and only a coarse cadence made that affordable. What the sharing
defends against is a **borrow**, not a race: the front is single-threaded and
the only other holder is the slot. So the leg that follows the frontier has the
view **give the samples back before the write** (`WaveformView::release_data`;
`WaveformData::nothing` is what a released view holds) and take it again on the
next fill, which a repaint runs first. In between the element is the sole owner
and `Arc::make_mut` hands it `&mut` with no copy at all — and where a slot *is*
still holding it, the same call copies first, which is the old rule unchanged.
The invariant above is what makes this safe rather than a shortcut around it:
the write leaves the slot dirty, and nothing draws from a released view, so no
renderer is ever patched under.

**One channel of a multichannel take needed a command that did not exist.**
`/buffer_setRange` writes a contiguous run of *flat, interleaved* samples, so
one channel of a stereo buffer is a **strided** write. Sending it as N
single-sample messages is not the answer; the command is — so the host refused
with that sentence rather than half-doing it, and the command arrived:
`/buffer_setRangeChannel` (and `/buffer_setChannel`), whose positions are frames
of one channel. Inside the server it is not a second write path but a **stride**
on the one that was already there. The edit vocabulary took the same coordinate:
`Intent::WriteSamples` carries a `channel`, which is what makes an undo put back
the channel it took rather than assuming the first.

**The lesson generalizes past the write.** Once a stereo take could be edited,
three other places turned out to have assumed one channel — the clip's body drew
only the first, the monitor played only the first, and a drag that crossed into
another lane read *that* lane's value. None of them was the write path, and all
three were invisible while the samples were mono. A channel count is not a
detail of the data: it is a fact every view, every gesture and every voice has
to carry, and the way to find out who forgot it is to open something wider than
two.

## A stroke writes what the reader can see

The pencil is refused until the picture draws its samples one by one — a stroke
before that would write values nobody can check, and the refusal is visible
(`"refused" "draw" …`) because a pencil that sometimes silently does nothing
teaches that it sometimes does not work.

**The threshold is the drawing's, and for a while it was not.** This section
first said *where a pixel is more than one sample*, and the gate was written to
that number; the trace puts its sample dots on at three radii of spacing, which
is twelve pixels a sample at the default density. Between the two a stroke was
allowed over a picture with no dots in it — found by eye on 2026-09-07, as *"it
lets me edit before the discs appear, and a very large amount at once"*, which
is both halves of the same gap: the samples were not shown, and a drag across
the body wrote every one it crossed. So the gesture asks the drawing
(`graphics::signal::trace::samples_are_drawn`) instead of carrying a threshold
beside it. The heading was right all along; two numbers for one rule is what
was wrong, and it is the general lesson — a rule stated in two places is a rule
that will disagree with itself.

The same rule decides what happens when the hand keeps going: a drag holds the
pointer, so it goes on reporting past the edge of the view and past the window
itself. Following it would rewrite samples nobody is looking at, and the damage
is discovered only by scrolling there afterwards. So the stroke is **clamped to
the body it started on** — the last visible column still follows the hand, and
the stroke ends where the eye does — and clamped again to the buffer's last
frame, since the right edge of a fully zoomed-out view maps to *one past* the
last sample and a stroke carrying that frame is refused whole by the owner.

## One layer is edited at a time, and it is the only one that acts

**Context.** A container that layers editable things — a clip today — draws
several of them on one rectangle: the placement (where it sits, how long it
is), the samples under it, the events over that, an automation over both. Four
claimants over the same pixels, with no rule saying which one a press belongs
to. Three attempts at something as small as a read-only body were written and
reverted in one day, each of them changing what the clip itself does, because
every attempt was really a fourth ad-hoc precedence added to three that already
disagreed.

**Decision.** One sentence, in `host::layers`: **one layer is active at a time,
and it is the only one that acts or offers an affordance.** A press resolves the
layer from what is drawn under it — the active layer first, then the topmost
layer whose *own samples* is there, then the container's placement — and
selecting a layer is an operation of its own, with the pointer rule as one
caller, `/gui_set layer` as another, and a key binding or a menu as the next.

Three things make it a rule rather than a table:

- **A layer's data is not its rectangle.** An element answers
  `Element::layer_hit` for the things it holds — a break-point, the line
  between two of them, a note — and never for the rectangle it shares with its
  container. That is the whole of why the clip's background, and the grips drawn
  on it, stay the clip's.
- **The stack is the container's contents**, read off the children that fill a
  body role, in the order they are drawn. Nothing in the module names a widget
  type, so a container that grows a fourth kind of content grows a fourth layer.
- **The active layer is a field of the node**, not of the `Clip` variant. A clip
  is the first container here that layers editable things and deliberately not
  the last: an audio editor's view is the same picture — samples, a selection
  over it, an automation over both, later a spectral layer.

**Consequence.** Two standing defects closed without being touched directly. A
clip whose contents are locked **moves again**: a layer that cannot be edited is
never selected by pointing at it, so the press falls through to the placement
instead of being consumed by a refusal — where a body sits is the
composition's, what it holds is the body's. And a curve's **lit segment** is
lit exactly when the curve is the layer in hand, where the bend is the gesture;
inside a clip it used to be lit whether or not the curve was in hand, and the
press there moved the clip.

The grips followed from the same rule rather than from a second one: they are
the placement layer's affordance, so they are drawn while it is active, and the
pixels that light up are the pixels that act — by construction now, instead of
by a precedence written down in two places.

**What is drawn is a second question**, and it is answered the same way:
`visible` on a node, `hidden` on the container, naming which layers are drawn.
Several are drawn at once and one is edited. The two meet in one rule: what is
not drawn is not edited either, since a window taking presses for a picture it
is not showing is the one combination a reader cannot see.

## A column reaches the column before it: the trace is a curve, and the columns are not

A digital square wave drew as a row of dashes along the top and another along
the bottom, with the vertical edge between them missing — and it came back at
some magnifications and vanished at others, which is what said the picture had
lost its grip on the samples rather than that one zoom was wrong.

The two regimes are one picture: below `LINE_THRESHOLD` the trace is the
polyline through the samples, where every consecutive pair is joined by a
segment; above it a column is that same polyline's envelope over the samples one
pixel covers. But a column measures a **group of samples**, and groups partition
the samples where the curve does not. Between the last sample of one column and
the first of the next there is a segment, and it was drawn nowhere. On audio it
never showed — a column holding a hundred samples of a wave already spans nearly
the excursion its neighbour does, so consecutive columns overlap by themselves —
and on a one-sample jump it is the whole of the feature. Whether the jump lands
inside a column or on its boundary is a fact about the zoom and the scroll, so
the edge appeared and disappeared as the view moved.

**A column is inked over what it measured, extended to reach the column before
it** (`trace::join`): where the previous column sat wholly below, this one
reaches down to its top; wholly above, up to its bottom; already overlapping,
nothing changes. What is remembered for the next column is the **measurement**
and never the extension, so a run of them cannot walk the trace outwards.

This is not the fill that was refused next door ("A waveform column is its own
envelope"), and the distinction is the whole of it. Filling to the baseline inks
values **the signal never took** — three samples at +0.6 drawn from 0 to 0.6 —
and needs a zoom threshold nobody can name. The join inks exactly the values the
curve takes **while it crosses the boundary**: the same segment the polyline
draws a zoom later, no more, measured from the two columns themselves. Both
rules say the same thing from two sides — the drawing shows the signal and
nothing else, at every zoom, and never changes its mind about what it is looking
at.

It holds for every source (raw samples, a peak pyramid, a cache-only view) and
every picture (the navigable waveform, a clip's take, a plot's series) at once,
because the host's `draw_channel` is the one renderer. The measurements stay
untouched: the pyramid's buckets keep tiling, the wire's overview keeps its
meaning, and nothing about the summary format moves.

**The rule itself is the core's** (`clausters_core::peaks::join`, and
`join_columns` over a whole measured row), which is the correction the web
client forced. "One renderer" is true of the *host* and of nothing else: a page
that reads `Peaks.columns` gets measurements and strokes them itself, so the
first version of this left the rule in Rust and copied it by hand into one
example — and the square wave came apart in a browser exactly as it had in a
window, while every widget-drawn picture in the same page was right. Two
implementations of one rule is how the same buffer comes to look different in a
window and in a browser, so the rule went where the measurement already is,
beside it and one level below both drawings: the host calls it per column,
keeping only the walk (which column the trace is on, and where a run starts
again because a column held nothing), and the web client's `data.joinColumns`
calls it over the row a page just measured. The C ABI has no row of its own —
nothing on that side strokes pixels — which `docs/bindings.md` records.

**The web half of that is gone, and the rule is better for it** *(W26)*. A page
strokes no columns any more: `Peaks.columns` and `data.joinColumns` were a
surface no other client had — a second drawing, reachable from one language —
and they were removed rather than kept in step. What is left is the sentence
this record was always making: `peaks::join` is the core's, the host is the one
thing that calls it, and a client names what to look at. The measurement side
is untouched, and so is everything above about *why* the join exists.

## A zoom past the summary asks for a finer summary, and only then for the samples

A view that cannot map the samples used to have two states and one crossing: the
summary it holds, and — past the base bucket — the **samples** over the span on
screen, read back with `/buffer_getRange`. That is right for the deepest zoom
and it is the wrong request everywhere else. What a picture needs is **one
min/max pair per pixel column**: eight hundred columns is eight hundred pairs,
and at a hundred samples a pixel the span behind them is eighty thousand samples
— a few hundred kilobytes through a 64 KiB carrier, a chunk a frame, to compute
a few kilobytes' worth. `/buffer_peaks` measures exactly that row, at any
bucket, in one reply.

What stood in the way is that **a view holds one pyramid at one base bucket**,
and a report at another bucket cannot be folded into it: the grids do not line
up, and `write_buckets` refuses it, correctly. So the finer answer lives *beside*
the summary rather than inside it — `waveform::Detail`, a pyramid over a span,
the way `Samples::Window` is a run of samples beside the whole. The regime picker
gains a rung: the samples where they answer, then a detail grid where it covers
at this zoom, then the summary.

**The crossing between the two requests is a question about bytes**, and it is
the wire's own arithmetic: a bucket is three floats where a sample is one, so a
grid only pays where a bucket holds well more than three samples. At four it
carries three quarters of what it describes, which is no saving for a second
grid to keep; at sixteen it carries a fifth. With a column holding two buckets
or more — so the fold's position error stays under half a pixel — that puts the
crossing at about **thirty-two samples a pixel**, which is also roughly where
the trace stops being columns and becomes the polyline through the samples. Below
it the samples are asked for and kept: they are exact, they answer every deeper
zoom too, and a grid answers only down to its own bucket.

The grid's bucket is a **power of two**, so a zoom holds still: a grid answers
every column from its bucket upwards (the levels above it are the same pyramid),
so zooming out is free and zooming in asks again only after a factor of two. And
it is coarsened until the whole span fits **one reply** (4096 buckets), because a
detail grid is *replaced* rather than extended — one grid at a time per view, for
the same reason there is one window at a time: what it is for is where the eye
is, and a cache with a policy is a different design.

Nothing about this is on the wire. Both requests are ordinary commands the
server already answered; which one a host sends is its own business, and the
draw pass is where it is decided because that is the only place that knows the
zoom and the span at once (`frame::owed`).

## A knob is measured from its press, so no front captures the pointer

*2026-08-22.*

A knob has no groove on screen to point at, so what turns it is a distance
rather than a position — and there are two ways to read that distance. The
first: accumulate the step since the last event, re-anchoring every frame. The
second: measure the travel since the **press**, against the value the press
found. They agree while the pointer stays on the widget and part company the
moment it does not.

The host had the first one, and paid for it twice. A drag whose pointer left the
element kept accumulating whatever motion still arrived, so the value fell out of
phase with the hand; and the clamp at an end **ate** the motion spent past it, so
coming back left the value short by however far the hand had gone. The same pair
of defects had already been found and fixed on a curve's bend, which is now
anchored at the press (`bpf::bend_curve` sets rather than adds). A knob was left
incremental on the argument that it is different — a knob has nothing on screen
to stay level with, so nothing to be out of phase *with*, and a pinned control
that reverses immediately reads better than one that has to unwind.

The reason to accumulate was that the cursor was expected to be **captured**: the
desktop front locked the pointer at the press (`CursorGrabMode::Locked`) and drove
the element with raw device deltas, so there were no positions to measure from.
The browser front had no such path, so the very same widget was one gesture in a
window and another in a page — which is a defect under the standing rule, not a
platform's prerogative.

**Making the page capture the pointer was tried and does not work**, and the
reasons are winit's web backend rather than ours:

- `set_cursor_grab(Locked)` calls `requestPointerLock` and returns `Ok(())`
  whatever the browser decides afterwards, so the front's answer — the very thing
  the machine routes on — is a guess. Nothing synchronous can do better: the lock
  resolves asynchronously, and the press has to decide now.
- The deltas it would be paid in are emitted only from a `pointermove` that
  carries **no button** (`PointerEvent.button == -1`). Chrome reports `-1` on a
  move; Firefox reports `0` while the button is down and takes the chorded-button
  path instead, where no motion event is emitted at all. The knob would be dead
  in one of the two browsers — the same Chrome/Firefox split that once delivered
  a press per frame.
- Those deltas come from `getCoalescedEvents()`, whose list is **empty for a
  synthesized event**, so no page test could drive a captured knob even where it
  worked.

So the capture is gone and the measurement changed instead: `Dial` records where
the press landed, the fraction the value stood at there and the body's height,
and every step is `t_press + drag_fraction_delta(cy - y_press, body_h)`, clamped.
A cursor position has **one** answer, so the pointer may leave the disc, cross
the window and come back with the value exactly where it says, and the motion
spent past an end is kept rather than eaten. `number` follows `knob`; the
absolute controls (`slider`) never had the question.

**What this costs** is the pinned-reversal the incremental form gave: past an
end, the hand has to travel back to the anchor before the value moves. That is
not a trade-off so much as the same rule applied honestly — the value is where
the cursor says, at an end as everywhere else — and it is what the bend already
does.

**What it removes** is a whole seam: `Take::grab`, `Claim::grabbing()`,
`Element::drag_relative`, `Gestures::locked`/`relative_motion`,
`GestureEffect::ReleasePointer`, the grab callback `Gestures::press` took, and
the native front's `grab_pointer`/`release_pointer`/`device_event` path. No
element asks a front for the pointer any more, and a gesture is made of cursor
positions on both fronts — which is what makes the two the same program.

## A drawing rule can belong in the shared core, and this one does

`clausters-core` is described as the numeric and timing layer — "general audio
tools, not display code" — and the split has held: a decibel curve, an
oscilloscope's framing, a spectrum's peak hold are all the host's, and a client
that wants to see a signal names a view rather than computing one.

The axis a break-point curve is **drawn** against went into the core anyway
(`envshape::curve_axis`, `clausters_core_curve_axis` / `curveAxis`), and the
reason is worth writing down because the boundary above says it should not have.

It is not that the arithmetic is hard — it is a range, a tenth of headroom, and
a comparison. It is that **the same curve is drawn in four places**: the
standalone curve editor and the clip body that draws the same automation, in
each of two clients. The rule was already written twice when the third caller
appeared, and the failure mode is not a wrong number but a *divergence nobody
can see*: two windows over one curve, one of them rescaling under a drag and the
other not, with no test able to compare them because each client's copy is
correct against itself.

So the line is not "audio belongs in the core and drawing does not". It is
**what more than one process must agree about lives once**, and a client's
picture of shared data is exactly that kind of agreement — the same argument
that put the envelope segment shapes there, where what the editor draws is what
the server plays, by construction. What stays out is what only one renderer ever
answers: how a trace fades, how a ruler labels itself, how many pixels a column
is.

The rule itself has one non-obvious half. The fresh range is right **exactly
once**: recomputed on every redraw it makes an edit rescale the picture, so
dragging one point visibly moves every other one — the defect a hand at the
window notices before it notices a wrong number. So the axis is remembered by
whoever draws and only ever **widened**, one side at a time: only the end that
stopped holding the data moves, since taking the union of the two padded ranges
would drop the floor whenever the ceiling grew, which is the same jump one step
removed. Widening rather than refitting is also what makes a point dragged down
and back up leave the drawing where it was.

## A widget id is named after what it draws, not leased

Pooled ids recycled across redraws, so an edit-back in flight, a correction on
its way out, or the host's own state on a widget landed on whatever took the
number next — the recurring visualization failure. The earlier decision above
solved a script's ergonomics (a name to hold instead of an integer) and left an
editor's redraw churning.

So a view asks for an id **by name** — the structure's identity, the widget's
role in the picture, and a key within the role — and gets the same number for
as long as the widget keeps being drawn. `clausters_core::widgetids::WidgetIds`
is one occupancy map with two doors: the anonymous lease a hand-built tree
takes, and the named id. Four rules come with it, each of which reads like
something to simplify:

- **A name is a map, not a hash.** A 31-bit space and a thousand live widgets is
  a collision every few thousand sessions, and a collision is two widgets
  answering to one number — silent, and indistinguishable from the defect the
  table exists to remove.
- **A draw names its drawer.** One table serves a whole host and a host carries
  more than one editor; a draw that did not say whose it was let one editor's
  redraw retire another's widgets.
- **An anonymous free cannot take back a named id.** A host frees a redefined
  subtree widget by widget while the names still hold those ids; releasing one
  there would hand one number to two widgets. A named id leaves only when its
  drawer retires it.
- **A draw with no host is private to its drawer** and numbers from the start,
  so drawing one picture twice gives one tree. On a host that would be wrong,
  because the leases there belong to every window the client has open.

Measured when the multitrack's widgets took it: two draws of one composition
gave the same ids, and a redraw of the same piece cost zero definitions and zero
sets. It is also what the host matches widgets by when it reconciles a def
(below).

## The picture has one owner: the host reconciles a def

The client used to send the difference between the tree it last sent and the
tree it would send now. That is correct only if its copy equals what the host
draws, and it cannot: the host mutates on its own every frame (a drag writes an
offset, a wheel writes a window), screen state reports nothing on purpose, and
a `/gui_def` can fail with nothing said. Versioning the picture puts the client
back to whole redefines on every drag; narrowing that means the client modelling
the host's rules, in two languages. And moving the difference into the host
reproduces the defect one process over, since the host writes its widgets
without writing the last document it was handed.

So **no client keeps a picture.** A `/gui_def` over a tree the host already
draws means *make it look like this*: the host walks the tree it holds beside
the one it was handed, matches a widget by id wherever it moved to and by
position for the bodies the wire does not address — only where the type agrees
— and carries its own state across: the window on the axis, the selection, the
active layer, what is hidden. The def still wins on any key it states. Bulk is
kept by being **asked for** (`"data": "keep"`), because silence already means a
clip with no body. Paths were considered and rejected: a path encodes a position,
and re-parenting a clip is the multitrack's commonest gesture. The redraw
difference was retired from the core and from both clients.

What decided the granularity, measured over one clip dragged for 60 frames:

| piece | the old delta | the widget the edit named | the whole window |
|---|---|---|---|
| 4 lanes × 4 clips | 21 B/frame | 91 B | 1.9 kB |
| 8 × 16 | 21 B | 91 B | 13 kB |
| 24 × 40 | 21 B | 91 B | 97 kB |
| 64 × 100 | 21 B | 91 B | 652 kB |

The window grows with the piece — 39 MB/s of JSON at drag rates on the largest —
and the widget the edit named does not. So which widget to publish is the
**caller's** argument: an editor knows which node its intent touched, and
nothing inside a publish can.

The field's name for this is reconciliation (the DOM's, Qt's `DelegateModel`,
kdenlive's timeline); it arrived here from a bug report, a lane split that reset
the zoom of every other lane in the window.

## Two reconstruction filters: the standard's for the number, a designed one for the picture

`clausters_core::resample` carries two polyphase tables where one would have
looked like enough, and the reason is that they answer different questions.

**The measurement takes the standard's.** `PHASES_4X` is ITU-R BS.1770-4,
Annex 2's own table — the order-48, 4-phase interpolating FIR the recommendation
prints — transcribed rather than designed, because a number this project calls
dBTP has to be the number the recommendation defines. It is not a filter anybody
would design today: its four branches sum to 1.0016, 0.9730, 0.9730 and 1.0016
rather than to one apiece, it ripples about a fifth of a decibel across the
passband, and its images are down only 40 dB. The Appendix says why in as many
words — "since we are not going to listen to the output of our over-sampler, but
only use it to display a reading … we probably do not have the same precision
requirements" — and the measured figures are asserted in the module's tests, so
what is carried is the standard's filter and not a plausible transcription of
it. (That distinction is not theoretical: the table was first written from
memory, and it took a numerical check against the recommendation's own text to
establish that it happened to be right.)

**The drawing cannot use it.** A waveform zoomed past its samples draws the
reconstruction between them, and the BS.1770 filter's centre falls half a
sub-sample off the grid: **none of its phases is a passthrough**, so the curve
misses its own dots by up to 0.09. `PHASES_8X` is therefore designed, in the
form that makes the property structural — the fractional-delay
`sinc(k − d) · kaiser(k − d)`, where `d = 0` leaves every term but `x[i]` a sinc
at an integer, so phase 0 **is** the sample, exactly. Eight phases rather than
four for the second reason the Appendix gives: the residual error of a true-peak
reading is the *grid* and not the filter, and the recommendation tabulates it —
0.554 dB of worst-case under-read at 4×, 0.136 dB at 8×.

**And the reconstruction is a layer over the samples, never a replacement for
them.** The first cut drew the curve *instead of* the straight segments once a
zoom crossed into the polyline regime, and a waveform's amplitude jumped there.
Measured, the jump is not a transition artefact at all: the envelope of the
samples sits below the envelope of the signal by a **fixed** amount that is the
signal's own inter-sample content — 0.21 dB for a band-limited 440 Hz
sawtooth, 3.01 dB for a tone at a quarter of the sample rate — and the same at
1, 8 or 64 samples per pixel. So no threshold removes it: the two pictures
measure different things, and swapping one for the other is a jump by
construction.

The field's two conventions each avoid it by never swapping. An **audio
editor** draws samples at every zoom — Audacity's manual: the envelope is "the
tallest peak in the area that pixel represents", and zoomed in "the waveform can
now be clearly seen as joining together many individual sample points". An
**oscilloscope** draws the reconstruction, as a selectable display mode ("most
digital oscilloscopes offer a choice of either of two interpolation processes:
linear or sin(x)/x"). The one editor that draws both, iZotope RX, draws them
**together**: "Show analog waveform plots the analog waveform in red under
digital sample values (blue)". That is the shape taken here. The reconstruction
is the `signal` **measure** — a layer in the same stack as `peak` and `rms`, in
an ink of its own — so the sample picture under it is continuous at every zoom
and the curve is *added* where the samples are separate points. Where it is
drawn, the sample layer keeps its dots and drops only the straight segment the
curve stands in for. It is on by default (`"peak signal"`), as it is in RX.

Zoomed out the layer draws nothing yet. Drawing it there means the envelope of
the *reconstructed* signal at every zoom, which belongs in the peak pyramid as a
plane computed once when the cache is built — the same move the mean square
made — and not in a reconstruction of every visible sample on every frame.

**What this is not.** It is not two implementations of one rule, which is the
thing the shared core exists to prevent: it is one mechanism (`Interpolator`
over a phase table) and two tables, each named for what it is for. A caller that
wants the standard's number asks for `BS1770`; a drawing asks for `FINE`; the
choice is visible at the call site rather than hidden in a constant.

The 12.04 dB attenuation the standard specifies before oversampling is skipped,
and the recommendation authorizes exactly that: it is headroom for integer
arithmetic, and "this step is not necessary if the calculations are performed in
floating point".

## A loudness meter keeps every block without growing, and redesigns the standard's filter

`clausters_core::loudness` measures what ITU-R BS.1770 and EBU R 128 define,
and every call the documents leave open was taken from the documents themselves
or from libebur128, the implementation the field treats as the reference. Three
of those calls are not obvious from the code.

**Gating needs every block, and a meter may not allocate.** The integrated
loudness is a mean over all the 400 ms blocks since the reset, gated by a
threshold computed from those same blocks, so a meter has to keep them; a list
that grows cannot run on an audio thread, and a live meter is the use the
streaming type exists for. libebur128 answers with a histogram mode — fixed
bins of loudness, counted — at the price of reading each bin as its centre. The
bins here are the same idea at 0.01 LU from −70 to +30 LUFS, but each holds the
**exact sum of its blocks' energies** beside the count, so every mean the
readings take is exact and the only approximation left is which side of a gate a
bin falls on, decided by the bin's own mean (exact for blocks that share a
level, as a test tone's do). The loudness range reads the same structure for its
percentiles, 0.01 LU apart. The two windows are a ring of 3 s of per-frame
energy with running sums, and each sum is **replaced** once per window by one
accumulated fresh since the window began, so a loud passage followed by silence
leaves no rounding residue. About 0.9 MB a meter at 48 kHz.

**The filters are redesigned by inverting the table, not by carrying
constants.** BS.1770 publishes the K-weighting at 48 kHz only and asks for "the
same frequency response" elsewhere. The bilinear transform is invertible in
closed form — `K² = (1+a1+a2)/(1−a1+a2)`, and the numerator gains out of
`b0+b1+b2`, `b0−b1+b2`, `b0−b2` — so the analogue section is recovered from the
standard's coefficients and transformed again at the rate asked for. What
comes back matches libebur128's literals to nine digits, which is how the
algebra was checked. One difference is deliberate: libebur128 keeps the
high-pass numerator at `[1, −2, 1]` at every rate, which moves its passband gain
with the rate (by about 0.002 dB at 44.1 kHz); here the numerator is redesigned
with the rest, so the gain is the standard's everywhere.

**The documents, not the reference implementation, where the two disagree.**
Tech 3342 has required the short-term loudness behind the range to be sampled
at least 10 times a second since its 2016 revision; libebur128 still takes one
value a second, as the 2011 text allowed, and so the range here samples at
10 Hz. And the momentary and short-term maxima are taken at every sample rather
than at the 100 ms block rate, because Tech 3341's case 13 is written to fail
exactly that: a 400 ms tone that starts off the block grid is never inside one
window whole, and a meter that looks every 100 ms reads it up to 0.46 LU low.

## A loudness layer brings its own axis, and its curve is measured once

The loudness curves (`measure: "momentary"` / `"short"`) are the first layer in
the signal element that does not measure the picture's own quantity. Peak, level
and reconstruction are amplitudes: they share the view's vertical, and a reader
compares them directly. A loudness reading is in LUFS, so drawing it against an
amplitude axis would be a number on a scale it does not belong to.

**So the layer states its scale instead of inferring one.** It is EBU Tech
3341's, named by its top — `+9` runs from 18 LU under the target to 9 over it,
`+18` from 36 under to 18 over — with EBU R 128's -23 LUFS as the target and the
zero of the scale. The layer draws **its own ruler**, on the right of the body,
labelled in LU relative to that target; the left strip stays the picture's own
axis, because two axes on one strip are two numbers where a reader expects one.
This is the concrete case the A track's later milestone generalizes into the
rule that a layer declares whether it maps through the container's axis or
normalizes into its box: the question was met by a layer that actually has a
second domain, rather than decided in the abstract.

**The curve and the numbers come off one measurement.**
`clausters_core::loudness::Profile` holds the take's weighted energy in 10 ms
blocks with running sums, so a momentary or short-term reading anywhere is two
subtractions, a pixel column is the band between the readings it covers, and the
gated aggregates over any span are a walk over blocks. It is in the **core** and
not in the host by the placement rule — a client analyzing a rendered file wants
the same curve — and it runs the same `Weighting` stage the streaming meter
does, so a drawn curve and a live meter cannot drift apart.

**Which is also why nothing is marked stale.** The A track decided in advance
that an edit would invalidate the aggregates entirely, and that a read-out would
therefore have to mark them stale and recompute on demand. That reasoning
assumed measuring them meant a pass over the samples. Over the profile it does
not: an edit re-measures only the blocks it touched, widened by the filters'
memory, and the aggregates that read those blocks follow immediately. The rule
the decision was made from — *a measure's affected span is the edit's span
widened by the measure's memory* — is what the incremental update implements;
the staleness it predicted was a consequence of a cost that turned out not to
exist.

**Live, the same measure is the meter's own curve**, fed from the bus's retained
history rather than from the oscilloscope's display window beside it. That
window is re-read every tick and overlaps itself, so a meter fed from it would
count the same audio several times over; the history is de-duplicated by stream
position, which is what makes one algorithm serve both a file and a bus.

## A layer declares its vertical, and a presentation is one layer of a stack

A signal picture used to be a *set* of measures whose order was the type's own —
the envelope under the level, because an envelope is the outer shape and there
was nothing to choose. That holds for two readings of one quantity and breaks
the moment a picture has a bottom: a spectrogram with the wave drawn over it is
the same body with a different layer underneath, and no ordering by type decides
between a texture and a curve. So the stack is **declared** (`layers`, back to
front), and two things follow that were not obvious before it was written down.

**A presentation had to become a layer.** The alternative was two elements on
one rectangle, and that is the mistake this project already made once: every
view of a signal paints its field before it draws, so the second is a lid rather
than a layer (see "A measure is a factor of the signal element"). The
presentation therefore stopped saying what is drawn and now says only what the
**default** stack is — `peak signal` for a trace, `spectrogram` for the
time-frequency view — and everything that used to ask it what the picture was
asks the stack instead: whether there is a texture, what a live tick
accumulates, how wide a tap read is, which bulk form is loaded, whether a y zoom
centres, the y strip's unit, the axis' headroom.

**A layer states its vertical rather than the host inferring one.** `y: "axis"`
maps through the body's own vertical and *reports* it — the ruler and the cursor
read-out are that layer's — while `y: "box"` normalizes into the rectangle with
a scale of its own, which is what the loudness curve was already doing before
there was a word for it. The default follows what a layer measures, so the
common stacks need no `y` at all. Two drawn layers claiming the axis for
**different domains** is refused at both doors (the def fails to build naming
both; a `/gui_set` leaves the picture as it was and says why), because
whichever one lost would be drawn on a scale that is not its own — a picture
that lies about what it shows. Refusing is cheap and recoverable; a silent
winner is neither.

**And a layered view reads the samples, not a summary.** An STFT is made from
samples and a peak pyramid is not, so a stack naming both asks for the audio and
derives both pictures from it, filling a geometry slot and a texture slot under
one key (the frame holds each kind in a map of its own, so the two do not
collide). The consequence is stated rather than worked around: a source that is
a summary and nothing else — a peaks cache, a streamed overview — draws no
spectrogram layer at all, exactly as an old cache with no mean square draws no
level body.

## A host names its own widgets past its clients' window

Widget ids are one namespace per host, allocated by its clients from a window
that starts at 1000. A host also opens widgets nobody asked it for: a roll over
a sequence of the session it owns, a bundle it mounts when it starts. Those
were numbered by hand -- a roll at `1000 + 10 * source`, a bundle's block from
1000 -- inside the very window a client attached to that host allocates from,
and a bundle's node ids, buses and buffers came from a bump allocator beside
the host's own id spaces, which hand out the same node ids.

The host now has a widget-id table of its own, over the window that starts
where the clients' ends (`clausters_core::widgetids::HOST_BASE`), and a mount
allocates its server ids from the host's `IdSpaces`. The split is the one the
node table already has between the server's ids and the clients': two
allocators of one namespace, kept apart by where each starts rather than by a
protocol between them. A share of the clients' window would have needed the
launcher to say how many clients there are, which the host that opens a roll
by itself is not told.

The same audit found the page keeping two occupancy maps over the clients'
window -- a mounted component's block and the page host's client each started
at 1000 -- and those are one table now (`pageWidgetIds`).

## A menu is a prop, the chooser is `choice`, and what opens over a window is the host's

A review of the host's chrome found one popup and seven defects in it. The
chooser held its own open list as a rectangle resolved at the press, against
the whole framebuffer: so the status bar, drawn after it, covered its last
rows; a list taller than the window ran off it; a resize left it where the
field had been; and only a press was routed to it -- the wheel reached the tree
underneath, no key closed it, and a static window never repainted the row under
the pointer. None of that was the chooser's to fix, because a menu bar and a
context menu have no element at all.

So what opens over a window is the **host's**: one stack of lists per window,
placed by one function inside the work area (the window minus the host's
bands), which the frame draws with and the press hit-tests with, run at every
frame so a list follows what it hangs off. An element that wants a list asks
for one and is told the pick; it holds the options, not the list.

A **menu is a prop value**, not a node. It is shown in three places -- along a
window as a bar, at the pointer as a context menu, under a button -- and in none
of them is it laid out among the widgets, so it is a tree a prop carries (`menu`
on a window or a button, `context` on anything), and what a pick reports is the
entry's verb from the widget that carries it. The bar is a window prop drawn by
the host, as the status bar is, rather than a widget in the tree: a widget
would have to be placed, and a bar that a layout could put anywhere is a bar a
reader has to look for.

That took the name `menu` from the chooser, which is `choice` now. The two
report different things -- a verb and a value -- and a chooser was never a menu;
`choice` also names the one element the six presentations of one-of-several
(combo, radio, segmented, tabs, pager, list) are, since the data, the value and
the event are the same in all of them and only the picture differs.

An **icon is a glyph**: the host has fonts and no images, so a small symbol set
is named by the codepoints that mean them and drawn by the host as shapes in a
glyph's cell. Drawn from the bitmap face they were a staircase beside the
smooth text of a loaded typeface, and most typefaces have no play triangle to
draw them with instead; as shapes they are crisp at any size, with or without
a face.

Two choices made with the code. The light controls became stops on the tab
ring and **do not report** the focus they take: their event stream is their
value, read by callbacks of one argument, and a `("focus", 1)` arriving there is
an argument nobody wrote for. And a binding may name its target **by the node**
(`bind=["widget", pages, "index"]`), resolved when the tree is opened, because
`tabs` and `pager` wire a chooser to a stack before either has an id; it is
resolved at the nearest container holding both ends, so a composition nested
twice wires each copy to its own target.

## A file chooser is the host's, and the host lists the directory

Choosing a file was first left out of the chrome as "the platform's". It is
not: a native dialog in a window and something else in a page is a behaviour
that differs between the two builds of one host, and a page has a filesystem
(OPFS). So the chooser is an element the host draws (`files`), and **the host
lists the directory**, not a client: an element asks for a listing
(`Element::wants_listing`) and is handed it (`Element::listed`); the native
front reads the disk at once, the page awaits OPFS and hands the answer back
through its event proxy, the way a bulk fetch already returns. The agnostic
core touches no filesystem, and neither client implements a listing.

It is an element a def composes, not a request a client makes of the host: a
request would have been a new `/gui_*` address, and a chooser placed in a
`dialog` beside a `text` bound to its value is a file dialog with nothing new
on the wire (`file_dialog`). A file on the user's own disk reaching a page's
storage, and leaving it, is the port's question and not the chooser's.

The rows it shows are a `table`'s, which is a list, a table and a tree as one
element. It reports what a hand did and never reorders: a header press moves
the mark and asks, since the rows are the owner's -- the host holds no
document. Which branches are open is a prop, written where it is drawn and
reported, as a menu's check is.

## The clipboard is the platform's, and text is all that crosses

*(2026-10-04, with the user.)* The host had one typed clipboard of its own --
text, notes, clips, samples -- which reached no other program. Natively it now
reaches the desktop's through one small seam (`SystemClipboard`, beside
`FontSource`), and two rules keep the typed half intact. **Text crosses, the
rest stays**: a string put on the clipboard (a field's selection, and a notes or
clips block, which already ride as JSON text) is written to the desktop's, while
a range of samples, which no other program reads, stays the host's. **A paste
reads the platform first, and only at a paste**: text another program copied
after the host's last write is what is pasted, and otherwise the host's own --
so a block of samples still pastes after a copy elsewhere left the desktop's
text untouched. Reading per key would be a round trip to the clipboard's owner
on every keystroke.

The clipboard moved onto the `Host` because a field's edit menu and the middle
button paste from places a front never hands one (a menu's pick, a press), and
an element edits while it borrows the host, so it is taken off for the length
of the call. The primary selection -- what a middle click pastes -- rides the
same seam, with the host's own beside it. A page keeps the host's own
clipboard: the host draws inside a canvas, and reaching the browser's takes
code on the page's side, left as a future direction.

## Text is three quarters of its line, and a symbol is twice a word

*(2026-10-05, with the user, who asked for the proportions rather than a
number: "the proportions have to be found, I do not know them".)* The host's
default `text_size` was 2, which writes capitals 14 logical pixels high -- the
whole of the cell every size role is derived from. Two things followed from
that one number. Text was half again the size of the text around it: a
browser's default has capitals of 11 to 12 pixels, an interface's 9 to 10, and
a window mounted in a page sat beside both. And a line had no air, because the
line *was* the capitals -- so a symbol drawn at the text's size, which is what
an icon was, looked small beside words that filled their row.

**The default is 1.5, and the cell stays 14.** Capitals are 10.5 in a line of
14: three quarters, the proportion a line of print has. The cell is declared
and never derived from the face, so nothing else in the table moved -- a
control is 22 high, a ruler 18 -- except the two roles counted in characters.
What had to change with it is `size::line_box`, the height a line of *content*
takes: it was the text's height plus padding, which is the cell only while the
text fills it, and a label would have come out shorter than the control beside
it. A line is the cell at any text size up to the cell's.

**A symbol's size is its text's, and it is nearly twice a word's.** An icon
is a glyph (a window brings the outlines of a music font), so a widget that
shows one sets `text_size`; the score editor's tools draw theirs at 2.5 and its
palettes at 3.5, an em of 22 and 30 pixels. That is the ratio an icon has to a
caption on every desktop -- 16 pixels beside capitals of 9 -- and it is what
makes an accidental stand a capital and a half high.

The half-step is not new (ruler captions shipped at 1.5), and on the bitmap
face it is the one concession to unequal pixels; on a display scaled by a
third, the user's, 1.5 lands on a whole step. A script that wants the old size
says `text_size: 2`.

## A host draws with a face it carries, and antialiased

*(2026-10-05, the user, reversing three earlier calls at once: "they were made
for a specific moment of the development".)* Three things were settings a
build or a launch had to ask for, each decided when the host drew meters and
waveforms and the cost of a default was the whole question: the rasterizer was
an optional feature with no face behind it ("The crate embeds no typeface,
because a face is licensed, heavy and already installed"), antialiasing was
`--msaa 1`, and a script's `text_size` was scaled by the display's factor and
drawn as it came. The host now draws applications -- a score editor whose
window is text, symbols and an engraved page -- and what a user saw by default
was a 5x7 bitmap, stepped curves, and at some display factors glyphs whose own
pixels were unequal.

**The rasterizer is a default feature, and the crate carries the face.** A
subset of DejaVu Sans -- Latin, Greek, Cyrillic, punctuation, arrows, signs;
238 KB -- is compiled into the host (`crates/clausters-gui/assets/fonts/`, with the
license it travels under and the script that makes it), and each shell loads
it as it starts. The three reasons of the first decision, as they stand now.
*Licensed*: the DejaVu license permits redistribution and modification under
its own name, and the notice is inside the file. *Heavy*: a quarter of a
megabyte against a wasm of fourteen. *Already installed*: natively, yes --
and a page has no installed faces to read, so "find one" was a native-only
answer, and one that made the same window a different picture on every
machine. The implicit search of the system's faces is gone with it: the host
is one, and its text is the same in a window and in a page. `--font` and
`/gui_font` still name another.

**Four samples is the default.** The chrome is hairlines on the pixel grid,
which multisampling leaves as they are; what it smooths is a curve of a page,
a symbol's outline, a trace. Measured on the score editor's window (1620 by
910 physical pixels) the main thread and the browser's GPU process spend the
same with four samples as with one, within the noise of the measurement.

**The bitmap is drawn on the pixel grid whatever size is asked**
(`font::bitmap_pixel`): the whole step nearest the size, centred in the cell
the size measures, so no layout moves. It is the floor now rather than the
face -- a build without the feature, a host with no face loaded -- and it is
right at every display factor instead of at the whole ones.

What it cost was measured and then paid for elsewhere: a window that shows a
playhead is repainted when its counter moves rather than thirty times a second
for as long as it is open, a page keeps the fills it tessellated, and a
triangle wholly inside its clip is not walked round it
(`crates/clausters-gui/PLAN.md`, "Found by use").

## A window keeps its picture, and a tick draws what moves

A window was laid out, meshed and uploaded whole for every frame, whatever
the frame was for: a playhead crossing a multitrack redrew its toolbar, its
boxes and its page thirty times a second, and so did a meter at rest. The
entries were two and old — "a window repaints whole for a line that moves in
one view", "what a playing frame still costs" — and each had been measured
and left, because a frame cost a millisecond. Taken 2026-10-06, which the
user counted among the important optimizations.

**What moves is a layer the element draws, not a region the frame tracks.**
The obvious designs were two. A damage rectangle per view asks the card to
keep the last picture, which a swapchain does not; and a retained closure per
moving line cannot read state its element changes on a tick. So an element
has a second draw, `draw_live`, called with the placement its last whole
frame gave it and the world as it is now, and a promise (`Needs::live`) that
its ordinary draw reads neither the clock nor a fed value. The frame keeps
those placements without the tree — the way down to each widget, its rect,
its clip, its size table — and a tick walks them alone.

**The front says what a redraw is for, and the least is drawn.** Every
request a host makes of a window names its cause: the picture changed, or
only what moves did. One is asked by a set, a gesture, a reply, a resize; the
other by the tick and by a set of a *live prop* (a clock's text). The rule
between them is the one that cannot draw a stale picture: **whole wins, and
silence is whole**. A frame the platform asked for is whole; one a tick asked
for beside a set is whole; a window with a dialog up keeps nothing, since its
lines are under the scrim and not where a live batch is drawn.

**A tick that moved nothing draws nothing.** The live mesh is compared with
the one on screen, to a quarter of a pixel — the step four samples to the
pixel can show — and a frame that would be the frame already there is not
drawn. That is what "thirty frames a second is one number for every window"
asked for: the tick still asks thirty times, and the answer is usually no.

**The card still draws the whole window.** What is kept is the describing —
the layout, the triangles, the upload — which is where a frame's time went
(measured: the score editor's host 11.2 % of a core playing, 1.1 % after;
the multitrack session with its server 9.5 % at rest, 2.8 % after). An
offscreen copy of the picture under the live layer would save the card's own
work, and nothing measured asks for it.

*Revised 2026-10-27: the card keeps the picture too.* Measured on the audio
editor playing a four-minute take, in a window of 3840 by 2071 pixels on an
integrated GPU, the card was busy 29 to 39 % of the time for a moving
playhead and a meter: every tick drew every panel, waveform and spectrogram
again into a multisampled attachment and resolved it, and the cost grew with
the window, not with what moved. A window with live layers now keeps
everything else in a texture of its own (`host::frame::underlay`), drawn when
a whole frame changed it. A frame copies it into the surface single-sampled,
and the live layers are drawn **multisampled in the boxes they cover**: each
box is the picture under it and the live layers over it, drawn into a small
atlas and resolved, then copied over the frame. Copying the whole frame
through the multisampled attachment kept the antialiasing too, at 22 to 26 %
in that window; with the boxes the card is busy 14 to 15 %, against 13 % for
the single-sampled copy alone, which is the floor, since a swapchain keeps no
picture to draw over and a presented frame is a whole one. In the editor's
default window the card went from 6 % to 2.5 % playing. The
order the eye sees does not change: the live layers sat over the window's
overlays and under the host's bands, a dialog and the open menus, so a window
with a dialog keeps nothing, and a live layer that meets a band or a menu is
drawn the single pass's way. Single-sampled the frame is the single pass's to
the bit; multisampled it differs only on the pixels where a live edge
crosses an edge of the picture, which blends over the picture's resolved
color rather than over each sample (a test bounds both). It is passes,
textures and texel loads, which every backend the host draws with has, so the
page draws it as the window does.

**The glyph atlas had to change with it.** A kept picture's text names
places in the atlas, so the atlas may not move them: a glyph's coordinates
are texels of the sheet, which the shader divides by the texture's own size,
and a sheet that grows leaves every texel where it was. A sheet packed again
is a new *epoch*, and a picture from an earlier one is drawn again. The same
pass found that each batch of a window held its own megabyte of sheet and
copied all of it for every new glyph; a window has one, as high as the atlas
has had to grow, and takes the rows that changed.

## A spectrogram is as long as its take, and the card sees a window of it

A stored spectrogram was one texture: a frame a texel wide, so a take's
transform could have no more frames than a card's widest texture, 8192. A long
file had its hop raised until it fit — five minutes at 48 kHz, asked for at a
hop of 512, came out at 1758 — and zoomed in, the picture was as coarse as
that. Zoomed out it was wrong the other way: ten frames to the pixel, and a
sampler that reads two.

**The transform keeps what the analysis made, and the view holds what it
shows.** The bound on a transform is the memory it takes (64 MiB of
magnitudes), and what is on the card is the run of columns the view is
showing, with as much again on either side, at the level of detail the screen
can resolve. A pan inside that run uploads nothing; one that leaves it, or a
zoom that changes the level, uploads the new run — at most a texture's worth,
whatever the take. Tiles were the entry's own word and are not what this is:
tiles are a fixed cut of the whole transform, most of which no frame shows.

**The levels are the transform's, and each keeps the peak.** Zoomed out, a
column of the picture stands for many frames, and a level of the pyramid holds
the largest of them per bin — the reduction a peak pyramid makes of a signal,
for the same reason: what a pixel has to show of ten frames is what happened
in them, a click included. A card's own mip chain could not be used: it halves
both axes together, and the frequency rows are not what there are too many of.

**A pixel shows a magnitude the analysis measured, and a texel larger than a
pixel is a block.** *(Revised 2026-10-27; the axis was interpolated with a
curve where it is stretched.)* A log axis gives its lowest bins tens of pixels
each, and the sampler's straight line between two rows drew each bin as a ramp
with a crease at its centre; a Catmull-Rom through four rows took the crease
away, and drew between two bins magnitudes nothing measured, its overshoots
included, with a shape the curve chose and not the signal. Every value on
screen is now a texel: the sampler is nearest, so a bin taller than a pixel is
a block as tall as its band and a column wider than one is a block as wide as
its hop, and a pixel over several texels still shows the largest. The
blocks' edges are where the analysis's resolution ends, which is what the
reader of a spectrogram has to see. It is the field's practice where a
spectrogram is read rather than looked at: a plot of a time-frequency grid
draws each value as a flat cell, a measuring view switches its smoothing off
to inspect single bins, and a zoomed-in spectrum is known to interpolate
the existing bins without resolving anything new. The same reasoning bounds
the columns: none is finer than the hop between the analysis's windows,
since one more window a few frames from the last measures nothing the grid
did not, and a column is drawn centred on the windows it was measured from
-- drawn from where they start, a click lit the block before its own.

*Revised again 2026-10-27: drawn smooth on request.* The blocks are the
default and stay what a reading of the picture is checked against, but the
curve read better as a picture, and the field offers both: a smoothing a
view turns on to look and off to measure. So a spectrogram's `interpolate`
prop draws it smooth -- the Catmull-Rom through the rows around a pixel
where a bin is taller than it, a line between two columns where a column is
wider than one -- and nothing else changes: a pixel over several texels
still shows the largest, the bins are still centred on their frequencies,
and the analysis is the same. It is a look of the display, like the colours,
and the documentation of the prop says what it draws between two bins.

**A spectrogram's overlap is a power of two.** *(2026-10-27.)* The audio
editor offered the overlap as a percent slider, 0 to 100, so a hop was any
number of frames down to one. Two things went wrong. A hop that is a
percent of the window lies neither on the grid of the coarser hops nor on
the power-of-two columns a view asks for, so the windows a pooled column
took were snapped to the grid, not on it, and every zoom asked for windows
no other zoom had analyzed. And at 100% a column of a few hundred frames
was the largest of a window every frame, hundreds of analyses for one
pixel. The overlaps offered are 0, 50, 75, 87.5 and 93.75%, each halving
the hop, which is how measuring software offers them (as fractions, since a
percent of a window is rarely a whole number of frames). Separately, a
pooled column takes its windows a quarter of a window apart at the closest:
a window's taper weighs a frame between two such windows at 85% or more, so
a closer one adds nothing a pixel shows.

**A note axis spans the notes, not a log axis's range.** *(2026-10-27.)* It
was the log axis with its floor at C0 of the tuning, so it reached from
16 Hz up to Nyquist like any log axis. A note axis is read in notes, so it
spans the MIDI notes 0 to 127 at an even step a semitone -- about 8 Hz to
12.5 kHz at A4 = 440 -- and what lies above note 127 is not on it. The
floor is note 0 at the tuning and is the one number every reader of the
axis takes (`ruler::axis_floor`: the texture, the spectrum curve, the
ruler, the gesture). The mapping of a bin onto it is the rule every scale
follows: a bin wider than a note is one block as tall as its band, many
bins under a pixel show the largest, and nothing is interpolated -- the
low octaves show the analysis's real resolution, which a longer window is
what improves.

**The analysis moved to the shared core, because a client has to write what
the host reads.** The host has always accepted a spectrogram cache and only
its own crate could make one. The window, the normalization, the pyramid and
the cache format are `clausters_core::stft`, exported over the C ABI and wasm
and bound in both clients. Their parity test compares what the analysis says
and not its bytes: the two builds take a cosine and a logarithm from two math
libraries, and may differ in the last place.

Not taken: analyzing on demand at the screen's resolution from the samples,
which would take the memory bound away. A cache holds no samples.

## A capture is the same picture on any adapter, within two levels (2026-10-16)

A window drawn offscreen and read back (`/gui_capture`) was measured on the
two adapters a machine can draw a headless host's windows with -- an
integrated card and the software one a machine with no card has (llvmpipe,
over Vulkan) -- for one window of text in the host's own typeface, a slider,
a knob and a roll's notes, at four samples and at one:

- **one adapter against itself, two runs:** identical, byte for byte;
- **the card against the software adapter:** about a third of the pixels
  differ (33% at four samples, 28% at one), and **none by more than two
  levels of 255** in any channel -- one level with the embedded bitmap face,
  two with the typeface's antialiased glyphs.

So a picture is the same picture wherever it is drawn: the difference is
rounding, not geometry, and no eye sees it. What it settles is how a picture
is compared. **Byte for byte holds only on one adapter**, which is the case a
person re-running a check on their own machine is in; a reference kept in the
repository and checked on whatever machine runs the suite is compared
**within two levels per channel**, which holds across both and still catches
anything that moved a line or changed a color. The measurement is a test that
runs by hand where both adapters are present
(`crates/clausters-gui`, `cargo test --lib -- --ignored --nocapture
software_against`), so it can be taken again when the drawing or a driver
changes.

## A node's generic editor is a GuiDef, and a binding is read back by asking (2026-10-16)

A node's generic editor could have been a new widget the host expands from a
description, with a new command that reads a control from the audio thread
and a stream that pushes the value back. Every piece of that already existed in
another form, so it is built from those instead:

- **The window is an ordinary GuiDef.** A control widget with an inline `bind`
  to `/node_set` is how any window writes a node; the editor is a generated one,
  and what decides what each control becomes is one function in
  `clausters_core::node_editor` that both clients call. The host gains no
  element. Search and paging are the client asking the core again and
  redefining the rows, so the host keeps no editor state either.
- **The description is `/def_query`'s.** Its per-control record grew the
  fields a generic view needs — range, step, warp, unit, label, group, flags and
  a list's labels — the same record in every family, built by one walk per def
  family that the query and the text commands share. A second command
  describing the same controls would have been a second walk to keep in step.
- **A binding is read back by asking, with the read form of its address.**
  `/synth_get` answers with the `/node_set` it reads, so a reply matches its
  widgets by the binding itself; the host asks every 200 ms for the bindings of
  open windows. A push would have needed a subscription per node and a second
  path for each way a value changes; the server's mirror already takes every
  one of them — a `/node_set` from any client, a plugin's own edit, a loaded
  state — so one question covers them all, and a window that changes nothing
  repaints nothing. Reading the processor instead would have answered a
  mapped control's bus value, at the cost of a command through the audio thread
  that does not answer while the audio is not running (a page whose context is
  suspended); a mapped control reads what was last set on it, and that gap is
  written down.
- **A value's text is a label bound to `/node_setText`.** A text field reports
  every keystroke, so typing `880` into one bound to a plugin would set `8`,
  then `88`; a label reports nothing, is read back like any binding, and the
  writing stays with the script (`set_text`).
- **The range is the control's when it declares one.** A SynthDef control may
  now declare `min`/`max`/`step`/`warp`/`unit`, which overturns "the range is
  the widget's" for a control that says it; one that does not still has none,
  and only the generic editor, which has to draw every control, falls back on
  the conventional range of a name (`freq`, `amp`, `pan`) and then on one
  around the default. The fields live in a control's `display`, never as
  attributes, because `min` and `max` on a signal are the binary operators.

## The pane owns the time axis, its bodies own their vertical scales (2026-10-16)

The audio editor needs annotations above a signal without giving either body
ownership of the other's axis. An ordinary layout panel can overlap children,
but each child still owns its ruler and receives presses independently. Adding
editor semantics to every layout panel would change the meaning of a layout.

A `pane` is therefore an axis container. It reuses the timeline group and the
existing body-role selector. Its ruler, selection and playhead are shared;
its active visible body receives edits. Signal visual layers stay inside the
take body: peak, RMS and spectrogram are pictures of samples, while a curve
is an independently editable structure. The distinction keeps one signal
analysis and one implementation of each curve gesture.

## A channel stack folds to strips and scrolls past a floor (2026-10-16)

A many-channel take thinned its rows until they were lines. Of the shapes the
plan named (scroll, fold, pick), folding to a strip was chosen with the user:
a folded channel stays on screen as one control's height with its name and
its peaks, so it is still there to unfold by hand. An open row has a floor of
three controls; a stack that does not fit is laid out at that height and seen
through the body, which is a scroll rather than a thinning.

The layout is one value, `host::bands::Lanes` (a 64-bit mask of folded
channels, the strip, the floor and the scroll), carried by every geometry
question a stacked view asks, so the drawing, the hit tests and the readouts
cannot disagree; with nothing folded it is the even split it replaced. The
plain wheel over the ruler scrolls and Ctrl zooms the amplitude, as the
keyboard gutter already does, because the rows are what is out of sight.
What is folded is the owner's (the audio editor's `folded`, a view state
outside the history), so a press on a strip only asks; the scroll is the
reader's alone and never reported.

## A host that cannot map draws every take from its summary (2026-10-22)

*Revised 2026-10-30, closing `S23`: the server keeps no summary and answers no `/buffer_peaks`, so a host that cannot map reads a server buffer standing still back and summarizes it itself. What this entry measured holds for an application's takes, which are lists the host draws from their files and summarizes at the level a view draws; a server buffer is what a script draws, and an edit announced by span is still read back by its span ("The server keeps no command an application's editing made").*

*Decided 2026-10-22, by the user, revising the two routes kept on
2026-08-20.* A host that cannot map a server buffer (a page, or a host attached
to a remote server) drew a short take by downloading every sample, so that
every later zoom was answered from samples in hand, and a long one from its
summary with the span under the eye read back. The line was the size, about
five seconds of stereo.

The edit is what the short route paid for. After every change the window
downloaded the whole take again: in a page, two seconds of stereo pasted onto
two was 94 `/buffer_getRange` requests and 1.5 MB, and the picture settled
about two seconds later, while a six-channel take of the same length, summarized because
it was over the line, showed the same paste at once. The cheaper case was
the one taking the expensive route.

**Every take is now drawn from its summary**, whatever its size: `/buffer_peaks`
for a take standing still, the `/buffer_stream` reports for one being written,
and samples only for the span a zoom shows, read in chunks of which three may
be in the air (the reply ring's bound). The same paste is eight requests,
59 kB and about 120 ms in headless Chrome. What it costs is one round trip the
first time a short take is zoomed to its samples, which is what a long take
always paid.

**The criterion, stated by the user:** the interface answers fast and spends
the least CPU an operation needs, and work done at the pace of the
interaction (a summary when a take is opened or edited, samples when a zoom
asks for them) is spread over what the hand does, so no operation arrives
as a spike. A route that front-loads work to save a later round trip is
measured against that, not against the round trip alone.

## A take's overview is answered from a summary kept on the buffer (2026-10-22)

*Superseded 2026-10-30, closing `S23`: `dsp::summary`, `/buffer_peaks` and the overview file beside a shared region are gone with the joins they composed ("The server keeps no command an application's editing made"). The measuring as it is asked lives on in the host's own answerer of lists (`clausters_core::peaks::Kept`).*

*Decided 2026-10-22, opening the audio editor's picture milestone (the GUI
plan's G44), which the user asked to be designed and measured with care
because every application draws through it.* `/buffer_peaks` read the samples
to answer, except for a buffer shared as a region file, whose overview it kept;
a page's engine and every join had none, so each overview was a pass over the
take and each edit's join was summarized again from its samples.

**Every buffer the server owns keeps a summary, on the buffer itself**
(`dsp::summary`, a `Mutex` only the network side takes). On the buffer rather
than in a table by buffer number, because a buffer is not always reached by its
number: a join holds its sources by `Arc`, and a source freed under a join goes
on being read through it. A server that maps another's segment keeps none,
since the samples change under it unannounced.

**It is measured as it is asked, not when the samples arrive.** The milestone
was written with the summary built at allocation; in a page the engine
answers between two audio blocks, and building the summary of four minutes of
stereo at once measured 75 ms in the wasm engine, against a 128-frame
quantum of 2.67 ms. So a summary starts unmeasured
(`clausters_core::peaks::Kept`), a question measures the base buckets it needs
and does not have, and what was measured is kept. What a reply may read is
bounded, 2^17 samples (about 0.4 ms there), so a first overview of a long take
is spread over replies and none of them is late. A question measures at least
the aligned group of 32 base buckets around what it lacks: the levels above
are recombined once per measurement, and a view asking the base grid a bucket
at a time would otherwise pay for every level per bucket (measured 127 ms for
ten minutes asked that way, 74 ms with the groups; reading the samples was
64 ms). The two statistics of a bucket are taken in two passes, as the
builder takes them: fused into one loop they ran at the pace of the energy's
`f64` sum, three times slower.

**A join keeps nothing of its own; its summary is composed from its sources'.**
Copying whole buckets of a source into a join's pyramid works only where the
part keeps the source's grid, and a paste shifts everything after it by the
pasted length, off the grid, so a stored pyramid of a join would be measured
from samples for all but its first part. The composition is done per question
instead: each part's share of the span is the source's summary over the frames
it reads, folded through the levels with the two partial edges read from
samples, so it is exact; a fade is read from the join's samples, a channel
mapped to nothing is silence, a source that is a join composes in turn, and a
part at another rate is bounded by its source over the frames it reads between
(exact extremes of the samples it interpolates, its energy taken as the join's
mean). An edit then costs the summary nothing, and the join's first overview
reads levels.

**A write is followed where it is said.** A `/buffer_*` write and a
`/buffer_touch` re-measure what was kept of their span; the audio thread's
writes (`RecordBuf`, `BufWr`) say no span, so each one counts
(`Buffer::writes`, one more per `raise_frontier`) and a summary that finds the
count moved forgets everything and is measured again as it is asked. The
region file beside a shared buffer is that summary written out; a recording
under it unlinks it until the summary is whole again.

Measured on ten minutes of stereo natively (`tests/summary_cost.rs`, release)
and on four minutes in the wasm engine (driven from node, the reply-by-reply
conversation a host has):

| | before | after |
|---|---|---|
| native, overview at a 760-column bucket, again | 62 ms | 0.1 ms |
| native, the base grid, again | 64 ms | 6 ms |
| native, a paste's join at the column bucket | 199 ms | 0.8 ms |
| native, the join at the base grid over one window | 1.4 ms | 0.5 ms |
| wasm, first overview at the column bucket, worst reply | 41 ms | 0.5 ms (warm) |
| wasm, the paste's join at the column bucket | 215 ms, worst 53 | 3 ms, worst 0.7 |

What still grows with the length is outside the summary: a native server with a
segment copies an edit's join into a region a peer maps. Filled part by part
and summarized from the region's cells, that is 183 ms for ten minutes of
stereo, from 545 ms sample by sample; it stays a pass over the take for as long
as a peer maps a join flattened (the GUI plan's "Found by use").

## A host draws a list of segments from its files, through the fetch machine it already has (2026-10-22)

*Decided 2026-10-22, building the GUI plan's `G45` for the turn to takes on
disk.* A take of the audio editor is to be a list of segments over files that
never change, drawn by the host with no server. Two shapes were open: a second
data path beside the buffer fetch, keyed by the list, or the buffer fetch
itself, answered by the host.

**The fetch machine, answered by the host.** A list gets a *take key*, a buffer
number below every one a server allocates and below the `-1` the protocol
spends on absence, and its views ask about it exactly as about a buffer —
`/buffer_query`, `/buffer_peaks` at the level they draw, `/buffer_getRange`
under a zoom — answered in the server's own reply formats. Only where a message
goes differs: a front sends a list key's to the host's own answerer. Every
behaviour the buffer path took a milestone to get right holds for a list
without being written again: asking at the drawn level and placing a picture
whole (`G44c`), keeping the picture across an edit until the new one is whole,
asking again after a reply the carrier lost. A second path would have been the
second implementation of each of them, and the first place they would drift.
The key is the host's, not the wire's: a def names the list, the host gives it
a key and keeps it across a redefine, and a new list for the same widget is
the take told to read again.

**The answerer does no I/O.** It says what it needs read and is handed the
bytes, so the native front's thread and a page, whose storage answers with
promises, drive the same one, and it is tested over real files with no front
at all. What it holds is blocks of levels and of samples, bounded and dropped
oldest first, never a file.

**A coarse column is folded from the peaks alone, and that is an
approximation, stated.** The server answers any span of a join exactly: where a
cut leaves a bucket's edge off the source's grid, it reads that partial bucket
from the samples. After a paste every column's edges are off the grid, so the
exact rule makes the overview of an edited take of hours read two partial
buckets of samples per column per channel, scattered across the file. Here a
column of 32 base buckets or more has its edges rounded to a level a
thirty-second of its width, so it draws at most a thirty-second of itself past
its own span, of the same part's material — invisible at the width it is drawn
at — and an edited take is drawn from peaks. What stays exact is what a reader
could see: a part's own edges (a cut, the end of a fade), since past a cut is
material the list does not hold; the fades, which are the list's own samples;
and every edge of a finer column, where a bucket is wide enough to be seen.

**A peaks file is trusted by its shape and its age.** It must describe the
file's frames and channels, have the size its header says, and be no more than
two seconds older than the samples, since a render closes its samples and its
peaks in either order. A file whose peaks are absent or stale is summarized
from its samples as it is asked, which is right and slow over a long file; the
application measures an imported file by a render instead.

## A list's spectrogram is analyzed where it is looked at (2026-10-22)

A take that is a list of segments over files is never read whole, so its
spectrogram cannot be analyzed whole as a buffer's is. Two shapes were open:
an analysis file kept beside each file's peaks, composed across the list, or
the span under the eye analyzed from the samples each time the view moves.

**The span under the eye.** A stored transform does not compose the way peaks
do: a bucket's minimum and maximum fold across a seam, a spectrum does not,
since a window across a seam, a fade or a resampled part is the spectrum of
samples no file holds. So a stored analysis would still read the samples there,
and it would be a second derived file per take for every writer to keep. The
field's own answer for long files is the other one: an analysis of what is
shown, switching to a fast preview above a visible length and accurate zoomed
in. Here the view asks the host's answerer for one window per column of the
span it shows (`/buffer_spectrum`, answered locally, never on the wire), read
through the list's parts so its fades and channel maps are in what is
analyzed; zoomed out a column is one window over seconds of the take, and
zoomed in the windows overlap. It asks again when it moves past what it holds
or zooms to another hop, and keeps the columns it has until the new ones have
come.

What it costs is the view's width times the window, whatever the take's
length: over an hour of stereo, 760 columns of 2048 frames read 37 MB in 87 ms
natively. A stored analysis stays a Future direction of the GUI plan, to take
only if a measurement asks for it.

*Revised 2026-10-25: the columns inside a part are kept, by file.* Analyzing
what is shown was cheap, but it made an edit's picture wait for an analysis
of the whole span again: after a cut or a paste the view kept the take as it
was until every column had been read and analyzed, so the picture did not
follow the edit, which is the one thing an editor's picture must do. The
answerer now keeps a **file's** columns in memory, as the peaks are a file's:
one window of a source channel at a point of a grid of the hop laid over the
file's own frames. A list's column whose window lies inside one part, clear
of its fades and at the list's rate, is that file's column at the nearest
grid point, so an edit that moves a part finds its columns again and only the
windows over the new seams are read and analyzed. The reasoning above still
holds where it was about: a window across a seam, a fade or a resampled part
is not in any file, and it is analyzed through the list, exactly. What it
trades is up to half a hop of offset for a column inside a part, at a hop of
about a column of the view. The columns are kept in memory, bounded; keeping
them on disk beside the peaks is the GUI plan's Future direction.

*Revised 2026-10-25, again: a column is what happened in its frames, and a
coarse one is read from a summary on disk.* One window per column was a
preview, and a wrong one: zoomed out, a column of seconds was the spectrum of
the 21 ms at its start, so a click or a note between two windows was not in
the picture at all, the aliasing a waveform drawn from one sample a pixel
would have. A column is now each bin's **largest** over the windows every
hop inside it, as a waveform's pixel is the extremes of its samples, which is
the standard reduction for a long file's spectrogram (a peak cache over
columns, time reduced, bins kept). That costs every window of the span, so a
coarse column is read from a **spectral summary** kept beside the file's
peaks (`clausters_core::spectra`): the windows of the time-frequency view's
own analysis (1024 frames every 512), one column for every 16 of them each
bin the largest, and levels halving that, a byte a bin. Three choices in it
are not obvious:

- **What is kept is not the transform.** A column per window takes about as
  many bytes as the samples it describes, which is no summary. The finest
  level is 16 windows, 8192 frames, wide: an eighth of a byte a frame and a
  channel with its levels, 43 MB beside an hour of stereo (whose peaks take
  32 MB and whose samples 1.4 GB as floats). A view finer than that
  analyzes the windows it shows, which costs what it shows, and keeps them
  in memory as before.
- **It is written by `PeaksOut`, not by a UGen of its own.** It is the same
  file's overview, made where the samples are made for the reason the peaks
  are, by every render that writes peaks and the one that measures an opened
  file. A UGen of its own would have been a new surface in both clients'
  catalogs and in every path a render drains its outputs through, for a file
  no script names. The audio thread folds the windows (the core's transform is
  on the stack, so nothing is allocated) and pushes a column every 8192
  frames.
- **A coarse column is assigned the summary's columns whose middle is in it**,
  so each is in exactly one column of the view and is off by at most half its
  width, which the threshold (a column of the view at least as wide as one of
  the summary) keeps under half a pixel. Where a view column is dozens of
  summary columns wide its edges are rounded to a level an eighth of it, as
  a coarse column of peaks is, so it reads the levels it is drawn at: an
  hour of stereo cut into 21 parts is drawn 760 columns wide in 7 ms from
  2.5 MB of the summaries, where analyzing a window a column read 37 MB of
  samples in 87 ms. A fade lowers what its part gives by its largest gain in
  the column; a part at another rate is in no summary and its column is
  analyzed.

The summary is of one analysis. A view analyzing with another window or hop
draws every column from the samples, pooled the same way; a summary per
analysis was not worth a file each.

*Revised 2026-10-27: another analysis is asked where it is looked at.* The
whole take used to be asked first after any change of analysis, as after an
edit. For the summary's analysis that is a few milliseconds; for any other it
reads and analyzes every frame of the take, 0.5 to 1.3 s of a four-minute
stereo take, and a zoomed view waited for it with the old picture on screen.
A setting dragged through a dozen values queued that whole analysis a dozen
times, one after the other, for seconds after the hand stopped. Now the span
shown is asked first and the whole take only by a view that shows it, and an
ask of an analysis the view no longer wants is let go at its next answer
(`host::columns`); after an edit the whole take is still asked first, since
the answerer holds most of its windows and the old picture is of the old
length.

*Revised again 2026-10-27: the analyses left are kept.* Going back to an
analysis tried a moment ago analyzed the whole take again, 0.4 to 0.7 s of
the same take each time. A view now keeps the pictures of the whole take by
the last two analyses it left (`SpectrogramView::shelve`), a column a pixel
or so each, and one that comes back to one of them draws it at once and asks
nothing for it: over the whole take it becomes the view, over a span the
overview under it. None is kept across an edit or into a take of another
length. They are kept in memory for as long as the view is: a store per
analysis beside the file's spectral summary would survive reopening the take,
at the size of a summary per analysis, and is left until reopening long takes
with another analysis is what is measured to cost.

## The host ships the faces it shapes (2026-11-08)

*Taken in `G43a` (`crates/clausters-gui/PLAN.md`).* Shaping Arabic, Hebrew and
Devanagari needs faces that draw them with their layout tables, and the plan
left open whether the host bundles them or reaches a system face natively and
a fetched one in a page through `FontSource`. Both are bundled: DejaVu Sans
grows by Arabic, Hebrew and its layout tables to under 300 KB, and a subset of
Noto Sans Devanagari of under 150 KB sits beside it as the face the host falls
back to, chosen grapheme by grapheme. Reaching a system face would make what a
window draws depend on the machine and leave a page with a fetch the window does
not need, and the acceptance's tests shape against the face the host ships; the
cost is about a megabyte of wasm with the shaper, measured in the plan. A face a
client loads replaces only the first, so the fallbacks stay under it.
