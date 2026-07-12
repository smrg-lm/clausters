# Glossary

Every term of art this section uses, pinned. The vocabulary is the code's own
— these are the words the docstrings, the design records and the tutorial all
share. Each entry links to the page that develops it.

**arrangement model** — the client-side layer that places materials in time,
groups them recursively and realizes them: `clausters.model`. Pure and
transport-agnostic; the server knows nothing of it. Its DAW-style view is the
[multitrack editor](#multitrack-editor). *Not* called "the model" bare (that
reads as the node tree or a def), and not "the material model" — material
names the contents, not the layer.

**automation / automation lane** — a break-point curve placed on the timeline
that drives controls: `clausters.seq.Automation`. Stored as an
[`Env`](#env); realized as a [control vector](#control-vector) read onto a
[control bus](#control-bus). ([Automation](automation.md))

**base level** — the zoom at which a nested group is *summarized* (one labeled
rectangle) or *resolved* (lanes of its own): `Editor.expand` / `collapse`. The
same structure, seen coarser or finer — a view state, not data.
([The editor](editor.md))

**beats** — the model's unit of time: musical, tempo-relative. Everything in
`clausters.model` — onsets, durations, placements, `extent` — is in beats.
Compare [timeline samples](#timeline-samples). ([Overview](../composing.md))

**bounce** — evaluating a generator offline into concrete material: a pattern
run on a throwaway clock into a timeline of events (at flatten time), or a
whole piece rendered to audio ([Bouncing](bounce.md)). A bounce is a
[change of state](#change-of-state).

**bus (internal)** — a private audio/control bus a [logical group](#group)
declares (`buses=["mix"]`); members wire to it by naming it in their
`controls`. Each graph instance allocates its own. The reserved name `OUT` is
the hardware output. ([The logical side](logical.md))

**change of state** — evaluating a [generator](#generated--generator) into
[generated](#generated--generator) material: a pattern bounced to events, a
piece rendered to a file. The compositional act realization performs — it
turns something you can only *produce* into something you can *manipulate*.
([Materials](materials.md))

**clip** — the editor's graphic unit: a placed rectangle spanning
`[offset, offset + dur]` on the shared axis — *its length is its duration*.
Draws the body (or layered bodies) its material calls for: a take, a
piano-roll, a curve. ([The editor](editor.md))

**control bus** — a server bus carrying control-rate values. The realization
target of an automation: a small internal synth reads the
[control vector](#control-vector) onto the bus, and targets follow it (via
`/n_map`, or by reading the bus directly). ([Automation](automation.md))

**control vector** — an automation curve discretized into a server control
buffer (`/b_gen "env"`, the same envelope math `EnvGen` plays — what is drawn
is what is heard). ([Automation](automation.md))

**cursor** — the *static* transport line: where a located, stopped transport
sits, and where the next `play` starts. Set by `locate` or a ruler click.
Compare [playhead](#playhead-sweeping). ([Editing](editing.md))

**dirty** — `Editor.dirty`: the model changed since the last realization. An
edit never interrupts what is sounding; the next transport action re-reads the
composition. ([Editing](editing.md))

**duration** — a material's own length in beats (`Material.duration`),
optional. Distinct from a [placement](#placement)'s `dur`, which overrides and
[trims](#placement-length-trim) it. ([Materials](materials.md))

**edit-back** — the GUI-to-model direction of the loop: the window's gestures
(`"clip"` move/resize, `"points"` curve edits, `"wire"` rewiring, `"locate"`)
applied onto the model by `Editor.apply` / `Editor.poll`.
([Editing](editing.md))

**Env** — the client's envelope object (levels, segment times, per-segment
shapes): the stored form of an automation curve, round-tripped to and from
break-points by `env_to_points` / `points_to_env` — the picture, the model and
the server buffer all read the same object. ([Automation](automation.md))

**event / clip (material)** — the `Event` primitive: parameters grouped into
one action, internally simultaneous. Wraps `clausters.seq.Event`.
([Materials](materials.md))

**extent** — the composition's length in beats, *read from the model* (the end
of its last placed material): `Editor.extent()`. Not a constant — move a clip
past the end and the piece is longer. ([The editor](editor.md))

**five primitives** — the five material kinds, each a thin adornment over an
object the client already has, with their conceptual names: `Event`
(*event/clip*), `Sequence` (*List*), `Buffer` (*Buffer*), `Track` (*Set*),
`Generator` (*Function*). ([Materials](materials.md))

**flatten** — the tree-walk that accumulates nested placement offsets into
absolute beats, producing a flat timeline of playable items; contained
patterns are bounced in the same pass. `clausters.model.flatten` /
`to_timeline` — also available as pure inspection. ([Grouping](grouping.md))

**follow** — `Editor.follow`: re-realize on every applied edit (the *live
editor*). Off, an edit marks [dirty](#dirty) and waits for the next transport
action. ([Editing](editing.md))

**generated / generator** — the two modes of a material, the axis the layer
turns on. *Generated*: the rendered thing — random-access data you can edit,
slice, read backwards (a buffer, a timeline of events). *Generator*: the
algorithm that renders it — forward-only, it can just be evaluated (a pattern,
a def). Between them, the [change of state](#change-of-state).
([Materials](materials.md))

**group** — the model's one new structure: a composite material placing
members recursively by offset. Two *kinds*: **compositional** (a structural/
temporal relation — a section, a lane) and **logical** (a processing relation
— members wired through buses, realized as a [GraphDef](#graphdef)).
([Grouping](grouping.md), [The logical side](logical.md))

**GraphDef** — the server's named configuration of member nodes wired by
buses, sent with `/d_graph` and instanced with `/graph_new`. The logical
realization: `Group.to_graphdef()` maps a logical group onto one, 1:1.
([The logical side](logical.md))

**handle (member)** — the stable object `Group.add` returns (also
`Group.handles`), identifying one placement across edits — what `move` and
`remove` take, and what the editor holds per clip. `Group.members` reads the
placements as `(offset, dur, material)` triples. ([Grouping](grouping.md))

**instrument** — the def named to play a `Buffer` material (its `buf` control
takes the buffer number). A buffer is *data*: without an instrument it is
structure only — it draws and contributes extent, but emits no event.
([Materials](materials.md))

**lane** — one `track` row of the multitrack window. The root group's members
are the lanes; a lane's members are its [clips](#clip).
([The editor](editor.md))

**locate** — seek: put the transport at a beat. Stopped, it moves the
[cursor](#cursor); playing, it re-realizes from there. A click on a lane's
ruler or empty space is the same locate. ([Editing](editing.md))

**material** — the model's unit: any bounded thing that produces a unit of
meaning and can be decomposed or combined — in one of the two modes,
[generated or generator](#generated--generator), carrying an optional `onset`
and `duration`, delegating its playing to the object it wraps.
([Materials](materials.md))

**multitrack editor** — the arrangement model's DAW-style view and driver:
`clausters.gui.Editor` plus the `track`/`clip`/`graph` widgets. Renders the
tree, applies edits back onto it, owns the transport, and is the *only*
converter between [beats](#beats) and [timeline samples](#timeline-samples).
([The editor](editor.md))

**onset** — where a material starts, in beats, relative to its context;
optional. Usually supplied by a [placement](#placement) rather than the
material itself. ([Materials](materials.md))

**patch / patcher** — the view of a [logical group](#group): member boxes,
bus nodes, one wire per `(member, control) ↔ bus` connection. Deliberately
*undirected* — the data knows the connection; the direction is the server's
analysis. ([The logical side](logical.md))

**piano-roll** — the clip body a material of events draws: one bar per note,
high pitches up. A pattern's roll is *bounced to be drawn* — a generator lane
shows the notes it is about to play. ([The editor](editor.md))

**placement** — one member's position in a group: an `offset` (beats, relative
to the group) and an optional `dur`. A material's concrete place comes from
its placement, not from itself. ([Grouping](grouping.md))

**placement length (trim)** — a placement's `dur` overrides the material's own
duration and *trims* what plays: events past the end are dropped, a final
event is shortened — on a copy; the material is never rewritten. The DAW rule:
a clip's length is what you hear of it. ([Grouping](grouping.md))

**playhead (sweeping)** — the moving transport line, anchored to the engine's
sample clock so it tracks the audio (`Editor.anchor`). Also the
`clausters.seq.Playhead` object itself: the transport realization returns.
Compare [cursor](#cursor). ([Realization](realization.md),
[Editing](editing.md))

**poll** — `Editor.poll()`: drain the window's pending events into the model
(apply each), returning whether the composition changed. One call, no loop;
never from the clock thread. ([Editing](editing.md))

**quant** — the one musical grid, in beats: the editor converts it once and
hands it to the lanes as their drag grid, and snaps edit-backs to it — so the
grid a clip is dropped on is the grid the model re-schedules on.
([The editor](editor.md))

**realization / realize** — the change of state to sound. *Compositional*:
flatten to a timeline, play through a `Playhead` — and every realize re-reads
the model. *Logical*: translate to a [GraphDef](#graphdef), send and instance.
RT or NRT purely by destination. ([Realization](realization.md))

**re-realize** — re-schedule the (edited) composition from the playhead's
position: stop, re-flatten, play. Honest semantics: *re-schedule from here*,
not a sample-exact splice — a synth already sounding keeps sounding.
([Editing](editing.md))

**RT / NRT** — real-time (a live server, timetagged bundles) versus
non-real-time (an offline score, `Session.nrt` + `render`). The same client
code and the same flattening either way — which is why a bounce is
**sample-identical** to what you heard. ([Bouncing](bounce.md))

**take** — an audio clip: a `Buffer` material's recorded/bounced content, drawn
from the server buffer itself (fetched and decimated host-side).
([Setup](setup.md), [The editor](editor.md))

**temporal character** — what a single material's `onset`/`duration` presence
makes it: **segment** (both), **punctual** (onset only), **relative**
(duration only), **abstract** (neither — pure context).
([Materials](materials.md))

**temporal relation** — what a group's members' placements make the group:
**successive** (they tile contiguously), **simultaneous** (they start and end
together — one thing on the timeline, drawn as one layered clip), **mixed**
(anything else). Derived, never declared. ([Grouping](grouping.md))

**timeline samples** — the view's unit: one unit per audio sample, so a take
sits 1:1 on the axis. Clips, rulers and edit-backs speak it; the editor
converts (one beat = `sample_rate / tempo` units) and nothing else does.
([The editor](editor.md))

**unit bridge** — that one conversion, beats ↔ timeline samples, owned
entirely by the editor (`units_per_beat`, `beats_to_units`, `units_to_beats`),
through the core's own time arithmetic. ([The editor](editor.md))

**wire** — one `(member, control) ↔ bus` connection of a patch. Rewiring on
screen rewrites the member `Generator`'s controls; the next realization sends
the graph as drawn. ([The logical side](logical.md))
