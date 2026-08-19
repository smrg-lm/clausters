# Glossary

Every term of art this section uses, pinned. The vocabulary is the code's own
— these are the words the docstrings, the design records and the tutorial all
share. Each entry links to the page that develops it.

**aggregate** — the arrangement's one new structure: a composite element
placing members recursively by offset (`Aggregate`). Two *kinds*: **concrete**
(the members relate in time — a section, a lane) and **logical** (the members
relate by processing — wired through buses, rendered as a
[GraphDef](#graphdef)). Not the server's `Group`, which is a node of the node
tree and knows nothing of the arrangement.
([Grouping](grouping.md), [The logical side](logical.md))

**arrangement** — the client-side layer that places [elements](#element) in
time, groups them recursively and renders them: `clausters.form`. Pure and
transport-agnostic; the server knows nothing of it. Its DAW-style view is the
[multitrack editor](#multitrack-editor). Called *the arrangement* (or *the
arrangement model* when naming the layer as such) — never "the model" bare,
which reads as the node tree or a def.

**automation / automation lane** — a break-point curve placed on the timeline
that drives controls: `clausters.seq.Automation`. Stored as an
[`Env`](#env); rendered as a [control vector](#control-vector) read onto a
[control bus](#control-bus). ([Automation](automation.md))

**base level** — the zoom at which a nested aggregate is *summarized* (one
labeled rectangle) or *resolved* (lanes of its own): `Editor.expand` /
`collapse`. The same structure, seen coarser or finer — a view state, not data.
([The editor](editor.md))

**beats** — the arrangement's unit of time: musical, tempo-relative. Everything
in `clausters.form` — onsets, durations, placements, `extent` — is in beats.
Compare [timeline samples](#timeline-samples). ([Overview](../composing.md))

**bounce** — evaluating a generator offline into a generated element: a pattern
run on a throwaway clock into a timeline of events (at flatten time), or a
whole piece rendered to audio ([Bouncing](bounce.md)). A bounce is a
[change of state](#change-of-state).

**bus (internal)** — a private audio/control bus a
[logical aggregate](#aggregate) declares (`buses=["mix"]`); members wire to it
by naming it in their `controls`. Each graph instance allocates its own. The
reserved name `OUT` is the hardware output. ([The logical side](logical.md))

**change of state** — evaluating a [generator](#generated--generator) into a
[generated](#generated--generator) element: a pattern bounced to events, a
piece rendered to a file. The compositional act [rendering](#render--rendering)
performs — it turns something you can only *produce* into something you can
*manipulate*. ([Elements](elements.md))

**clip** — the editor's graphic unit: a placed rectangle spanning
`[offset, offset + dur]` on the shared axis — *its length is its duration*.
Draws the body (or layered bodies) its element calls for: a take, a
piano-roll, a curve. ([The editor](editor.md))

**concrete (aggregate kind)** — see [aggregate](#aggregate): an aggregate whose
members relate *in time* (a section, a lane), as opposed to a **logical** one,
whose members relate by *processing*. It is the default kind, and the one the
multitrack draws as lanes of clips. ([Grouping](grouping.md))

**control bus** — a server bus carrying control-rate values. What an automation
renders onto: a small internal synth reads the
[control vector](#control-vector) onto the bus, and targets follow it (via
`/node_map`, or by reading the bus directly). ([Automation](automation.md))

**control vector** — an automation curve discretized into a server control
buffer (`/buffer_gen "env"`, the same envelope math `EnvGen` plays — what is drawn
is what is heard). ([Automation](automation.md))

**cursor** — the *static* transport line: where a located, stopped transport
sits, and where the next `play` starts. Set by `locate` or a ruler click.
Compare [playhead](#playhead-sweeping). ([Editing](editing.md))

**dirty** — `Editor.dirty`: the arrangement changed since the last render. An
edit never interrupts what is sounding; the next transport action re-reads the
composition. ([Editing](editing.md))

**duration** — an element's own length in beats (`Element.duration`),
optional. Distinct from a [placement](#placement)'s `dur`, which overrides and
[trims](#placement-length-trim) it. ([Elements](elements.md))

**edit-back** — the GUI-to-data direction of the loop: the window's gestures
(`"clip"` move/resize, `"points"` curve edits, `"wire"` rewiring, `"locate"`)
applied onto the arrangement by `Editor.apply` / `Editor.poll`.
([Editing](editing.md))

**element** — the arrangement's unit: any bounded thing that produces a unit of
meaning and can be decomposed or combined — in one of the two modes,
[generated or generator](#generated--generator), carrying an optional `onset`
and `duration`, delegating its playing to the object it wraps.
([Elements](elements.md))

**Env** — the client's envelope object (levels, segment times, per-segment
shapes): the stored form of an automation curve, round-tripped to and from
break-points by `env_to_points` / `points_to_env` — the picture, the data and
the server buffer all read the same object. ([Automation](automation.md))

**extent** — the composition's length in beats, *read from the arrangement* (the
end of its last placed element): `Editor.extent()`. Not a constant — move a clip
past the end and the piece is longer. ([The editor](editor.md))

**five primitives** — the five element kinds, each a thin adornment over an
object the client already has, with their conceptual names: `Clang`
(*event/clip*), `Sequence` (*List*), `Vector` (*buffer*), `Track` (*Set*),
`Generator` (*Function*). ([Elements](elements.md))

**flatten** — the tree-walk that accumulates nested placement offsets into
absolute beats, producing a flat timeline of playable items; contained
patterns are bounced in the same pass. `clausters.form.flatten` /
`to_timeline` — also available as pure inspection. ([Grouping](grouping.md))

**follow** — `Editor.follow`: re-render on every applied edit (the *live
editor*). Off, an edit marks [dirty](#dirty) and waits for the next transport
action. ([Editing](editing.md))

**generated / generator** — the two modes of an element, the axis the layer
turns on. *Generated*: the rendered thing — random-access data you can edit,
slice, read backwards (a buffer, a timeline of events). *Generator*: the
algorithm that renders it — forward-only, it can just be evaluated (a pattern,
a def). Between them, the [change of state](#change-of-state).
([Elements](elements.md))

**GraphDef** — the server's named configuration of member nodes wired by
buses, sent with `/def_send graph` and instanced with `/graph_new`. What a logical
aggregate renders to: `Aggregate.to_graphdef()` maps one onto it, 1:1.
([The logical side](logical.md))

**handle (member)** — the stable object `Aggregate.add` returns (also
`Aggregate.handles`), identifying one placement across edits — what `move` and
`remove` take, and what the editor holds per clip. `Aggregate.members` reads
the placements as `(offset, dur, element)` triples. ([Grouping](grouping.md))

**instrument** — the def named to play a `Vector` element (its `buf` control
takes the buffer number). A vector is *data*: without an instrument it is
structure only — it draws and contributes extent, but emits no event.
([Elements](elements.md))

**clang (element)** — the `Clang` primitive: parameters grouped into one
action, internally simultaneous — the sonority sense of the word. Wraps a
`clausters.seq.Event`; element and event keep distinct names so that neither
reads as the other. ([Elements](elements.md))

**lane** — one `track` row of the multitrack window. The root aggregate's
members are the lanes; a lane's members are its [clips](#clip).
([The editor](editor.md))

**locate** — seek: put the transport at a beat. Stopped, it moves the
[cursor](#cursor); playing, it re-renders from there. A click on a lane's
ruler or empty space is the same locate. ([Editing](editing.md))

**logical (aggregate kind)** — see [aggregate](#aggregate): an aggregate whose
members relate by *processing* — wired to each other through buses — rather than
in time. It does not flatten; it renders to a [GraphDef](#graphdef), and it
draws as a [patch](#patch--patcher). ([The logical side](logical.md))

**multitrack editor** — the arrangement's DAW-style view and driver:
`clausters.gui.Editor` plus the `track`/`clip`/`patch` widgets. Draws the
tree, applies edits back onto it, owns the transport, and is the *only*
converter between [beats](#beats) and [timeline samples](#timeline-samples).
([The editor](editor.md))

**onset** — where an element starts, in beats, relative to its context;
optional. Usually supplied by a [placement](#placement) rather than the
element itself. ([Elements](elements.md))

**patch / patcher** — the view of a [logical aggregate](#aggregate): a box per
member, **directed and typed** — inlets on top, outlets on the bottom, a cord
per `outlet -> inlet` connection (the buses are not drawn — a cord *is* a bus).
Direction is read from the def (an `In` control is an inlet, an `Out` an outlet),
so the picture reads as signal flow. ([The logical side](logical.md))

**piano-roll** — the clip body an element of events draws: one bar per note,
high pitches up. A pattern's roll is *bounced to be drawn* — a generator lane
shows the notes it is about to play. ([The editor](editor.md))

**placement** — one member's position in an aggregate: an `offset` (beats,
relative to the aggregate) and an optional `dur`. An element's concrete place
comes from its placement, not from itself. ([Grouping](grouping.md))

**placement length (trim)** — a placement's `dur` overrides the element's own
duration and *trims* what plays: events past the end are dropped, a final
event is shortened — on a copy; the element is never rewritten. The DAW rule:
a clip's length is what you hear of it. ([Grouping](grouping.md))

**playhead (sweeping)** — the moving transport line, anchored to the engine's
sample clock so it tracks the audio (`Editor.anchor`). Also the
`clausters.seq.Playhead` object itself: what a render returns. Compare
[cursor](#cursor). ([Rendering](render.md), [Editing](editing.md))

**poll** — `Editor.poll()`: drain the window's pending events into the
arrangement (apply each), returning whether the composition changed. One call,
no loop; never from the clock thread. ([Editing](editing.md))

**quant** — the one musical grid, in beats: the editor converts it once and
hands it to the lanes as their drag grid, and snaps edit-backs to it — so the
grid a clip is dropped on is the grid the arrangement re-schedules on.
([The editor](editor.md))

**render / rendering** — the change of state to sound, `Element.render`. What
it does depends on the [aggregate](#aggregate) **kind**. For a **concrete**
aggregate (a relation in time): flatten to a timeline and play it through a
`Playhead` — and every render re-reads the tree. For a **logical** aggregate (a
relation of processing): translate to a [GraphDef](#graphdef), send it, instance
it. RT or NRT purely by destination. ([Rendering](render.md))

**rerender** — `Editor.rerender()`: re-schedule the (edited) composition from
the playhead's position, reusing the destination and clock the last `render`
remembered — stop, re-flatten, play. Honest semantics: *re-schedule from here*,
not a sample-exact splice; a synth already sounding keeps sounding.
([Editing](editing.md))

**RT / NRT** — real-time (a live server, timetagged bundles) versus
non-real-time (an offline score, `Session.nrt` + `render`). The same client
code and the same flattening either way — which is why a bounce is
**sample-identical** to what you heard. ([Bouncing](bounce.md))

**take** — an audio clip: a `Vector` element's recorded/bounced content, drawn
from the server buffer itself (fetched and decimated host-side).
([Setup](setup.md), [The editor](editor.md))

**temporal character** — what a single element's `onset`/`duration` presence
makes it: **segment** (both), **punctual** (onset only), **relative**
(duration only), **abstract** (neither — pure context).
([Elements](elements.md))

**temporal relation** — what an aggregate's members' placements make it:
**successive** (they tile contiguously), **simultaneous** (they start and end
together — one thing on the timeline, drawn as one layered clip), **mixed**
(anything else). Derived, never declared. ([Grouping](grouping.md))

**timeline samples** — the view's unit: one unit per audio sample, so a take
sits 1:1 on the axis. Clips, rulers and edit-backs speak it; the editor
converts (one beat = `sample_rate / tempo` units) and nothing else does.
([The editor](editor.md))

**unit bridge** — the single conversion between [beats](#beats) and
[timeline samples](#timeline-samples), owned entirely by the editor
(`units_per_beat`, `beats_to_units`, `units_to_beats`), through the core's own
time arithmetic. ([The editor](editor.md))

**vector (element)** — the `Vector` primitive: a list at constant time
(samples). Wraps a `clausters.defs.Buffer` — the server buffer holds the
samples, the element places a window onto it. ([Elements](elements.md))

**wire** — one `(member, control) ↔ bus` connection of a patch. Rewiring on
screen rewrites the member `Generator`'s controls; the next render sends the
graph as drawn. ([The logical side](logical.md))
