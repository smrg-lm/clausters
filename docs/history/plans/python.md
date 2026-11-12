# Plan — High-level clients for Clausters, with a shared native Rust core: the closed tracks

The tracks of [`clients/python/PLAN.md`](../../../clients/python/PLAN.md) whose every milestone shipped, moved here
whole on 2026-11-12 so that the plan holds what is open. They are the record
of what each track decided and built; a track is not reopened here, and
work that changes what one built is written in the plan.

## Milestones (client "C" track, parallel to the server "M" track)

> Markers: **✅ done** · **⏳ pending** · milestone **unmarked** = future, not started.

- ✅ **C0 — Workspace + core + FFI**: convert to a workspace; create `clausters-core` (builtins, tempoclock, rng, osc) and `clausters-ffi` (C-ABI + version); refactor the server's native ops to consume `clausters-core`. Server↔core numeric parity (bit-exact native, documented tolerance vs Faust); RT-safety intact.
- ✅ **C1 — Client scaffold + accessible core**: `pyproject.toml`, `clausters/` package, relocate transport, `_native.py` (ctypes over `clausters-ffi`). Smoke: builtin, `TempoClock`, OSC bundle, `render()`.
- ✅ **C2 — base**: `absobject`/`builtins`, `stream` (Routine/Stream with `yield`), `main`, `clock`, `netaddr`, and the swappable `_oscinterface`/`_midiinterface` targets so clock and routines emit against an interface.
- ✅ **C3 — Faust-first defs**: `signals.py` (lowercase callables mapping the Faust Signal API → JSON signal tree), `faustdef` (the three `/def_send faust` forms + controls), and the `node`/`bus`/`buffer`/`server` allocators. E2E vertical slice: `signals` → `faustdef` → `/def_send faust` → `/synth_new` → control.
- ✅ **C4 — Refactor: client/server separation** (post-C3 correction): pull communication out of `TempoClock` into `Server`. The clock keeps only timing; the `Server` owns the one **communication interface** (RT: UDP, later shm/embed; NRT: score → `render()`) and emits, reading time from the running routine's clock. A transport change is a new interface on the `Server`, never a change to clock/seq.
- ✅ **C5 — seq**: `event`, `pattern`, stream-patterns; one `Pbind`+`TempoClock`+`Server` runs RT or NRT just by changing the `Server`'s interface. Design points that hold as invariants: beats advance **only via `yield`** (the monotonic clock only computes sleeps → exact relational timing); `main.current_tt` is **thread-local** so several clocks and RT-beside-NRT coexist in one script without clobber; a selectable timebase (`MonotonicTimebase` vs `SampleClockTimebase` emitting `/sched_at`); a byte-identical score-parity golden; and `Session` as the explicit no-globals context. The C5 leftover closed the **instance-based** UGen graph (`defs/ugens.py` + `defs/synthdef.py` → `/def_send synth`), byte-identical to the internal `default`.
- ✅ **C6 — UDP sample-clock anchoring**: `defs/clocksync.py` — a `SampleClockModel` (least-squares fit over a sliding window of `/clock_query` anchors) and `UdpSampleClock` (background anchor/track), so `SampleClockTimebase` works live over UDP without shm/embed and the `Server` emits via `/sched_at` anchored to the server's clock.
- ✅ **C7 — MIDI interfaces** (re-planned → **C11**): the first sketch (MIDI 1.0 in a Python library) was poorly planned and redone as a reusable native crate for client+server (MIDI 2.0/UMP); moved out of the sequential track to C11.
- ✅ **C8 — TCP interface** (both ends): server `--tcp` accepts length-prefixed OSC (scsynth framing) multiplexed in the single-thread loop with no async runtime, `ClientId::Tcp` routing replies per connection; client `OscTCPInterface` is a UDP drop-in with reply reassembly. Timing still rides timetags/`/sched_at`, so arrival latency never affects when a scheduled command fires.
- ✅ **C9 — multi-language + close-out**: documented the cross-language architecture (`docs/clients.md` — the single C-ABI contract, the Python layers, the path to a JS client) and a commented `examples/sequencing.py` tour of the sequencing layer across the NRT/live seam. Confirms the boundary is not Python-specific.
- ✅ **C10 — Documentation and examples maintenance**: keep the mdBooks, `clients/python/README.md`, the smoke checklists and the examples current as milestones land (an ongoing duty, not a one-time task).

### The arrangement model + the multitrack editor (client arc, phased)

The recursive-granularity composition/editor track: a client-side **arrangement
model** (`clausters.form`) — elements placed in time, grouped recursively,
rendered onto the server (RT/NRT) — a GUI multitrack widget, and the bridge
between them. (*Element* is the layer's general term for its contents:
everything is an element, in one of two modes — *generated* data, or the
*generator* that renders it.) Each phase ships incrementally.

- ✅ **C23 — Automation as a control vector** (editor Fase 0 B): a break-point curve is discretized into a control buffer on the server (`/buffer_gen "env"`, reusing `clausters_core::envshape`) and read back onto a control bus by a lane synth (the new `OutCtl` UGen, symmetric to `InCtl`). Client `seq.automation.Automation` prepares the buffer without blocking the clock and `play()`s the lane + `/node_map` + free; the List↔Buffer duality of the arrangement made playable/editable, reusing the bpf editor (G21) for the curve. **`seq.automation` was removed on 2026-09-20** and what survives of this milestone is `OutCtl`, `/buffer_gen "env"` and the curve as a datum, which is `Bpf` now: see "`seq.Automation` is three things fused" in Found by use for what the class was carrying and where each half went.
- ✅ **C24 — High-level buffer I/O wrappers** (editor Fase 0 A): `Server` gains `read_buffer`/`read_into`/`write_buffer`/`zero_buffer`/`gen_buffer` (score-aware) and the synchronous `query_buffer`/`get_samples` over the existing `/buffer_*` surface — the audio-clip element (load from file, generate, query, fetch, export). No server change; buffers are loaded or generated, never push-filled.
- ✅ **C25 — Arrangement model core** (editor Fase 1A): `clausters.form` — a `Element` base carrying temporal metadata (onset/duration and the derived temporal *character*), the five primitives (`Clang`/`Sequence`/`Vector`/`Track`/`Generator`) as **thin wrappers** delegating `play(destination)` to the objects the client already has, and `Aggregate` (concrete/logical) placing elements recursively by offset with the derived temporal *relation* (successive/simultaneous/mixed). Pure and transport-agnostic; rendering onto the server/NRT is the next phase.
- ✅ **C26 — Concrete rendering** (editor Fase 1B): `clausters.form.render` — the *change of state* to sound. An `Aggregate{concrete}` is **flattened** (a tree-walk accumulating nested placement offsets into absolute beats) into a flat `Timeline` whose items follow the `play(destination)` seam; a contained event pattern is *bounced* in the same pass (reusing `Timeline.from_pattern`), a `Track` is shifted, an abstract element yields no event. `Element.render(destination, clock)` plays that timeline through a `Playhead` — RT or NRT, sample-identical, no new path. Tests: pure flatten of nested offsets, and NRT byte-equivalence to a hand-built timeline. `Aggregate{logical}` rendering, a `Vector` clip and def instancing are deferred to Fase 1C / later.
- ✅ **C27 — Logical rendering → GraphDef** (editor Fase 1C, closes Fase 1): a `Group{logical}` translates 1:1 onto a `clausters.defs.GraphDef` (the bus-wired configuration the server already expresses) — each `Generator` member becomes a wired member (its `def_name`, `controls` — numbers, an internal bus name, or `"OUT"` — and `maps` for `/node_map`), the group's `buses` the private internal buses. `Group.to_graphdef()` builds it (reusing GraphDef, not reimplementing) and `render()` sends (`/def_send graph`) and instances it (`/graph_new`) on the server. The bidirectional edit-back stays for Fase 3. Test: a two-node source→sink group yields the same GraphDef spec as the hand-built one, plus render routing.

- ✅ **C28 — The multitrack editor driver: the arrangement → GuiDef** (editor Fase 3A): `clausters.gui.editor.Editor` — the forward half of the bridge. It renders a `clausters.form` tree into the multitrack view: the root `Aggregate`'s members are the **lanes**, a lane's members its **clips** (a `Vector` names its server buffer and spans its frames; an element of clangs draws a **piano-roll**, bouncing a contained pattern in the same pass — the *change of state* made visible; a nested `Aggregate` draws as the labeled rectangle that summarizes it, until `expand`ed into lanes of its own — the arrangement's **base level** as an edit). Two decisions carry it: the dependency arrow is **gui → form** (the arrangement stays pure and transport-agnostic, as `points_to_env` moving out of `guidef` already established), and the **unit bridge lives here** — the arrangement places in beats, the view in timeline samples (a clip's body *is* audio data, so its sample 0 sits at the offset), so one beat is `sample_rate / tempo` units and a musical `quant` becomes the lane's drag grid; the arithmetic is the core's (`beats_to_secs` → `secs_to_samples`), not a second implementation. Pure and host-free: the draw is a function of the arrangement. `Aggregate.handles` exposes the stable member identities the clip registry keys on.

- ✅ **C29 — Clip edit-back onto the arrangement + re-rendering** (editor Fase 3B): the return half of the bridge — the loop *data ↔ graphic ↔ sound* closed. `Editor.apply` resolves a `/gui_event <id> "clip" <offset> <dur>` (the drag/resize payload) through the clip registry and writes it onto the arrangement with `Group.move`; `poll` drains a whole host stream into it (from the script's loop, never the clock thread). Two conversions are the substance: a clip's offset is **absolute** on the shared axis while a placement is **relative** to its group, so the position converts back through the base the clip was drawn at; and only what actually *moved* is written — a drag carries the clip's unchanged `dur` along, and snapping that to the grid would silently reshape the element. `render(destination, clock)` plays the arrangement through its own `render` (no new path) and anchors every lane's playhead to the engine clock; `rerealize()` re-schedules the edited composition from the playhead's position (honest semantics: *re-schedule from here*, not a sample-exact splice — a sounding synth keeps sounding), and `follow=True` does it on every edit (the live editor). Tests: the arrangement gets the beats the clip was dropped on (grid-snapped), a nested clip converts through its base, a move leaves the length alone, render→apply→render is a fixed point, and the edited composition's NRT score starts where the clip was dropped.

- ✅ **C33 — Attach an envelope to the event it shapes**: the algebra already had the answer, and the editor now reads it. An `Aggregate` whose members **start and end together** — its derived temporal relation is *simultaneous* — is one thing on the timeline, so the editor draws it as **one clip with layered bodies** (the curve over the note) and drags it as one: the voice cannot outlive its envelope, and the envelope cannot be left behind. Each body keeps its own value axis, and a curve edited on such a clip finds the automation inside the aggregate. An element is also named for **what it is** (an automation is an *envelope*, named for the control it drives — not the `Element` that happens to wrap it).
- ✅ **C32 — The editor's transport** (with G22i): `Editor` owns the transport the multitrack view implies — `position` (in beats, live while playing), `play`/`pause`/`stop` and `locate`, plus `extent()` (the composition's length, **read from the arrangement**, so dragging a clip past the end lengthens the piece and it plays to its new end — a fixed length used to cut playback short). A `"locate"` from the host (a click on a lane's ruler) seeks the playhead: playing, it re-renders from there (so a seek also applies a pending edit); stopped, it moves the cursor the lanes draw. The two playheads stay distinct: the clock-anchored line that sweeps while playing, and the static cursor of a stopped transport.
- ✅ **C31 — A placement's length is what you hear of it**: rendering honors the *placement* `dur`, not only the element's own — a clip's length **trims** what it plays (events past the placement's end are dropped; a single-event element sounds exactly that long, through a copy — a placement never rewrites the element it places). Resizing a clip in the editor was until now a purely graphical act. Alongside it, `Editor.dirty`: an edit no longer interrupts what is sounding, it *marks* the composition, and the next transport action (a play, a resume after pause, a rewind/seek) re-reads it — rendering always re-flattens the arrangement, so it plays the clips where they now are. The editor also keeps its clip registry truthful after an edit, so a second drag measures against the current placement, not the drawn one.
- ✅ **C30 — The composition loop, end to end** (editor Fase 3C, closes Fase 3): the example, the docs and the last gap in between. A `Vector` element now **renders**: a buffer is data, so it sounds through the def *named to play it* (`Vector(buf, instrument="take")`, whose event carries the buffer number in a `buf` control and sets `legato = 1` so the take sounds its whole length) — the "needs an instrument, later phase" note from the compositional rendering, closed on the arrangement's own terms (no built-in sampler def; see `docs/decisions.md`). `Editor.window` lets a script's loop notice the window closing, and the editor's widget ids start clear of the host's auto-assigned window ids. Example `examples/editors/composer.py`: a take bounced offline and loaded from disk, a melody and a bounced pattern, composed, rendered as a multitrack window, dragged and re-rendered on the fly. Docs: the Python book gains a composition page (the arrangement's elements, grouping, rendering, and the editor's mapping and unit bridge); a manual smoke step for the audible/visual loop.

- ✅ **C38 — The session document: concrete material with provenance** *(opened 2026-08-13 with the GUI host's H track; **relocated 2026-08-14 to `crates/clausters-document/PLAN.md`, O8**, and kept here as a pointer; **O8 shipped**, which is what closes this)*. The decision that fixed its contents is unchanged and is the one worth reading twice — **an algorithm is never serialized, the way a project file never serializes a plugin**: a document holds concrete material, configuration (a generator's own settings as an opaque blob it never interprets) and provenance, the reference to the scripts that generated it. What moved is where it is implemented, and why: the `standalone` host is the other writer of the same format, and a format with two writers in two languages is a format that drifts — so the document became a crate every mode links, and this client binds it rather than defining it. The Python half that remains is a refactor rather than a format: **O8 landed the format and O10 the binding**, so `clausters.form` already writes and reads the crate's document losslessly and id-stably (`to_document`/`from_document`, `to_session`/`from_session`), and an edit already goes through the crate's own `apply` (`_native.document_apply`) rather than through a second implementation here. What is still this client's own is the **object model** - `Aggregate`, `Element` and the rest are Python objects with a conversion, not accessors over a shared tree. That is what the crate's decisions asked for (*the clients round-trip; they do not hold handles*), so this stays open as a refinement with no consumer waiting on it rather than as a gap.

  **Two writers, one format, disjoint subsets.** The client writes all of it; a `standalone` GUI host writes what its own editing can hold, as any program does. Which forces the rule that keeps that safe, and it is the same invariant the widget protocol already runs on (an unknown widget is laid out, not painted, and never dropped): **what a writer does not understand, it preserves**. A session authored from Python, opened in the standalone editor and saved back, must come out with its generator references intact — losing them there would lose the piece. The one concurrency guard needed: a save carries the version it was loaded at and is refused if the file moved underneath, which turns a silent overwrite into an error; no locking, and no merge machinery, because the two writers are two deployments and not two live editors.

  **Boundaries, recorded so the milestone does not grow:** the document is not the undo history (that is the host's, per session, never written — GUI H track), a save is not an undo boundary, and the document does not persist logical material even when it can point at it. Python: `form` reads and writes it, the `Editor` opens and saves one; `docs/decisions.md` for the plugin analogy and the preserve-what-you-do-not-understand rule; the client book's composition page for the user-facing half; an example that saves a composition, reopens it and re-renders.

### The generic editor: one editor per domain, and `edit(x)` (client arc, phased)

`Editor` was written for one model — the arrangement — and every view it has
grown since is a **mode** of it (`open_pianoroll`, `open_signal`) rather than a
view in its own right. What this track is pointed at is the verb the whole
document arc was pointed at: **`edit(x)` opens an editable view over a bare
structure the way `plot` opens a static one** — a `Timeline`, a buffer, a
break-point curve — and returns the handle the edited data is read back through.
*(It replaces the "Future directions" entry of the same name, which was owed by
`crates/clausters-document/PLAN.md`'s `O19` — that milestone closed saying these
"are written in their plans" and they were not. What it did leave is the floor:
the history is not an editor's to hold, and `Editing` is what a standalone
structure registers itself in.)*

**What the views actually edit, counted rather than remembered.** The host emits
edit-backs for six kinds of data and this client applies four of them:

| The view | What it writes back | The structure | Applied by |
|---|---|---|---|
| `waveform`, a clip's take body | `sample`, `draw`, `cut`, `paste` | a buffer's samples | **nobody here** — only `clausters-gui --session` |
| `pianoroll`, a clip's notes body | `notes`, `osc` | a `Timeline` of events | `Editor` (`osc` unrouted — see "Found by use") |
| `bpf`, a clip's points body | `points` | a break-point curve | `Editor` |
| `score` | `element`, `transpose`, `insert` | an engraved page | nobody — the examples do it by hand |
| `clip`, `track` | `clip`, `split`, `join`, `layer`, `locate`, `mute`, `solo`, `level`, `height` | the tree | `Editor`, except the four header tags |
| `patch` | `wire`, `move` | a logical aggregate | `Editor` |

Two of those rows are why this is a track and not a rename. **Sample editing
exists in the host and in no client**: `editors/session.py` draws over a take
because the standalone host owns the document there, and the identical gesture
from Python reaches nothing. And **`mute`/`solo` are not in the arrangement at
all** — `Track` wraps a `Timeline` and has neither — so they are widget props
whose edit-backs nobody receives, and what a person sets on a lane header lives
on the screen and does not survive a save.

**What of `Editor` is generic.** Five responsibilities are tangled in its ~3200
lines: it draws a GuiDef from a model, routes a tag onto an edit, records that
edit through the crate and projects the outcome onto Python objects, acknowledges
(`seq`/`version`, `_correct`, `_resync`, `adopt` and the fan-out to the other
views), and drives a transport. **The fourth is entirely generic** — it talks
about versions and windows and never about what was edited — the second is a
dispatch table, and only the rest is the arrangement's. So the base class is the
window, the view registry, the version protocol, the undo walk and an abstract
`route`; `FormEditor` is that plus the tree and the transport, and a roll, a
curve or a waveform is that plus one widget and one domain.

**Where a rule lives, decided 2026-09-02 with the user: the crate holds how an
edit inverts, the client holds the projection.** `_apply_points` knows "the
points are now these" in Python, `applyPoints` knows it again in TypeScript, and
the crate already knows it — which is the duplication the non-divergence rule
exists to stop, and the kind no compiler finds. So a domain is
`history::Editable` in Rust (`crates/clausters-document/PLAN.md`, `O20`) and
`edit(x)` needs three things per type and no more: the domain name, the
serializer and the projector onto the client's own object.

**And `Editing` is two layers wearing one name.** It is documented as one
composition's context and cached on the root element, but what the crate calls
`History` is a **registry** and what it calls `Log` is the arrangement's face of
it. The arrangement's state (`document`, `by_node`, `next_node`, `rederive`)
belongs to that face; the registry is one **editing context** over whatever is
open, and a structure registers itself in it. A bare structure therefore has a
`History` and **no `Document` at all** — the document exists only for the
arrangement's leg (`log::Tree`), which is the confusion `Editing` carries today
by holding both under one name.

**"Session" is taken twice already and never names this.** `Session` is the
client facade over a server, a clock and a host (`Session.live`/`nrt`/`embed`),
and a *session file* is the format `to_session` writes (a document plus its
`sources` table) — which is what `C48` below is about, and the only sense the
word carries in this track. **What an editor holds is not a session in either
sense and must not be called one**: it is the **editing context**, and it is
*transient state of the run* — the thing that is never written to a file and
goes when the data goes. The crate's own prose calls that "session state"; here
it does not, because two of this project's own structures already answer to the
word and a third meaning is how a reader ends up looking for it on disk.

**Which leaves one thing genuinely undecided, and it is named rather than
assumed: where the ambient registry hangs.** The direction said "the ambient
session's, or one the caller passes", and the second half is settled — a
`history=` argument, so a `GuiDef` composing a roll and a curve passes one and
undoes across both in one order. The first half is not: `Session` is the obvious
holder (it already owns the server, the clock and the GUI host, and a piece is
edited inside one), but nothing on it holds an editing context today, and a
process-wide ambient registry beside `ambient_host()` reaches the scripts that
never build a `Session` at all. **The choice is `C49`'s to take**, with its own
reason written down, and it must be the same choice in TypeScript.

**Quarantine, and what it means here.** `form` and the multitrack are not
removed: they are what three books, both example directories and the whole
composed case stand on, and taking them out of the API would break all of that to
prove a point. What is frozen is their **surface** — nothing new goes into `form`
or `Editor` (mute and solo, recording, the score as an element, the OSC lane)
until the domains below exist, because those are precisely the additions that
would deepen the confusion this track is here to undo. **The rename comes with
the split** (`C49`) rather than at the end: `Editor` is the name of the generic
class, so the arrangement's cannot keep it while the generic one is being
written under some placeholder — that is how a placeholder becomes the API.

Three neighbouring "Future directions" entries are part of this design and are
read with it rather than separately: "`Track` wraps a `Timeline`, so the tree has
two ways of placing things" (the events domain's model question), "A drawn curve
is a list of points, and `Env` is an envelope for `EnvGen`" (the points domain's),
and "The mapping exists and is private to the `Editor`" (what a structure plays
through once it is editable on its own).

- ✅ **C48 — Reopening a session gives back the structures, not a description.**
  First, because `edit(x)` over samples is only half a verb without it:
  `from_session` rebuilds the **tree**, but every leaf that is code comes back as
  a bare reference unless the caller passes `resolve=`, and the `sources` table is
  handed back as data — nothing loads the takes into buffers again. So: a
  resolver over the session's own table (buffers read from `sources` onto the
  server, defs looked up in the store, a generator left frozen with its
  `rendered` as the floor, which is already the documented contract), which is
  what makes "edit these samples" name a buffer that is on the server and a file
  on disk. It settles the two things the header tags exposed:
  **`mute`/`solo`/`level` are the composition's** and belong in a node's
  configuration, so they survive a save and a reopen, while **`height` is the
  view's** and belongs in no document. And it names where recording lands —
  `RecordingStream` produces a buffer today and nothing places it as a take,
  which is the other half of why the domain reads as confused. *Acceptance:* a
  session written here, reopened here, gives buffers that sound and lanes that
  are muted the way they were left; a generator with no resolver still draws and
  plays what it last rendered.

  **Done 2026-09-02.** Four things, in both clients and in one commit
  (`sessionResolver`/`sourcesOf`/`take`/`mixingOf` are the TypeScript spellings;
  the calls and their order are the same).

  - **`session_resolver(session, *, server, folder, defs)`** — a `resolve` over
    the table, handed to `from_session`. Each file is read **once per source
    id**, before the tree asks: two clips over one take are two windows onto one
    buffer, and resolving each window on its own gives them two buffers that
    drift apart on the first edit. A *volatile* source, and a file that has
    moved, come back frozen — half a session is worth opening, and one moved
    file is not the difference between a piece and nothing. Reading is
    asynchronous in the web client and not here, so the takes are read *while
    the resolver is built* and the resolver itself is the same synchronous
    function on both sides; the `await` is the language's, not a different call.
  - **`sources_of(element, *, folder)`** — the table built from the arrangement,
    which is `to_session`'s own error message as a function. It needed one new
    fact: **`Buffer.path`**, the file a buffer was read from, which is the one
    thing a slot number cannot say. A buffer allocated in this run is written
    *volatile*. And the second save turned out to be where the format lost its
    own contents — a piece opened with no resolver came back holding
    `FrozenSource`s that had forgotten where their samples were, so
    `from_session` now **locates** each of them from the table it just read.
  - **Mixing is the composition's, height is the view's.** `Element` grew
    `mute`/`solo`/`level`; they ride in the node's **configuration** (the same
    opaque door a leaf's code uses, which also puts them in `leaf_config` — a
    `Configure` that started from a config without them would unmute a lane on
    every curve edit) and they are honoured by `flatten`, all three inherited
    down the tree. The editor draws them on every lane header and routes the
    three tags through the log like any other edit, and answers `"height"` as
    the screen state it is. One thing this forced, found by drawing: **a view
    flattens unmixed** (`mixed=False`) — a muted lane keeps its clips, its notes
    and its length, or the picture reports silence as absence.
  - **`take(buffer, …)`** — where a recording lands: a `Vector` whose length is
    the samples' own, which is the line every script wrote by hand.

  What is **not** here, and is written where it is read: the standalone host
  *carries* mixing across a save (its config is opaque) and does not draw it —
  its lane header is not bound to a node. That is `crates/clausters-gui/PLAN.md`'s.

- ✅ **C49 — `Editor` is the generic one, and the arrangement's becomes
  `FormEditor`.** The split and the rename are **one milestone**, because the
  name is the point: `Editor` is what a person calls to edit a buffer, a curve
  or a timeline, and it **imports nothing from `clausters.form`**. What is named
  `Editor` today was written for the arrangement and becomes `FormEditor`, no
  alias, with both books and every example moving in the same commit. Doing the
  extraction first under a placeholder name and renaming afterwards is how a
  placeholder becomes the API.

  **How the generic one is modularized, since it orchestrates rather than
  performs.** Four collaborators, each testable without the others, and the rule
  that fixes the boundaries is that an editor owns **neither the data nor the
  history**:

  - **`View`** — draws the `GuiDef` of *one* structure and keeps the registry
    from widget id to what it shows. The only per-domain thing on the graphic
    side, and what `_draw_signal`/`_draw_pianoroll` already are without being
    separable.
  - **`Domain`** — the data adapter: structure → payload, payload → the client
    object (the projection), plus the entry's label and its coalesce key. It
    does **not** know how an edit inverts — that is `history::Editable` in the
    crate (`O20`) — and it does not draw.
  - **`Echo`** — the acknowledgement protocol: `seq`/`version`, `_stale`,
    `_correct`, `_resync`, `_acknowledge`. Entirely generic, and today
    interleaved with the routing; on its own it is testable with no structure at
    all.
  - **The editing context** (what `Editing` is today) — the `History`, the list
    of views, and the `turn` that answers the other windows. The editor **asks
    for it and never builds one**, which is `O19`'s rule restated where it can
    be enforced.

  So `Editor` itself does three things: opens the window through its `View`;
  dispatches an event — the tags that are **not** edits (`selection`, `view`,
  `view_x`, `view_y`, `layer`, `focus`, `locate`) it answers generically, the
  ones that are go to the `Domain` and from there to the `History` — and walks
  undo/redo applying the legs the pile hands back. **Transport and render are
  not here**: they are `FormEditor`'s, because a bare structure at most sounds,
  it has no piece to move over.

  **Where the files go: editing is a subdomain of the GUI, so it is a
  subpackage**, the way `notation/` already is — `clausters/gui/editing/`, with
  `editor.py` (the orchestrator), `view.py`, `domain.py`, `echo.py`,
  `context.py` (today's `Editing`: the history, the views, the `turn`) and
  `formeditor.py`. Today's `gui/editor.py` and `gui/editing.py` are what it
  absorbs, and `gui/editing.py` **splits** on the way in: the registry half is
  generic and stays in `context.py`, the arrangement half (`document`,
  `by_node`, `next_node`, `rederive`) goes with `FormEditor`, which is whose it
  is. `clausters.gui` keeps exporting `Editor` and `FormEditor`, so the import
  path a script writes does not move; what moves is the module path four places
  name (`gui/ids.py`, `gui/__init__.py`, `form/__init__.py`,
  `docs/architecture.md`), and they move in this commit. The TypeScript side
  mirrors it literally — `src/gui/editing/` with the same six files plus its
  `index.ts` — since the module tree is one tree in two languages (`W21`).

  **`FormEditor` is then `Editor` plus what only a tree has**: the held
  `Document` and the node index, the several views of one composition, the
  lanes/clips/patch registries, and the transport. **No new surface** in either
  class beyond the rename — `edit(x)` is `C50`'s — which is what makes this safe
  to do before the domains exist.

  *Acceptance:* `Editor` and its four collaborators import nothing from
  `clausters.form`, and a test drives one over a plain structure with no
  arrangement anywhere; `FormEditor` keeps every public name today's `Editor`
  has, so `editors/two_windows.py`, `editors/multitrack.py` and
  `editors/composer.py` run unchanged by hand after the rename; `Echo` is
  exercised by a test that never touches a structure; the suites pass with no
  test edited to accommodate the split, only the rename.

  **Done 2026-09-02**, as written, in both clients and in one commit.

  **The open question, taken: the registry hangs on the data, and there is no
  ambient one.** The two candidates named above were `Session` and a
  process-wide registry beside `ambient_host()`; the answer is neither, and the
  reason is the rule the whole design rests on — *a history belongs to the data,
  never to a view*. A structure carries its own context (`Editing.of` caches it
  on the object), so two editors over one thing find one history with no
  registry to consult, and a composed view over several structures passes one
  (`context=`), which is the half that was already settled. An ambient holder
  would be a **second way to answer one question**, and the two would disagree
  the first time a script edits a curve outside the `Session` that made it — or
  in no `Session` at all, which is most scripts. It also keeps the lifetime
  right for free: the context dies with the data, which is what "never
  serialized" asks for, where an ambient one outlives every piece it ever held.
  The same choice in TypeScript, by the same route (`Editing.of` over a
  `WeakMap`, plus the `context` option).

  What else the doing decided beyond the plan:

  - **`FormEditor` inherits.** "`Editor` plus what only a tree has" is a
    subclass, so `apply`/`_deliver`, the unit bridge, the ids, the echo
    delegation, `undo`/`redo` and the labels are written **once** and the
    arrangement overrides five hooks (`draw`, `_route`, `_owns`, `_resync`,
    `_restructure`) plus `_step` — the tree walks its history through the `Log`
    over a document, the generic one projects a payload per structure. The base
    calls what it edits `structure`; `FormEditor` keeps `element` as the
    arrangement's word for the same slot, so its 3 000 lines were not touched to
    rename a field.
  - **`Editing` splits by subclass, not by copy.** `FormEditing` (in
    `formeditor.py`) is the generic context plus the held document, the node
    index and the id to mint next, and `Editing.of` is a classmethod so
    `FormEditing.of` mints the right kind.
  - **`_resync` is the view's answer, sent by the `Echo`.** The plan listed it
    under `Echo`; what a widget *should be drawing* is per-domain, so the
    protocol asks the view (`View.props`) and `Echo` owns only when and how it
    goes out. That keeps `Echo` testable with nothing behind it, which was the
    point of naming it.
  - **`FIRST_VERSION` is defined twice on purpose**, in `context.py` and in
    `form/document.py`. One is what a *file* says its version is, the other what
    an editing context counts from; they are the same number, and a generic
    module importing the arrangement's would be the dependency this subpackage
    exists to refuse. Written down where both are.
  - **Two `View`s, one package.** `clausters.gui.guidef.View` is a tree you can
    open; `clausters.gui.editing.View` is the picture of a structure. Only the
    first is exported from `clausters.gui`, and the web client mirrors it by
    exporting the collaborators as the `gui.editing` namespace rather than
    flattening them.

  What the split cost in tests: **nothing was edited to accommodate it** — only
  the rename, plus two import paths that named the module. `tests/test_gui_editing.py`
  is new and drives a generic editor over a plain object with a test-local
  `Domain` and `View`, which is also the shape `C50`'s three domains take.

- ✅ **C50 — `edit(x)` over the three fundamental structures** *(waits on the
  crate's `O20`; ported in the same commit as the web client's `W28`)*. The verb
  and the three editors under it: `edit(buffer)` → a `SamplesEditor` over a
  `waveform`, `edit(curve)` → a `PointsEditor` over a `bpf`, `edit(timeline)` →
  a `NotesEditor` over a `pianoroll`. Each registers **its structure** in the
  ambient editing registry, draws one widget, routes its own domain's tags and hands
  the edited data back through the object the caller already holds. This is where
  `"sample"`, `"draw"` and `"osc"` are routed for the first time, and where a
  sample edit finally has a client-side path: the payload's own inverse into the
  history, the values onto the server's buffer (`/buffer_setRange`), the picture
  acknowledged after the samples are pushed and not before.

  **What of this waits on `C48`, stated so the order is not re-derived later.**
  Everything here is buildable over a structure the calling process **holds** — a
  buffer it loaded or generated, a curve it built, a timeline it filled — and
  that covers the whole acceptance below, which is why the two are separable at
  all. What it does not cover is the structure that came back from a **file**:
  after a reopen the buffer is a row in the `sources` table and not an object, so
  `edit(x)` has nothing to be handed and a sample edit has no server buffer to
  write to. Taking `C48` first is therefore not a dependency of the code but of
  the verb being worth having: without it `edit(x)` works for the run that built
  the data and stops at the one that reopened it.

  *Acceptance* (the direction's own, kept verbatim): `edit(x)` called twice over
  one structure gives two windows and **one** stack, and an undo in either
  updates both; a client builds a curve, edits it in a view and reads the edited
  points back with no composition involved; a `GuiDef` composing a roll and a
  curve undoes across both in the order the edits were made. Plus this track's
  own: a stroke drawn over a buffer's samples from Python changes what the server
  plays, and `Ctrl+Z` puts it back; and — the clause `C48` is what makes
  writable — the same stroke over a take of a **reopened** session does the same.
  An example per structure, since nothing in CI runs one.

  **Done 2026-09-02**, in both clients and in one commit, with an example per
  structure in each (`editors/edit_curve.py`, `edit_notes.py`,
  `edit_samples.py`, and the three pages beside them). What the doing decided:

  - **The crate holds the inverse, and now there is a door for it.** `O20` named
    the vocabularies and bound the coalesce key; the other half was still being
    computed per language, which is the divergence the first door existed to
    prevent. So `domain::edit` (crate) and `clausters_domain_edit` /
    `domainEdit` (both seams, `CORE_ABI_VERSION` 35→36): the state goes in with
    the payload and comes back as what the structure now is **plus** the payload
    that puts it back, one call because the inverse has to be read before the
    edit lands.
  - **Two domains answer nothing there, and it is a finding rather than a
    limit.** The tree needs a version to check against and a grid to snap to,
    and has `clausters_document_apply`. A span of **samples** is a borrowed view
    by construction — the frames are in a server buffer — so the crate holds no
    state to invert, and the protocol had already solved it: `"draw"` and
    `"sample"` carry what they replaced, so nothing is read back to undo a
    stroke. What the domain shares is the payload's shape and its coalesce key,
    and where the state lives is the client's. Written into both books.
  - **A curve's segment shapes had nowhere to go.** `points::Point` was a
    position and a value, so a curve edited through the vocabulary came back
    **straight** — the shape of each segment silently dropped. `Point` grew the
    `data` field `Event` already had: deciding what a shape *is* stays refused,
    carrying one is not the same act.
  - **An identity is per structure, not per view.** Found by the acceptance's
    own first clause: `edit` twice over one curve minted two identities in one
    pile, so the second window's undo walked legs naming somebody else — a dead
    button. `Editing.identity` is now the door, and `Editing.register` stays for
    a caller routing its own structure.
  - **The web client queues a stroke's write.** A page's buffer calls are
    asynchronous where Python's are not, and `Domain.project` answers *whether*
    the edit lands rather than when — so the writes are chained in order, since
    two strokes over one span that raced would leave the buffer holding the
    first. Named in `docs/bindings.md`'s sense of `idiom` and written in both
    books.

  What is **not** here: `"osc"` still reaches no branch — a roll's OSC lane is
  the timeline's non-note items, which `NotesDomain` keeps across an edit and
  does not yet let a hand move. It is the events domain's second verb and it
  waits for one to exist.

- ✅ **C51 — `FormEditor` composes the views it used to contain** *(done
  2026-09-02, with `W28` in the same commit)*. The rename happened in `C49`;
  this is the collapse it was for. `open_pianoroll` and `open_signal` stopped
  being modes with private draws and became `edit()` over the part of the
  composition the window is pointed at, joined to the piece's editing context —
  so a `FormEditor` holds `Editor`s rather than reimplementing them, which is
  the claim the whole track was worth. `_draw_pianoroll`, `_draw_signal`,
  `_source_of`, the `_mode` field and the `_signals` registry are **gone**, not
  wrapped; what replaces them is `_compose`, `FormEditor.composed` and two
  constructors.

  **What made one undo order possible, and it is the part worth reading.** A
  history entry can name several structures, and the two editors write in
  different vocabularies: the multitrack's legs are intents on a document, a
  stroke's leg is the crate's `samples` — which the document's own face
  (`Log.undo`) drops on the floor, because it filters to what it can read. So
  the walk stopped being the walker's: `Editor._step` takes the step off the
  shared `History` and hands the legs **round the context**, and every editor
  projects the ones naming its own structure (`project_legs`), after which the
  one that walked draws (`reflect_step`) and the rest are told on the way out of
  the turn as they always were. `FormEditor` overrides the pair: its legs go to
  the held document and through `_project`. Without this, Ctrl+Z over the
  multitrack after a stroke stepped the cursor and moved nothing — a dead
  button, and the exact failure the shared pile was placed to avoid, one level
  up.

  Four smaller things the work forced, each because something was wrong rather
  than missing: a composed view has **no piece of its own**, so a ruler click
  and a marquee are handed up through `composed_in` (a seek of the composition,
  and a selection *of the element that view draws*); `extra` moved onto the
  generic `Editor`, since the script's own panel belongs to whichever window it
  was asked for; the measure stack moved to `samples` with `MEASURES`/`measures`
  public in both clients, because it is the `SamplesView`'s and not the
  arrangement's; and `open_signal`/`open_pianoroll` **stopped overwriting the
  editor's host** — the multitrack already answers one, and taking the second
  window's silently sent every acknowledgement to the wrong place, invisible in
  the ordinary case where the two are the same object.

  A generator's roll is now refused **before the hand tries**: what it produced
  is bounced onto a timeline of its own and the widget is told `notes_editable`
  is false, instead of unwinding each drag after the fact.

- ✅ **C52 — The client's event loop: `edit(x)` opens, and nobody writes a drain
  loop** *(designed and done 2026-09-04 with the user, off a throwaway prototype
  whose three measurements are below)*. `edit` is the **only** verb in the client that
  hands back something unopened: `plot`, `scope`, `plot_def` and `View.open` all
  resolve the ambient host, open, and return a handle the script may discard.
  And it is the only one that then asks the script for a loop — `while
  editor.window is not None: editor.poll(...)` — which **43 example files**
  currently spell out by hand. Both halves are one gap: there is no place in
  this client where inbound host messages are drained, so the drain is copied
  into every program that wants one.

  The same absence shows up twice more. `WindowHandle.on_closed` and
  `PlotWindow.on_closed` fire only in the programs that happen to pump, so a
  callback the docstring promises is delivered by the caller. And the standing
  rule that a routine must never block the clock thread has **nowhere to defer
  to**: `OscReceiver` can already hand a handler to `clock.sched(0.0, ...)`, and
  there is no equivalent for anything that touches a window.

  **Two pieces, because the loop and the clock are not the same object.**

  - **`EventLoop`** — the machine every application event loop is: registered
    *sources* (the host's socket, timers, a cross-thread post queue), **one
    wait** over all of them bounded by the nearest timer rather than a fixed
    sleep, a wake channel so another thread can break that wait, an iteration
    with **fixed phases** (due timers → ready sources → dispatch → deferred), and
    `run()` / `stop()` / `iterate(timeout)` so it can be driven by someone else's
    loop (a notebook, a test) instead of owning the thread.
  - **`AppClock`** — the clock face over it, a sibling of `TempoClock`, in
    **seconds**: `sched`, `play(routine)`, `defer(func)`. This is sclang's
    reading and it is the substance of the split: the timer source of the app
    loop *is* a clock, so an animation is a routine that waits, not a second
    scheduling vocabulary beside the one the client already has. Beats stay
    `TempoClock`'s; this is the application, not the music.

  **The precedent is inside this client already**: `base/_oscinterface.py`'s
  `OscReceiver` is a daemon thread with its **own socket** (`settimeout(0.1)`)
  that optionally hands each handler to a clock. "Receiving thread, then deliver
  to a clock" is how this client already talks to the audio server; what is
  missing is the GUI's half of it.

  **What goes to Rust, and it is almost nothing — measured, not assumed.**
  `clausters-core::tempoclock::Scheduler` is already the timer queue this needs
  (a min-heap by time, stable insertion order, `pop_due`/`remove`), and
  `TempoClock` already binds it; the `AppClock` binds the same one keyed in
  seconds instead of beats, and no second queue is written. Nothing else is worth
  moving down: the wait, the thread, the GIL and the callback invocation are the
  language's by definition, and a sans-io phase machine would shuttle opaque ids
  across the FFI to decide "what is due" (that is the `Scheduler`) and "who gets
  this" (that is a dict). The two problems the prototype actually found — the GIL
  and the socket — are precisely the two a shared crate cannot own.

  **The web client is not waiting on this; it already has it.** `gui/host.ts`
  says so in as many words (*"Nothing pumps... the browser pushes"*) and routes
  `/gui_event`/`/gui_closed` to the handles as they arrive; none of its examples
  drain anything. So this milestone runs the unusual direction — **Python
  catching up to a structure TypeScript has by language** — and the phase
  contract is read off the browser's rather than invented here. What the port
  owes is the surface: `edit(x)` opening, `Editor.close`/`onClosed`, and an
  `AppClock` whose engine is the page's own loop (`clients/web/PLAN.md`, `W29`).

  **Three decisions, taken from measurements rather than from taste** (the
  prototype, 2026-09-04, discarded after the run):

  1. **The loop holds a lock across a whole applied message, and the read side
     can take it.** `NotesDomain.project` does `clear()` then re-`add`, so a
     reader can see a half-rebuilt timeline. Measured, one thread applying while
     the main thread reads: **0 torn reads in 3.2 M** at 8 notes and **0 in
     214 k** at 300 — and **21 191 of 24 150 (87.7%)** at 4000, where the
     rebuild finally outlasts CPython's 5 ms switch interval. That is the worst
     possible shape (invisible in every small test, systematic in a real piece),
     so the lock is not optional. Its cost is a tight-loop reader losing ~30% of
     its reads, which is nothing in use.
  2. **A reply is not an event: the loop routes it to whoever asked.**
     With the drain thread running, `GuiHost.query` lost its `/gui_info`
     intermittently (1 of 3); with a ten-line reply slot, 3 of 3. There is
     exactly **one** such call site, and today's `query` is already fragile
     without any loop — it requires the *very next* datagram to be `/gui_info`,
     so an event arriving mid-query already breaks it. The loop is what makes
     that correct, not what endangers it.
  3. **The loop gets its own thread.** sclang's `AppClock` runs on the app's main
     thread and inherits its safety for free; here the main thread is the user's
     script, so it cannot. `OscReceiver` is the precedent and the audio server's
     event sequencing is the reason: a shared thread would let a window's drain
     delay it.

  **`edit(x)` then opens** on the ambient host — the rule `plot`, `scope` and
  `View.open` already follow — and returns the `Editor` of the structure's kind,
  which is already one per window and one per kind. It gains what a handle in
  this client means: `id`, `closed`, `close()`, `on_closed()`. **No
  `MidiEditorWindow` and no per-kind window class**: that would be a second
  hierarchy over the axis `SamplesEditor`/`PointsEditor`/`NotesEditor` already
  is, and every useful member on it would be a back-pointer. Closing the window
  deregisters the editor; the **history survives**, because it belongs to the
  data (`Editing.of`), so re-opening resumes the same undo order.

  **Deliberately out of scope: a shared base class for the four window handles.**
  `PlotWindow.id` versus `Editor.window` for the same thing is untidy, and
  tidying it unblocks nothing. What this milestone does take is the *name*: `id`
  everywhere. The base class is a candidate for later, on its own merits.

  *Acceptance:* `edit(curve)` followed by `print(curve.to_points())` reflects
  what was drawn, with no loop written anywhere; `query` under the running loop
  answers N of N; a 4000-note timeline read from the script while a stroke is
  applied never sees fewer notes than it has; a script holds its window open with
  one call and a notebook needs none; `AppClock.play(routine)` animates a widget
  from a routine, and a routine on the `TempoClock` reaches a window through
  `defer` without blocking the clock.

  **Done 2026-09-04.** `base/loop.py` (`EventLoop`), `base/appclock.py`
  (`AppClock`), the host's end (`loop`, `clock`, `looping`,
  `subscribe`/`unsubscribe`, `deliver`, the reply slot) and `edit(x)` opening,
  with `Editor` grown into a handle (`id`, `closed`, `close`, `on_closed`) --
  plus the three `edit_*` examples, which are this milestone's own acceptance
  surface and lost their loops here rather than in `C53`. Checked by eye: five
  points where the example built four, times and shapes moved, read back off the
  `Automation` with nothing written in the script. Four things the work changed
  or found, each because something was wrong rather than missing:

  - **The lock is not what answers a torn read; a swap is.** Decision 1 said the
    loop must hold a lock and the reader take it, which is true of a handler in
    general and answers nothing for `print(curve.to_points())` -- nobody takes a
    lock to print. So the projection stopped clearing and re-adding:
    `Timeline.replace` builds the new order and binds it in **one step**, and the
    same measurement that gave 87.7% torn reads at 4000 notes now gives **0 of
    5099** with no lock anywhere. The loop still holds its lock across a whole
    delivery, for what a swap cannot cover (several structures written together,
    a handler of the caller's own).
  - **`OscTcpInterface.recv(0.0)` never touched the socket.** It computed a
    remaining budget, found it spent and returned -- so a loop that waits on the
    descriptor itself and then reads with no timeout of its own span on a
    readable socket it never drained. A zero timeout now means *what is there
    now*, which is what the whole select-then-read shape asks of it.
  - **A socket's timeout belongs to the socket, not to the call.** With a loop
    reading on one thread and a script sending on another, `_recv_into_buf`'s
    `settimeout` governed the other thread's `sendall` too. The TCP interface
    grew a lock held over the *timeout*, never over the wait -- the loop reads
    with `timeout=0`, so it never holds it while waiting.
  - **One editor, one window.** `open` on an open editor answered by opening a
    second and orphaning the first, which nothing did until `edit` began opening
    for itself. A second view of one structure is `edit(x)` twice -- two editors
    on one history -- so this now returns the window it already has.

  What is **not** in it, and was `C53`'s: the other 40 examples, and the question
  of whether `open()` should start the loop for every window rather than only
  where an editor or the `AppClock` asks for it. (It should, and `C53` took it.)

- ✅ **C53 — The examples pass: the drain loop is deleted, once** *(done
  2026-09-04)*. 43 example files hand-write a drain today —
  `clients/python/examples/views/` 13 (`editor.py` four times),
  `panels/` 12 (`piano.py` three times), `editors/` 12, `notation/` 4, and the
  root `examples/clock_recorder.py`; `clients/web/examples/` has none, which is
  the same evidence from the other side. They are rewritten against the loop, and
  the pass is where **one decision that cannot be made in the abstract** gets
  taken: a script must still hold its window open until it is closed (a by-eye
  check needs the window to live), while a `# %%` notebook must not block at all,
  since the loop is already running and the cells go on. That asymmetry is
  written once — in the `examples` skill and in the books — rather than
  rediscovered per example.

  It is a separate commit so `C52`'s diff stays readable, and it is entirely
  manual: **nothing runs any example**, so each one that is touched is run by
  hand, and `npx pyright` in `clients/python` covers the call sites afterwards.

  **Done**, and the decision it was here to take came out the simple way:
  **opening a window starts the loop**, for every window and not only where an
  editor or the `AppClock` asks. An open window is the thing that *has* events,
  so having one is what makes a loop necessary — and a plain panel now delivers
  its callbacks exactly as the page does, which is the gap `W29` would otherwise
  have inherited. `pump` and `poll` stand down from the first open window (they
  already did while a loop ran); a script that would rather drain by hand builds
  its window with `define`, which opens nothing this client remembers.

  What the pass added beyond deleting loops, each because the examples could not
  be written honestly without it:

  - **`wait` is the one call a script ends with**, on everything that opens a
    window: `GuiHost.wait` (every window this host opened), `WindowHandle.wait`,
    `Editor.wait`, `FormEditor.wait` (its own window *and* its composed views),
    `PlotWindow.wait`, `PatchWindow.wait`, `ScopeWindow.wait` — with `closed`
    beside each for the same question asked without blocking. Waiting from the
    loop's own thread raises rather than hangs (`EventLoop.is_current`).
  - **The transport ticks itself.** `Transport.update` was documented as "call
    it once per pass of the script's loop", which is a hand-written pump by
    another name and the reason seven examples had one. A `play` now schedules
    it on the host's `AppClock` while the piece is sounding, and it stops when
    the pass does.
  - **`GuiHost.stop` closes the loop** — and the clock over it. A host that
    started a thread to drain a socket had no business leaving it running once
    the socket was closed.
  - **The work that used to ride in the drain loop moved to the app clock**, not
    to a `while` with a stopwatch: the timed restyles in `panels/text.py` and
    `panels/style.py`, the membership demo in `views/linked.py`, and the
    periodic read-outs in `views/take.py`, `editors/two_windows.py`,
    `editors/composed.py` and `editors/multitrack.py`. Those are the examples to
    read for the shape.

  The root `examples/clock_recorder.py` was on the list and does not belong to
  it: its `poll` is the shm IPC ring's, not a GUI host's, and it stays as it is.

### A sequence's structures are objects (client arc, phased)

*(Opened 2026-10-01 by the user, reading what a script can do with a sequence
an editor holds: a client API whose return values are id numbers is not
usable; each should be an object that represents the data structure, as a web
page's DOM would be unusable if it were driven by ids.)*

**The defect.** `clausters.seq.EventSequence` answers and takes **numbers**:
`add` answers an `int`, and so do `add_lane`, `add_expression` and
`expression_to_lane`; `move`, `set`, `remove`, `remove_lane`,
`remove_expression` and `lane_to_expression` take one. Reading is worse:
`entries`, `get` and iteration build `Event(keys)` and drop the event's
`expression`, so a note's curves are seen only in `data()`, as JSON. The web
port has the same surface (`clients/web/src/seq/sequence.ts`). It is the
document's identity leaking through the JSON door into the API. Every other
resource this client has is an object holding its number — a `Bus`, a
`Buffer`, a `Synth` — and so is `Timeline`, whose `add` answers an entry and
whose `remove` and `move` take one. A script holding a sequence has to carry
the document's ids by hand, and nothing tells it what one of them names.

**The principle**, which is the DOM's and holds for every structure a client
reaches through a handle:

1. **What a script creates or reads is an object that represents the
   structure.** The id is inside it; no call takes or answers a number.
2. **The objects are live views over the Rust document** (the handle and an
   id), not copies. A read asks the document, so after a hand moves a note in
   the roll, the object the script holds reads where it now is. A write goes
   through the structure's vocabulary, and through its history when it has one.
3. **One structure, one object.** The sequence keeps an identity map (id to a
   weak reference), so an event read twice is the same object, and objects work
   as keys of a `dict` or members of a `set`.
4. **A view can be detached.** An event that is removed leaves its object
   detached (`sequence is None`, and a write raises); an undo restores the same
   id, and with it the same object.
5. **A collection carries its own verbs**, as `element.children` does: `add`
   answers the object made, and each object has `remove()`.

**The shape.**

```
EventSequence
 ├─ .events        live collection in time order: add, at, range, iteration
 │    └─ SeqEvent       .at  [key]  .automation  .remove()  .event (a free Event)
 │         └─ .automation   collection: add -> Automation
 ├─ .automation    collection: add -> Automation   (curves over the whole sequence)
 │    └─ Automation     .target  .points  to_points/set_points  .remove()
 ├─ .history       UndoHistory: (label) as a context, undo, redo, undo_label
 ├─ .tempo_map     TempoMap
 └─ .midi          MidiSpec
```

```python
seq = timeline.render_events()

level = seq.automation.add({"control": "amp"}, [(0.0, 0.05), (6.0, 0.4)])
top = seq.events.at(4.0)[0]
bend = top.automation.add({"bend": True}, [(0.0, 0.0), (2.0, 1.0)])
coda = seq.events.add(6.0, Event(midinote=74, dur=1.0))

roll = edit(seq)

with seq.history("humanize"):            # one entry, labelled "humanize"
    for event in seq.events.range(0.0, 4.0):
        event.at += 0.01                 # a move
        if event["midinote"] >= 67:
            event["velocity"] = 90       # a set, with the core's coherence
coda.remove()

edit(bend)                               # the points editor, over one note's curve
```

**The history, made public.** What `"humanize"` labels is an **entry of the
structure's history**: the text an undo names, as `"draw a curve"` is the
label of a gesture in the notes editor's intake. The history is the editing
context `clausters.gui.editing.Editing.of` already makes on first ask and every
window over the structure shares; today it is private to the GUI, which is why
a script's edit beside an open editor records nothing, redraws nothing and is
not heard until the next gesture — the gap `crates/clausters-document/PLAN.md`
names under `O4` ("the document moved by a route that is not a gesture"). It
becomes `seq.history`, and it needs no host, so it leaves `clausters.gui`:

- a structure with **no** history — no editor opened on it, nobody asked —
  changes freely and records nothing, so a script that writes ten thousand
  notes keeps no ten thousand inverses;
- a structure **with** one records each change through its objects as an entry,
  and the change is a turn: the other views are brought in step, the playback
  reading it is synced, and `on_change` is called;
- `with seq.history(label):` makes everything inside it **one** entry;
- `seq.history.undo()`, `.redo()` and `.undo_label` walk the same order the
  windows' Ctrl+Z walks.

**Decisions:**

- ✅ **`SeqEvent`, not `SequenceEvent` and not `Event`** *(the user,
  2026-10-01)*. An event of the sequence; the abbreviation keeps it apart from
  `EventSequence`, where the same two words swapped would be hard to remember.
  Not `SeqItem` on the pattern of `OscItem`/`MidiItem`: those are events of
  their `type` now (`crates/clausters-apps/PLAN.md`, `X3.1`), and what a
  sequence holds is events. Not `Event` itself, although the DOM keeps one
  class for a node in and out of a document: `Event` is a `dict`, and its
  `play()` writes into itself (`node`, `server`, the derived keys), so an
  `Event` inside a sequence would write the document every time it was played.
  A `SeqEvent` answers a free `Event` (`.event`) to play or to copy.
- ✅ **"Lane" names four things, and each gets its own name first** — a
  rename that lands **before** `C57`, so the objects are written once with
  the right names. The inventory (2026-10-01):
  - the multitrack's **take lane**: the several lanes a track holds, one of
    which plays (`Track.active`) — comping. `multitrack::Lane` and
    `view::LaneView` in Rust, `Lane`/`LaneView` in both clients.
    ✅ **`TakeLane`** *(the user, 2026-10-01)*, and `TakeLaneView`: the
    prefix says what it holds, and leaves no "lane" in the project
    unqualified.
  - the multitrack widget's **row**, which draws a *track* (`picture.rs`: "a
    row is a track, showing the lane it plays") and is named `Lane` anyway —
    `clausters.gui.multitrack.Lane`, `gui/multitrack.ts`'s `Lane`, the host's
    `structures/clips.rs::Lane` and `document/tree.rs::LaneRow`. It is the
    whole row — its identity (a clip names the row it is on), its height, and
    the state its header is drawn from (`track::Header`, built from it on every
    draw) — and it is not a `Track`, nor the `TrackView` the session saves
    beside one. ✅ **`TrackRow`** *(the user, 2026-10-01)*, which also tells it
    from the automation rows stacked under it. **The wire follows** *(the
    user, 2026-10-01)*: the `multitrack` widget's `lanes` prop becomes
    `tracks`, a clip's `lane` field `track`, and the host's `"lanes"` report
    `"tracks"` — in `docs/gui-protocol.md`, the host, `clausters-editing`'s
    projection and reader, and both clients' builders.
  - `clausters.gui.multitrack.Multitrack`, the client's record of what one
    multitrack widget draws, under the document's `Multitrack`'s name: a
    second way to hold a multitrack, with no document, no history, no sources
    and no playback, which nothing but the tests uses. ✅ **Deleted** *(the
    user, 2026-10-01)*, in both clients, with its `Clip` and its row class,
    and **the `multitrack` builder with it** (`guidef.multitrack`, its port in
    `guidef.ts`): a multitrack drawn with no document has nowhere for an edit
    to live, the editor's window is composed by the crate and never by the
    builder, and its one other caller is a column of `panels/gestures` (both
    clients), which goes — the waveform, the roll and the ruler there show the
    same gesture table. `TrackRow` is the host's alone, the host's `multitrack`
    element stays (the editor draws with it), and `edit(multitrack)` over the
    document is the one way a script holds a multitrack.
  - the server's **event lane** (`/lane_new`, a transport's events as data).
    ✅ **`EventLane`** *(the user, 2026-10-01)*: `clausters_core::lane`'s
    `LaneData`, `LaneNote`, `LaneVoice`, `LaneMessage`, `LaneMidi`,
    `LaneUmp` and the engine's `LaneTag` take the prefix; the commands, and
    the `Server.lane_*` methods named after them, stay.
  - `EventSequence.lanes`, the **curves over the whole sequence**: an
    `Automation`, the one Rust type a `Track` and a `Region` hang too.
    ✅ **`automation`** *(the user, 2026-10-01: the multitrack's document uses
    the multitrack's names)*, as `Track.automation` is — the field, the intents
    and both clients' verbs. **An event's curves are its `automation` too**,
    as a region's are, and not its `expression` *(the user, 2026-10-01)*: what
    MPE's word added, the target already says (`{"bend": true}`), and the two
    moves between the levels (`lane_to_expression`, `expression_to_lane`) are
    renamed with them.
  - the roll's **markers**, which the code calls a lane (`osc_lane`, "the
    marker lane", "the markers lane", the `osc` tag) and are not one: the
    sequence's events of type `"osc"`, drawn as `(time, label)` marks on the
    roll's axis, read-only. The strip that shows them is not a structure, so
    "lane" goes. ✅ **`OscMarker`** *(the user, 2026-10-01)*: the prefix keeps
    them apart from `Multitrack.markers` (`Marker`, a named point on the
    timeline that sends nothing), which keeps its name.

  **`Automation` is one object read by four containers**, and stays one name.
  The struct, its points, its `points` vocabulary, the host's curve element
  and the tabulation are the same in all four; what differs is the container's
  reading — the axis (`Track`: seconds from the multitrack's start; `Region`:
  seconds from the region's; the sequence: beats from its start; an event:
  beats from the note's, running past its end) and the target's vocabulary
  (`{"port": ...}` on a track, `{control|bend|pressure|timbre|cc}` with
  `channel`, `min`, `max` in a sequence). ✅ The type's doc comment said "`at`
  is in seconds", which was false in two of its four uses; it says what each
  holder makes of it now *(with the rename)*. ✅ `at` was an untyped `f64`
  among `Second` and `Beat` positions: a `Point` is shared with every other
  curve, so typing the axis per holder is a generic `Automation` (or a point
  per unit), a change of its own rather than a rename's. **Done 2026-10-08**,
  as the generic: `Point<T>` and `Automation<T>` over the crate's
  `timebase::Axis` -- `Second` on a track and a region, `Beat` on a sequence
  and an event, plain `f64` (the default) for a curve nothing places. The
  wire is unchanged, so neither client moved; what is shared over curves
  (sampling, drawing, comparing) is generic over the axis and leaves it only
  through `Axis::number`.
- ✅ **`UndoHistory`, not `History`** *(the user, 2026-10-01)*: what
  `seq.history` answers. `History` is already the crate's pile in both
  clients (`clausters.document.History`, the structures registered and the
  walk), and the two would be exported side by side in the web client's
  index; the prefix says what it is to a script, the undo order its editors
  share.
- ✅ **No structure is constructed with an id** *(the user, 2026-10-01: a
  defect to correct)*. `clausters.multitrack.Automation` requires `id: int` in
  its constructor, and so do `Region` and `Track`: the same defect on the
  multitrack's side. `Automation` becomes one class that is a **value** when it
  is free (built by a script, held by nothing) and a **live view** once it
  belongs to a sequence or an event; the id is the document's to assign. The
  multitrack's own dataclasses follow when the multitrack is held as a handle,
  which is `C60`.

**The steps**, each a commit with both clients in it — the surface is one in
two languages, so neither ships ahead — the books, and `editors/edit_notes`
extended rather than a new example. The number surface is **deleted**, not
kept beside the objects, and every call site is rewritten in the same pass
(`npx pyright` in `clients/python`, `./build.sh && ./test.sh` in
`clients/web`).

- ✅ **C57 — A sequence's structures are objects.**
  - ✅ **C57.0 — The lanes renamed.** Every rename of the inventory above, in
    the crates, the host, both clients, the wire's JSON where a field moves
    (with the session format's migration), the books and the examples — its
    own commit, before any object exists. `clausters.form`, frozen, is not
    touched: its `Track` keeps the name.
    *(Done 2026-10-01, one commit per rename: `EventLane`, `TrackRow` with
    the wire and the widget-level `Multitrack` deleted, `automation`,
    `TakeLane`, `OscMarker`, and `Automation`'s doc comment. The session
    format is 5. Left open then: `at`'s type, above (since done), and the fifth sense of
    "lane" found on the way, in Found by use.)*
  - ✅ **C57.1 — Reading.** `seq.events` with its identity map, `SeqEvent`
    (`at`, its keys, `automation`, `.event`), `at`/`range`/
    iteration on the pattern of `Timeline`'s.
    *(Done 2026-10-01: `SeqEvents` and `SeqAutomation` are the live
    collections, `Automation` is one class that is a value or a held view,
    and the sequence's door reads ids by beat (`ids`) and curves by holder
    (`automation`). A web `SeqEvent` reads its keys with `get`/`has`/`keys`
    and a collection indexes with `item(i)`, where Python indexes.)*
  - ✅ **C57.2 — Writing through the objects.** `seq.events.add` answers a
    `SeqEvent`; `event.at = ...`, `event[key] = ...` and `remove()` are the
    vocabulary's `move`, `set` and `remove`; the curve collections answer
    `Automation`, which loses its constructor id; detaching and reattaching
    across a removal and its undo.
    *(Done 2026-10-01. The number surface is gone in both clients: `add`,
    `remove`, `move`, `set`, `entries`, `get`, `apply` and the six curve
    verbs. A free `Automation` added to a collection becomes the view, and
    the moves between the levels are the sequence collection's
    `to_events(curve)` and `from_events(target)`. The web `SeqEvent` writes a
    key with `set(key, value)`. Reattaching on an undo is the identity map's,
    and is exercised with the history in `C57.3`.)*
  - ✅ **C57.3 — `seq.history`.** The editing context outside `clausters.gui`,
    a script's change as a turn, `with seq.history(label)` as one entry. The
    acceptance is the roll: with it open, a script's change redraws it, is
    heard from the lane, and one Ctrl+Z in the window takes back a whole
    `with` block.
    *(Done 2026-10-01. `Editing` moved to `clausters/history.py`
    (`clients/web/src/history.ts`) beside `UndoHistory`; the web block is
    `seq.history.entry(label, () => ...)`, as `session.use` is the web's
    `with session`. A sequence an editor opens in a context it was handed,
    or a multitrack binds, is claimed by that context, so a script's change
    to a box's notes is the multitrack's turn too. A redone add carries the
    id it was given, so it brings back the same object; an edit that mints
    ids of its own is redone as the state it left, and a block is recorded
    as the states before and after it.)*

### A script plays and holds what the editors do (client arc, phased)

*(Opened 2026-10-01 by the user, with `C57`, from the same session: render a
`Timeline` into a sequence, edit it in the roll, close the window, and play
the result from the client with no GUI. `play` should answer an object that
represents the server transport the lane is loaded on, and everything should
work with the default session, with no `with` a script does not need.)*

`C57` makes what a script holds of a sequence an object; this arc makes the
rest of what the editors do reachable from a script with no window: playing
a sequence and holding a multitrack the way a sequence is held. Each milestone is a commit with both clients in it,
the books, and the existing examples extended.

- ✅ **C58 — `play(seq)` answers a transport.** A sequence sounds only
  through `NotesEditor.play()` (`gui/editing/events.py`): `_NotesPlayback`,
  one per server, loads its event lane and drives it, and nothing outside the
  window reaches it. `play(sequence)` is not one of the free `play`'s kinds.
  - **A transport object.** `TransportView` (`defs/server/transport.py`) is
    "a `Server` addressed through one of its transports": the server's
    `transport_*` methods with the id filled in and `__getattr__` delegating
    the rest, made so a `Timeline` or a GUI playback had something to point
    at (the web client's `transportAt` answers a `Server` the same way). It
    becomes an object of its own whose verbs are the transport's —
    `play`, `pause`, `stop`, `locate`, `loop`, `end`, `state`, and a `wait`
    a script may call or not — and it behaves as a routine does: a live
    session drives it with the same commands, an automated script waits on
    it. The GUI's playbacks and `Timeline.transport` take it too, so there
    is one.
  - **The playback leaves `clausters.gui`.** What `_NotesPlayback` does is
    the client's, not the window's; `NotesEditor` uses it instead of owning
    it. `play(sequence)` loads the lane and answers the transport it is on.
  - **Which transport.** Today each editor's is a constant in
    `clausters-editing` (`MULTITRACK_TRANSPORT` 0, `AUDIO_EDITOR_TRANSPORT`
    1, `NOTES_EDITOR_TRANSPORT` 3), so two plays of two sequences would share
    one. To decide when it is taken: one shared transport for every
    `play(sequence)`, or one handed out per play by an allocator, as node ids
    and buses are (`C55`).
  - **The end.** `wait()` returns when the contents end, with no end mark
    set by hand.
  - **The default session.** `play(sequence)` with no `with` and no server
    works: today `main.resolve_server` raises unless a server was booted, so
    the default session boots one on first use.
  *(Done 2026-10-01, with **one shared transport** — the notes editor's —
  as the user chose: a second `play` gives way to it. `server.transport_at(n)`
  answers a `Transport`, one object per id, with `play`, `pause`, `stop`,
  `locate`, `loop`/`unloop`, `end`, `playing`, `state` and `wait`, and the
  transport's other commands by their own names (`group`, `follow`, `fade`,
  `locate_sample`, `lane_new`, `sched_clear`); the server view it sends
  through is private, and `Timeline.transport` takes the object. Its
  positions are those of what is loaded on it: a sequence's beats, else the
  transport's seconds. The playback is `clausters/seq/playback.py`
  (`src/seq/playback.ts`), shared by `play(sequence)` and the notes editor,
  and a change made through the sequence's objects reaches the lane with or
  without a window. `main.server_or_boot()` (`serverOrBoot()`) is the
  default session's boot. `examples/basics/verbs` plays the rendered
  timeline as a sequence, `examples/transports.py` moved onto the object, and
  `docs/decisions.md` records the revision. In the web client `play` of a
  sequence answers a promise, and `playing()`, `end()`/`setEnd()` are
  methods.)*
- ✅ **C59 — A sequence back in a `Timeline`** *(closed 2026-10-03 without
  being implemented)*. `X3` decided that no
  timeline is rebuilt from its values (`crates/clausters-apps/PLAN.md`):
  `render_events` is the one-way change, and what the roll edits lives in the
  `EventSequence`. Reloading the *sequence* already works — inline in a
  session's `Source`, or as a `.mid`. What is open is a `Timeline` that holds
  the edited sequence, and it is **decided when it is taken**, between:
  - `Timeline.from_sequence`, which contradicts `X3` and would answer a flat
    list of fixed events — no generator, no nesting comes back;
  - an `EventSequence` as an entry of a `Timeline`, played in place as its
    other items are, with nothing rebuilt *(the recommendation)*.
  The reason to want either was playing the sequence from the client; with
  `C58` that is `play(sequence)`, so what is left is placing an edited
  sequence among a `Timeline`'s other entries.

  **Not a case to support** *(the user, 2026-10-03, after it had been built
  once)*: a `Timeline` is a client abstraction that renders to an
  `EventSequence`, the structure shared with the server, and the sequence is
  played by the server's transport. That direction is the whole relation, and
  `C59` should not have been opened. Built as an entry, the sequence made the
  timeline's client-side tree do what a transport's position already does --
  a sequence's beats placed by a client clock, matched per entry, re-read on
  each edit, measured through seconds -- and it played none of the sequence's
  curves. That build never reached `main`, and was discarded. The one thing it surfaced that stands on its own is
  that **no event with automation could be played from the client** except
  through a sequence's lane: that is `C62`.
- ✅ **C60 — The multitrack is held as a handle.** A sequence is a handle
  over the Rust structure: one tree. A `Multitrack` is a mirror —
  `Multitrack`, `Track`, `Region`, `Source` are dataclasses
  (`clausters/multitrack.py`) that cross as JSON (`write`/`read`): two
  trees. The editor's core is already a handle (`MultitrackEditorCore`);
  what a script holds is still the mirror. So `Region` and `Track` take an
  `id: int` in their constructor — the numbers `C57` removes from the
  sequence — `mt.track(id)` and `mt.region(id)` look up by number, and a
  script's change to a track, a region or their automation is no turn the
  editor sees. (A notes region's sequence is already the source table's
  handle, and since `C57.3` the multitrack claims it: a script's edit of
  it is the multitrack's turn, redrawn in the box and undone by its Ctrl+Z.
  The multitrack around it is what is not.)
  - The `C57` model on the multitrack: one tree in Rust; tracks, take lanes,
    regions, sources and automation as live objects with an identity map
    (`mt.tracks.add(...)` answers a `Track`, `region.position = ...`); ids
    the crate assigns; `mt.history(label)` making a script's change a turn.
  - It revises `O10`'s rule in `crates/clausters-document/PLAN.md` ("the
    clients round-trip; they do not hold handles"), whose tension `O12`
    wrote down and answered for the edit, not for the objects. The revision
    is written there in the same commit.
  - After `C57`, which settles the shape on the smaller structure.

  **The rule, written down first** *(the user, 2026-10-03: `O10`'s rule is
  no longer valid — it predates the applications that use the document and
  its history, and moving their logic into Rust, which also serves the
  standalone host, annulled it)*. The sequence had already left it — `X3.2`
  made it a handle and `C57` gave it live objects and an identity map — and
  nothing said so. It is written in `docs/decisions.md` ("The round-trip rule
  is annulled: the applications hold the structures") and under `O10`: the
  applications hold the structures they edit, a client holding one holds a
  handle, and what still crosses by value is data rather than a rule.

  **Decisions:**
  - **Sources stay the session's values.** A source is an entry of the
    session's table, not of the multitrack — the multitrack names it by id
    from a region's window — so holding it as a live object is a handle on
    the *session*, a structure of its own. `Session` keeps its table as it
    is and holds the multitrack's handle; a session handle is not this
    milestone.
  - **What has no identity stays a value**: `Content`, `Fade`, `Tempo`,
    `Meter` and `Span` are read as values and written whole through the
    object that holds them (`region.fade_in = Fade(0.1)`,
    `mt.set_tempo(Tempo(...))`). A `Marker` has an id, so it is a view.
  - **Every write is the crate's vocabulary**, with no new verb: a region's
    place is `PlaceRegion`, its length and window `TrimRegion`, its fades
    `FadeRegion`, a curve's points `SetAutomation`, a marker `SetMarker` /
    `RemoveMarker`, the loop and punch `SetRange`, the maps `SetTempoMap` /
    `SetMeterMap`, which take lane plays `SetActiveTakeLane`; any other field
    of a region, or a region added or removed, is its take lane's
    `SetTakeLane`, and any other field of a track, a take lane or a track's
    curve is `SetTracks`. Both are whole, and cost a copy of what they state
    — the crate's own trade-off for those verbs.
  - **Ids are minted by the crate**, from what the multitrack names
    (`fresh_id`, past its markers too). An id an undo took away may be minted
    again, which is safe: recording any edit drops the redo that could have
    brought the old one back.

**The steps**, each a commit with both clients in it, the books, and the
existing examples rewritten rather than new ones:

- ✅ **C60.1 — The handle.** `clausters_document::multitrack::call_json`,
  the door a handle speaks (reads by id, `apply` answering the inverse, ids
  minted); its C ABI and wasm faces, declared in `docs/bindings.md`; the
  multitrack editor over the shared structure, and `openMultitrack` over a
  handle, as `openNotes` is over a sequence's.
  *(Done 2026-10-03: the door is `clausters_document::multitrack::handle`,
  the C ABI `clausters_document_multitrack_{new,free,call}` and
  `clausters_apps_editing_open_multitrack` (core ABI 79, additive), wasm
  `JsMultitrack` and `JsEditing.openMultitrack`; the editor holds
  `clausters_apps::multitrack::Shared` and `over_json` opens one over a
  handle. Python binds it as `_native.MultitrackHandle` and
  `EditingCore.open_multitrack`; the web wrapper comes with the objects in
  `C60.2`, where the first caller is.)*
- ✅ **C60.2 — The objects.** `Multitrack` over the handle in both clients:
  `mt.tracks`, `track.take_lanes`, `lane.regions`, `track.automation`,
  `region.automation`, `mt.markers` as live collections whose `add` answers
  the object and whose members `remove()`; the fields read from the crate
  and written through the vocabulary above; an identity map; no constructor
  takes an id and no call answers or takes one (`mt.track(id)`,
  `lane.region(id)` go). The dataclass mirror is deleted, not kept beside the
  objects, and every call site is rewritten in the same pass.
- ✅ **C60.3 — `mt.history`.** A script's change through the objects is a
  turn of the multitrack's context — the editor's when one is open, which
  redraws and undoes it — and `with mt.history(label)` makes a block one
  entry. The editor stops writing the multitrack back onto the script's
  objects: there is only the one.

  *(`C60.2` and `C60.3` done 2026-10-03, in one commit, since the editor had
  to move onto the handle the moment the mirror went. `Multitrack` is a
  handle (`_native.MultitrackHandle`, web `JsMultitrack`) with an identity
  map; `Region`, `TakeLane`, `Track` and `Marker` are views with no public
  constructor, and `Tracks`, `TakeLanes`, `Regions`, `Markers` and `Curves`
  the live collections (indexed in Python, `item(i)` in the web client);
  `Automation`'s holder is a sequence or a multitrack through one protocol
  (`_curve`/`_write_curve`/`_remove_curve`). The editing context's
  `script_edit` and `block` take any structure that answers `_apply`,
  `_script_key` and `_forward`: a sequence's block is still its state before
  and after, a multitrack's is the edits made, which an undo walks in
  reverse. `View` takes the objects (`view.track_view(track)`), the editor
  opens over the handle (`open_multitrack`) and syncs no multitrack, and
  `Session` compares by what it writes. The shared vector generator reads
  its multitrack from data with the ids both suites look up, and writes it
  through the handle.)*
- ✅ **C61 — `span` and `selected`, the same two words in every editor**
  *(decided 2026-10-02 by the user: one word names one thing in every case)*.
  **Its context** *(the user, 2026-10-03)*: the editors it was decided for
  are the multitrack, the roll and the audio editor, whose operations act on
  what the hand marks -- not every editor. The points editor took the two
  words as well, with nothing to act on them ("The points editor carries a
  range and a selection it has no use for", `crates/clausters-gui/PLAN.md`), and
  lost them the same day: the three editors' `selected`, `select` and
  `unselect` are the `Marking` mixin's, which the points editor does not
  take. A view has
  two selections, and they are told apart by what they are, not by which
  editor holds them:
  - **`selected`** — with `select(...)` and `unselect()` — is **what the hand
    marked**, the view's state, in the structure's own type: a roll's events
    (`SeqEvent`s, done for the roll in `C58`'s follow-up), a multitrack's
    regions, an audio editor's samples. It is the window's: it enters no
    history, and two windows over one structure each have their own.
  - **`span`** is **the time range** an Alt+drag sweeps: drawn as a band,
    played by `play`, repeated by the loop. It is the **transport's**, since
    it says what sounds — it exists with no window (a sequence played with
    `play(seq)` loops with no roll open), every window over what a transport
    plays draws the one span, and the server's `/transport_loop` is the
    transport's too. Done for a sequence's transport (`t.span`, `t.loop()`,
    `t.unloop()`).
  - In an audio editor the two **coincide** in extent — the frames selected
    are the frames of the range — which is a particular case of the rule, not
    a second rule: `selected` is the samples (the data, with the value band
    of a sweep with height), `span` the interval.
  Done for the multitrack: `editor.transport` is the `Transport` it plays
  on, in seconds, its `span` the band a sweep leaves (drawn from a script
  through the crate's `span` verb) and its loop switch the window's `L`
  (a `/gui_set looping` the host takes as the key); `editor.selected` is
  the held `Region`s, read and set through the host's `selected` prop.
  And for the audio editor: `editor.transport` is the audio editors'
  shared `Transport` in the take's seconds, driven through the crate's
  `span`, `locate` and `play` verbs, and `editor.selected` is a `Segment`
  over `editor.buffer`, setting which sets the span. The value band the
  rule names for a sweep with height is not carried: the audio editor's
  crate never read one, so there was none to keep. `PointsEditor`, not an
  application yet, still keeps the old `editor.selection` dict — written
  under "Found by use". `examples/editors/span_selected` (and its page) is
  the manual check: the three editors' transports, spans and selections,
  step by step.
- ✅ **C62 — An event carries its curves** *(opened 2026-10-03 by the user,
  from `C59`'s review: there was no way to play an event with automation from
  the client; closed 2026-10-04)*. A sequence's curves — a channel's
  automation and a note's own — sounded only on the sequence's own playback,
  the event lane on a transport. An `Event`, a pattern or a routine had no
  curve at all, and `render_events` had none to keep. It is only about playing
  events with automation **from the client**; the server's lane is not touched
  (the user, 2026-10-03).
  - **A curve is part of the event**: `Event(..., automation=[curve])`, so
    whatever plays events — a pattern, a routine, a timeline — plays its
    curves, and `render_events` keeps them as the notes' own. A **channel's**
    curve is a playable of its own (`Automation({"control": "amp",
    "channel": 0}, points).play()`, `stop()`), read by the notes on that
    channel.
  - **Nothing is built on the server.** The note is the plain `/synth_new` it
    always was, and the client sets the controls its curves drive with
    `/node_set`, one value every block, in timed bundles. A control with no
    curve costs nothing, which is what the user asked of the design.
  - **One emitter per server** (`clausters.seq.curves.CurveEmitter`, web
    `seq/curves.ts`, reached as `server.curves`): it keeps what sounds by the
    clock it sounds on, wakes every 50 ms on that clock — a thread outside
    any clock, at once into a score — and sends the instants up to two wakes
    ahead, one bundle per instant holding every curve.
  - **What each value is is the crate's**:
    `clausters_editing::event_curves::window`, bound by both clients
    (`editing_event_curves`). A note's curve wins over its channel's, a bend
    adds the two onto the frequency the note started with, and a channel's
    curve is glided by the lane's own one-pole over `CURVE_LAG`.
  - **A curve reaches a note from its start to its off**, and the note keeps
    the last value through its release: a client is not told when a voice
    ends, and a `/node_set` to a node that is gone ends an offline render.
  - **A channel's curve reaches the notes played on the clock it is played
    on**: each clock is an axis of its own in the emitter.

  **The direction it was opened with was dropped**: an event graph per
  instrument sent with its def, a reader per control added as a slot, a
  private bus each, `/graph_map` naming it, a channel as a graph. A control
  mapped to a bus reads one value a block and a curve is sampled once a
  block, so the reader computed nothing a `/node_set` on the voice does not,
  and its three open points (the channel's slots, a FaustDef written as
  source, the lane on the same graph) went with it. The record, with the
  numbers that sized the emitter, is `docs/decisions.md`, "A played event's
  curves are values the client sends".

  `tests/test_event_curves.py` and the web `tests/event-curves.test.ts` read
  the bundles off an offline score; `examples/basics/verbs` (and its page)
  is the manual check: an event with a swell and a bend, and a channel's fade
  over a phrase.

### The numeric builtins: SuperCollider's vocabulary (client arc, phased)

*Opened 2026-10-09 by the user.* `clausters.builtins` is a port of the value
side of sc3's builtins and has stopped short of it: `blend`, `normalize`, the
random distributions, `clip`/`wrap`/`fold` with two bounds, the windows of the
operator table and the scale conversions are missing, and so are a dozen
operators scsynth's `UnaryOpUGen`/`BinaryOpUGen` have. The arc adds them **in the
shared core**, so a value a client computes and the same operator on the audio
thread are one function, and it settles first what "the same value" means.

**The contract**: a builtin is **one function in the shared core**, which the
real-time server, an offline render and both clients all call, so they share a
formula and, on one platform, the bits. scsynth is the reference for
**semantics** (what `sqrt` of a negative is, what `mod` returns for a negative
divisor), never for rounding: scsynth itself does not agree with itself — its
audio-rate kernels for `sin`, `cos`, `tan`, `log`, `exp`, `tanh` and their
neighbours are SIMD approximations while its control-rate ones call the system
libm, and that libm differs per platform. Nor does the arc rewrite what a
library already gives: where sclang took a function from the C++ libraries, the
core takes it from an established Rust one.

**The same bits across platforms was asked for and then declined** (the user,
2026-10-09), on `C63`'s measurement: one portable libm makes native and wasm
agree, and costs the audio thread too much on the platform that is fastest
today. Native and wasm keep agreeing to an ulp, as `docs/decisions.md`
("Native↔wasm render parity is a tight tolerance") already holds.

**Every change of behaviour is measured before it lands** (the user,
2026-10-09): what moves, by how much, at what cost — a regression is a defect
of the milestone, not a side effect of it.

Measured while opening the arc (2026-10-09):

- **The two clients do not agree today.** Python reaches the core natively
  (the system libm) and the web client through wasm (Rust's own libm). Over
  200 000 arguments the two differ on 1-2% of `sin`/`cos`/`log`/`log2`/`log10`,
  4% of `tanh`, 10% of `exp`/`pow`, 13% of `tan`/`hypot` and 25% of `sinh` —
  so `midicps` or `dbamp` already differs between the clients.
- **Seven operators disagree with scsynth's definitions**, four of them in
  meaning rather than rounding: `sqrt` (scsynth's is signed, `-sqrt(-x)`; ours
  is NaN below zero), `pow` (signed likewise), `log10` (of `abs(x)`), `mod`
  (floored into `[0, b)`; ours truncates); and three in rounding only:
  `cpsoct` (`x/440` against our `x*(1/440)`), `hypot` (`hypotf` against
  `sqrt(a*a+b*b)`) and `recip` (scsynth's SSE reciprocal estimate plus one
  Newton step, which differs between CPU vendors).
- **White noise is not where the two servers differ.** scsynth's Taus88 with
  `frand2` and our xorshift64 measure the same over 4M samples: RMS 0.5773 and
  0.5775 (1/sqrt(3) = 0.57735), spectral flatness 0.9995 both, every band
  within 0.03 dB, lag autocorrelation under 0.002. Porting Taus88 to replay
  scsynth's streams was **declined by the user**: matching a seed adds a second
  generator to a solved problem and opens divergences elsewhere.

- ~~**C63 — One libm for the core.**~~ **Dropped 2026-10-09 by the user, on
  its measurement.** The core's transcendentals through the `libm` crate on
  every target would have made native and wasm one implementation. Per call
  against the system's (glibc 2.43, x86-64), in `f32`: `log` 1.05x, `sin`/`cos`
  1.2x, `exp` 1.4-1.8x, `tan`/`tanh` 2.4-2.8x, `powf` 7.5x, and `exp10f` off a
  correctly rounded result on ~40% of arguments; in `f64` it is at par but for
  `exp` 1.7x, `ln` 1.5x, `log2` 2x and `pow` 4x. With the cheaper routes
  written in (`exp2` for powers of two, `f64` rounded once for `exp10`,
  `tanh` and a positive base's `pow`), the own cost per sample of an operator
  over an audio-rate signal was still 1.5x (`exp`, `midicps`, `dbamp`), 1.7x
  (`sin`), 1.8x (`tanh`) and 3.4x (`pow` with a moving base). The system's
  `f32` functions are correctly rounded and fast, and no pure-Rust library
  matches them. The record is `docs/decisions.md`, "Native↔wasm render parity
  is a tight tolerance". **What it left**, shipped on its own: `EnvGen`
  prepares a segment once instead of every sample, and a fixed-base power is
  one `exp2` — the bent envelope 0.83x, the exponential 0.87x and `linexp`
  0.85x of what they cost, and `bench_math` (`examples/bench.rs`) to measure
  the per-sample transcendentals, which nothing timed before.
- ✅ **C64 — scsynth's semantics for the operators that exist** *(done
  2026-10-09)*. `sqrt` and `pow` signed, `log10` of `|x|`, `mod` floored
  (scsynth's `sc_mod`, its two fast paths ported as written; a zero divisor
  gives 0), `cpsoct` as `x/440`, and `hypot` the libm's (no overflow on the
  squares) once `C63` was dropped. `recip` stays `1/x`: scsynth's SSE estimate
  gives different bits on different CPU vendors. The bitwise edge cases are
  written down as the core's: the `i32` cast saturates (NaN is 0), a shift
  count wraps modulo 32, the right shift is arithmetic. **Measured**: against
  scsynth's own definitions compiled from its headers, all six now agree on
  20 000 of 20 000 arguments (they were 50%, 50%, 75%, 50%, 11% and 17% off).
  No golden render and no example crosses the change — the one `pow` in a
  shipped def is the default's `2 ** x`, a positive base — and the parity
  vectors moved where expected: a `pow` of a negative base that was NaN is a
  value, and one `cpsoct` an ulp. `docs/schemas.md` says which meaning each
  operator has.
- ✅ **C65 — The operators scsynth has and the core does not** *(done
  2026-10-09)*. Unary `bitnot`, `rectwindow`, `hanwindow`, `welwindow`,
  `triwindow`, `ramp`, `scurve`; binary `idiv`, `roundup`, `urshift`,
  `amclip`, `scaleneg`, `firstarg` — appended to the op tables, reached by both
  clients' `builtins`, their graph methods and the per-bin programs. And
  `Clip`/`Wrap`/`Fold(in, lo, hi)`: a `BoundOp` table in the core
  (`apply_bound`, `bound_slice`), three server kinds, `.clip/.wrap/.fold(lo,
  hi)` on a graph node and `clip`/`wrap`/`fold` in `builtins`, over a new C
  door and wasm export (`clausters_core_bound`/`bound`). `distortneg` was left
  out: scsynth defines it and dispatches nothing to it, and sclang has no such
  method. **Measured** against scsynth's definitions from its headers: every
  one agrees on all 24 001 arguments of a sweep, the windows' mixed
  `f32`/`f64` precision included. The header's own `float` `sc_scaleneg` is
  wrong below zero and unused by any kernel; the core takes the kernels' form
  (`docs/decisions.md`, "`scaleneg` is the UGen kernels' form"). The example
  is `C69`'s.
- ✅ **C66 — Random values, named as sclang names them** *(done 2026-10-09)*.
  A `Draw` table in `clausters_core::rng` — `rand`, `rand2`, `rrand`,
  `exprand`, `linrand`, `bilinrand`, `sum3rand`, `coin`, `gauss`, and the
  whole-number `irand`, `irand2`, `irrand` — with `Rng::draw` and
  `Rng::windex` (`wchoose`'s relative weights), behind one C door
  (`clausters_rng_draw`, `clausters_rng_windex`) and two wasm methods. Both
  clients' `Rng` draws by these names, and the module functions draw from the
  context; `choose`/`wchoose` pick from a list. They **replace** `uniform`,
  `next_f64`, `next_below` and `choice` (the patterns, the examples and the
  books moved; a seeded `Pwhite`/`Prand` gives the values it gave). **The
  integers are their own functions** rather than sclang's dispatch on the
  receiver's type: a TypeScript number cannot say whether it was written `10`
  or `10.0`, and the two clients draw by the same calls. In Python
  `clausters.base` exports every draw but `rand`, which would hide its own
  module; the function is `clausters.rand`. **On a signal** the eight random
  operators sclang has (`.rand()`, `.rand2()`, `.linrand()`, `.bilinrand()`,
  `.sum3rand()`, `.coin()`, `.rrand(hi)`, `.exprand(hi)`) compose a new
  `RandOpUGen` carrying the distribution by name, a new value every sample
  from its own stream off the graph's seed walk. The generator is the one the
  server and the clients already shared; replaying scsynth's Taus88 stream was
  declined by the user (see the arc's preamble).
- ✅ **C67 — The numeric functions that are not operators** *(done
  2026-10-09)*. `clausters_core::numeric`: a `Calc` table — `degrad`,
  `raddeg`, `cbrt`, `log1p`, `expm1`, `erf`, `erfc`, `tgamma`, `lgamma`,
  `zapgremlins`, `wrap1`, `fold1`, `even`, `odd`, `graycode`,
  `nextpowerof`/`previouspowerof` (with `nextpoweroftwo`/`nextpowerofthree`
  in the clients), `moddif`, `softround`, `snap`, `lcurve`, `gausscurve`,
  `lg3interp`, `calcfeedback`, `bilin`, `biexp`, `besselj`, `bessely` — and a
  `ScaleCalc` table — `degkey`, `keydeg`, `nearestinscale`, `nearestinlist` —
  each one C door and one wasm export, and named functions in both clients'
  `builtins`. **`f64`**, as sclang computes them: no UGen computes them, so
  there is no `f32` result to match. The special functions come from the
  `libm` crate, never by hand. Two deliberate departures from sclang, both
  recorded where they live: a power maps to itself under `nextpowerof`
  (sclang's `ceil(ln x / ln b)` sends `8.0.nextPowerOfTwo` to 16), and
  `degkey` reads a fractional degree the way the rest of the core does
  (`split_degree`). The divergence the survey found is closed: `exp10` is in
  Python's `builtins` and on both graphs now, and `degree_to_midinote` is in
  Python's `builtins` beside the web client's `degreeToMidinote`.
- ✅ **C68 — Functions over arrays** *(done 2026-10-09)*. In
  `clausters_core::numeric`: `blend` (`Calc::Blend`; two lists blend point by
  point in the clients, the shorter extended cyclically), `blend_at` with
  sclang's read past either end as a `BoundOp` (`clip`, `wrap`, `fold`), a
  `ListOp` table — `normalize`, `normalizesum`, `integrate`, `differentiate`,
  `resamp0`, `resamp1`, `asrandomtable` — and `indexinbetween`/
  `indexofgreaterthan` in the scale table; in `clausters_core::rng`,
  `tablerand` and `scramble`. New doors: `clausters_core_list`,
  `clausters_core_blendat`, `clausters_rng_tablerand`,
  `clausters_rng_scramble` and their wasm faces. Both clients' `builtins`
  name the list functions and the random context draws `tablerand` and
  `scramble`. The name is sclang's `scramble`, not the `shuffle` this entry
  first said: sclang has no `shuffle` (its `perfectShuffle` is a riffle).
- ✅ **C69 — Documentation and the manual check** *(done 2026-10-09)*. The
  builtins pages of both clients' books are their API references, so the
  modules' own headers now name every family in order; the server's operator
  reference (`docs/schemas.md`) and both guides carried each milestone as it
  landed. The manual check is one example in two languages,
  `basics/sc_vocabulary` (`clients/python/examples/basics/sc_vocabulary.py`,
  `clients/web/examples/basics/sc-vocabulary.html`): a lead squared off by a
  signed `pow`, folded between two bounds, pulsed by a Hann window and
  breathed by a per-sample `rand2`, playing a phrase drawn with `scramble`,
  `degkey`, `exprand` and `wchoose` from a seeded session. The audit reads the
  pair as one example (the page's own ending declared, as every offline pair
  declares it); the Python render was run (no NaN, peak 0.19) and the web
  calls draw the same six notes. **Waiting on the ear**: the user listens to
  both.
Out of the arc: the special functions sclang takes from Boost and no
established Rust crate carries (elliptic integrals, Legendre and Laguerre
polynomials, zeta…).

### Patterns: SuperCollider's operators and library (client arc, phased)

*Opened 2026-10-10 by the user, from a survey of sc3 against both clients.* In
sc3 a `Pattern` is an `AbstractObject`: `Pseq([74, 60, 46]) + 2` is a
`Pbinop`, `pat.linlin(...)` a `Pnarop`, and a `Stream` composes the same way
(`UnopStream`, `BinopStream`, `NaropStream`). Here `Pattern` is a bare class
in both clients, so that line raises `TypeError` in Python and has no method
in TypeScript; the library has 10 pattern classes against sc3's 73; and
`AbstractObject` itself carries less than sc3's.

**The contract**: a pattern composes with **the same vocabulary a graph
does** — the same selectors, the same `AbstractObject` methods — and the
value each step yields is computed by the **`clausters.builtins` function of
that name**, so a pattern, a list and a signal answer one operator with one
function in the shared core. The arc adds no numeric rule of its own: a value
a pattern needs that the core does not have (a distribution, a list function)
goes into the core first, under the numeric builtins arc's contract.

**An operator on a pattern is per value; a list function is not an
operator.** `blend`, `linlin`, `clip` and every operator apply to each value a
stream yields, so a pattern has them. `normalize`, `normalizesum`,
`integrate`, `differentiate`, `resamp0`/`resamp1` and `asrandomtable` read a
whole list — a minimum and a maximum, a sum — and a stream may never end, so a
pattern does **not** have them, as in sclang (they are `ArrayedCollection`'s,
not `AbstractFunction`'s). The idiom is `Pseq(normalize([...]))`.

**The defect it closes on the way**: `builtins._is_seq` takes anything with
`__iter__` for a list, and a pattern has one — so `midicps(Pseq([69, 81]))`
reads the pattern whole into a list (and hangs on an endless one), and
`normalize(Pseq(...))` fails inside `_native.list_op` with `object of type
'Pseq' has no len()`.

**"pyramid" already names something else here.** The 750-odd occurrences
across the tree are the **peak pyramid** (`clausters_core::peaks::Pyramid`,
`MultiPyramid`, `JsPyramid`), the min/max/mean-square cache behind every
waveform view; sclang's list function `pyramid` exists nowhere, and sc3 left
it, with its neighbours, as a comment (`sc3/base/builtins.py`: `mirror`,
`mirror1`, `mirror2`, `stutter`, `rotate`, `pyramid`, `pyramidg`, `sputter`).
The prose says "the list function `pyramid`" wherever the peak pyramid is a
possible reading.

**A pattern is a function, as a UGen is** (the user, 2026-10-10): `sine(freq)`
returns the generic `Ugen("Sine", [freq])`, and `pseq([1, 2, 3]) + 60` returns
the generic `Pattern` the same way — lowercase names, the arc's break with
sclang's `Pseq`, which the UGens already made (`sine`, not `SinOsc`); the
decision goes into `docs/decisions.md`. Each pattern is **written as a
generator function under sc3's `@pattern` decorator**, which keeps the
function and its arguments and starts a fresh generator on every iteration: a
call is a reusable description, never a one-shot generator. One `Pattern`
class (with `EventPattern` beside it, chosen by the decorator) replaces one
class per pattern; `Punop`/`Pbinop`/`Pnarop` are generators like the rest; in
TypeScript it is `pattern(function*)`. Nothing outside the tests asks
`isinstance` of a concrete pattern class, so nothing depends on the classes.
**This is possible because no pattern holds state** (surveyed 2026-10-10 over
sc3's 73 classes and both clients' 10: none assigns to itself after it is
built, and both clients copy their items): a walk's position, a state
machine's state, `Pseed`'s seed are each stream's, in the generator's frame,
and a `Pfunc`'s state is its closure's. What an event pattern adds is not
state but the **incoming event**: `Pkey` reads it, `Pchain` composes it,
`Pbind` fills it — sc3's `inval = yield value` — so the decorator passes it
from `C70` on, though `C73e` is its first reader. The proxies (`Pdef`, `Pdefn`,
`Pbindef`, `PatternProxy`) are **out, decided by the user on 2026-10-10**:
they are JITLib's, not the pattern library's, and sc3 never carried them; a
source that changes makes them the one stateful kind, which a `@pattern`
could not be anyway.

**Value pattern and event pattern are the distinction the library is read
by** (the user, 2026-10-10): they differ in what their stream yields — a value
pattern yields values (numbers, lists, anything a key takes), an event pattern
yields `Event`s, and only an event pattern plays. Every class says which it
is, and the ones that can be either (a list or filter pattern over event
patterns, a composition over one) say how they resolve it, which is the rule
`Pseq`/`Prand`/`Pn` already follow: decided when the pattern is built. The
books teach the two kinds before any class.

**The library is organized by family, and each family is a module** (the
user, 2026-10-10), the way the UGens are (`defs/ugens/osc.py`, `filter.py`,
...) and the way sc3 was (`seq/patterns/`): value patterns, random patterns,
list patterns, filter patterns, function patterns, event patterns, time
patterns — `C73`'s phases. A module's header says what its family does and
how it differs from its neighbours, and **every pattern class carries a brief
description of what it yields**, one or two lines in its docstring (TSDoc in
the web client), as every UGen function does (`sine`: "Sine by f64 phase
accumulation, starting at phase 0."). The ten classes that exist today get
theirs in `C70`, and the module split happens there too, before the library
grows into it.

**A pattern is an iterable, in both languages** (the user, 2026-10-10):
`for x in Pseq([1, 2, 3])` in Python, `for (const x of new Pseq([1, 2, 3]))`
in TypeScript (`Pattern` implements `[Symbol.iterator]`), each loop over a
fresh stream; `list(pat)` / `[...pat]` read a finite one whole. Every pattern
the arc adds keeps this, a composed one included, and the books show the loop
as the first way to see what a pattern yields — with an endless one bounded
by the loop (`islice`, a `break`), never read whole.

- ✅ **C70 — A pattern is a function, and composes by operators** *(done
  2026-10-10)*. `pattern` (`seq.pattern(function*)` in TypeScript) builds a
  pattern from a generator function that receives the incoming value first;
  the ten patterns are lowercase functions (`pseq`, `pser`, `prand`, `pwhite`,
  `pseries`, `pgeom`, `pfunc`, `pn`, `pconst`, `pbind`) over one generic
  `Pattern` and `EventPattern`, in family modules (`seq/patterns/`:
  `valuepatterns`, `randompatterns`, `listpatterns`, `filterpatterns`,
  `funcpatterns`, `eventpatterns`, `oppatterns`), each with its description;
  every example, test and book page moved in the same commit. A `pbind` hands
  each key's pattern the event being built. `Pattern` and `Stream` implement
  the `AbstractObject` hooks: `punop`/`pbinop`/`pnarop` and a composed stream
  take one value from each operand per step, compute it with
  `builtins.value_op` (the `UNARY`, `BINARY`, `BOUND` and new `NARY` tables,
  mirrored in TypeScript as `valueOp`) and end with the shortest operand. **An
  event pattern refuses an operator**, where this entry first said it would
  propagate as sc3's did: an event has no arithmetic, so the composition could
  only fail later (`docs/decisions.md`, "A pattern is a function, as a UGen
  is"). A list or dict argument is copied when the pattern is built. Parity:
  `gen-pattern-vectors.py` freezes what each pattern yields, seeded draws
  included, and `pattern-parity.test.ts` builds the same cases. Both books have
  a "Patterns" chapter: the walk, the two kinds, the operators against the list
  functions, writing one.
- ✅ **C71 — `AbstractObject` carries sc3's whole surface, and the builtins
  take a pattern** *(done 2026-10-10)*. The range maps, `clip`/`wrap`/`fold`
  and the eight random operators moved from the UGen graph into the base, and
  the base grew `blend`, `bilin`, `biexp`, `moddif`, `degrad`, `raddeg`,
  `not_` (`not()` in TypeScript), `//` and `~` (`idiv`, `bitnot`), `rint`, the
  named `eq`/`ne` and `bitand`/`bitor`/`bitxor`/`leftshift`/`rightshift`
  (the builtins' names, not the `lshift`/`rshift` this entry first said). A
  pattern and a stream compute each by `value_op`, which now finds the draws
  and any builtin by name. A graph writes `degrad`, `raddeg` and `blend` with
  the operators it has and refuses a value function no UGen computes
  (`moddif`, `cbrt`) by name; a Faust signal or box refuses what it lacks
  through the base's hook (`a Signal has no 'linlin'`). The TypeScript base
  gained the n-ary and reflected hooks (`narop`, `rbinop`) and an internal
  `composeOp` door. **The builtins**: every element-wise builtin given a
  pattern, a stream or a signal answers what that object's method does, over
  its first argument (a binary operator reflects), so `midicps(pseq(...))`,
  `add(2, p)` and `degkey(pseq(...), scale)` are patterns; a list function
  refuses one with the idiom in the message, and `_is_seq` no longer reads a
  composable as a list. **Equality stays out of the operators.** The parity
  vectors gained the new methods, the seeded draws and the builtins over a
  pattern; both books' "Patterns" chapters name the whole vocabulary.
- ✅ **C72 — sclang's list functions left out of `C68`** *(done 2026-10-10)*.
  The core's `ListOp` table grows `mirror`, `mirror1`, `mirror2`, `stutter`,
  `rotate`, `pyramid` and `pyramidsizes`; the length change the entry left open
  is `ListOp::output_len`, which the table already had, now read by a new C
  door, `clausters_core_list_len`, so the Python binding stops computing it
  itself. `pyramid` follows sclang's `_ArrayPyramid` primitive (its ten types
  as runs, `numeric::pyramid_runs`, checked against the primitive's loops run
  by hand), and `pyramidg` splits it by `pyramidsizes` in the clients, which
  is what sclang's own `pyramidg` method builds. **They reshape any items**: the
  clients hand the core the indices and map them back, so `pyramid([pat_a,
  pat_b])` is a list of patterns. `sputter` is `Rng::sputter` in the core (the
  indices, each repeated while a coin of `probability` lands), behind
  `clausters_rng_sputter` and `JsRng.sputter`, and both random contexts draw
  it. All are list functions, and refuse a pattern. The clock parity vectors
  call each function and draw `sputter` from the same stream in both clients.
- ✅ **C73 — The pattern library** *(done 2026-10-10)*. sc3's patterns, phased by family, each one
  a `@pattern` function in both clients with its parity vector, its brief
  description and its kind (value, event, or resolved when built), in its
  family's module. The lists below cite sclang's names; each is written
  lowercase (`Pexprand` is `pexprand`):
  - ✅ **C73a — value and random patterns** *(done 2026-10-10)*: `pexprand`,
    `pbrown`, `pgbrown`, `plprand`, `phprand`, `pmeanrand`, `pbeta`,
    `pcauchy`, `pgauss`, `ppoisson`, `pprob`, `pvalue`. The core's `Draw`
    table grew `lprand`, `hprand`, `meanrand`, `betarand` (sclang's `Pbeta`
    rejection loop), `cauchy` and `poisson`, drawn by both random contexts
    under those names; `pprob` reads the table `asrandomtable` builds. Every
    bound, step and parameter may be a pattern, read one value per step as
    sclang reads them, and `pwhite`, `pseries` and `pgeom` gained that too.
    The walks step with `rand2`, which is what sclang's `xrand2` is on a
    float. Parity: each pattern's seeded walk in both clients, the libm draws
    to the shapes' tolerance.
  - ✅ **C73b — list patterns** *(done 2026-10-10)*: `pxrand`, `pwrand`,
    `pshuffle`, `pslide`, `pwalk`, `place`, `placep` (sc3's name for sclang's
    `Ppatlace`), `ptuple`, `pswitch`, `pswitch1`, `pfsm`, `pdfsm`, each an
    event pattern when its items are (`place`, `ptuple`, `pswitch1` and the
    state machines yield values). `pseq` and `pser` gained sclang's `offset`,
    and `pser` now embeds an item that is a pattern, as sclang's does. A
    `pdfsm` state is a mapping from a signal (or `"default"`) to the next
    state and the item, a dict in Python and an object in TypeScript. The
    parity vectors walk every one, seeded where it draws.
  - ✅ **C73c — filter patterns** *(done 2026-10-10)*: `pcollect`,
    `pselect`, `preject`, `pdrop`, `plen`, `pstutter`, `platch`, `pwhile`,
    `pwrap`, `pclump`, `pflatten`, `pdiff`, `pdelta`, `pdur`, `pgate`,
    `pprorate`, `pavaroh`, `ptrace` and `Pattern.trace`, with `pn`'s `key`.
    The `key` marks the event in place, as sclang's does, so a `pn` that is
    a key of a `pbind` marks the event being built and a `pgate` after it
    reads the mark. A function filter calls `func(value, inval)` when the
    function takes two arguments (`inspect` in Python, `length` in
    TypeScript). `pavaroh` reads with `degkey`. **`Pseed` is left out**,
    decided here: it gives a pattern a seed of its own, which is exactly what
    the random context declines ("no per-pattern seeds",
    `routines-and-clocks.md`); a pattern that must replay its own draws plays
    in its own routine. The parity vectors walk every filter, events
    included through their deltas.
  - ✅ **C73d — function patterns** *(done 2026-10-10)*: `prout` (a generator
    function walked as a pattern), `pfuncn`, `plazy`, `pif`, `pproduct` (its
    function first, as its family has it, `None`/`null` for the lists
    themselves), and `pfunc` now passes the incoming value to a function that
    takes one, as sclang's `Pfunc` does. sc3's `@pattern` decorator shipped
    with `C70`, where every pattern became one. The parity vectors walk each.
  - ✅ **C73e — event and time patterns** *(done 2026-10-10)*: `pkey`,
    `pchain` (sclang's `<>`), `pevent`, `pvoice` (below), `ppar`, `ptime`,
    `pstep`, `pseg`. `ppar` merges in time with a stable order for events
    that fall together, and sets `delta` on the event in place so what a
    source keeps of it survives; `pseg` reads each segment with the core's
    `shape_value`, behind a new door (`clausters_core_shape_value`,
    `shapeValue`). The time patterns read the routine's logical beat, so a
    plain loop holds their first value. `pvoice` shipped as designed below:
    the server realization is two event keys the core's synth render reads
    (`held`, `continues`), the render's ties and slurs come from
    `clausters_core::event::notation::articulation` behind
    `clausters_core_articulation`, and the voice reads one note ahead
    (`docs/decisions.md`, "A voice is notes and their joins, realized by each
    destination"). What a later edit does to the two shapes is open: "An
    edit to a voice's legato or slur does not rewrite the other" in Found by
    use.
    **`pvoice` is sclang's `Pmono` and `PmonoArtic`, as one voice** (the user,
    2026-10-10). sc3 already merged the two into `Pmono(instrument, mapping,
    articulate=False)`: `_mono_on` starts a node, `_mono_set` sets it, and
    with the flag an event whose `sustain < delta` (or a rest) releases it
    so the next one starts again. **The flag is `artic`** (the user,
    2026-10-10, renaming sc3's `articulate`): shorter, and in the patterns'
    own abbreviated style. Its events carry `server` and `node_id`,
    state of a sounding server, and no curves. The name is the exception to
    the lists' sclang spelling because what it yields is **a voice** in the
    sense notation, MIDI and MPE share — a line of notes, the voice of the
    `voice` key — and not the graph slot `crates/clausters-apps/PLAN.md` calls
    a note's voice.

    **Articulation and inflection are the two concepts it is designed by**
    (the user's framing, 2026-10-10, taken from the spoken voice). An
    **articulation** is how distinct notes relate: legato, detached, a slur, a
    tie, the attack marks of `articulations`. An **inflection** is a variation
    inside one note: what `Event.expression` holds (bend, pressure, timbre,
    any control), from the note's start. A change of note is an articulation
    in every abstraction, MPE included: an MPE note keeps the note-on it
    started with and is inflected by its bend, and the next pitch is a new
    note-on. So **every event a `pvoice` yields is a note**, `type: note` like
    any other, and what `pvoice` adds is the **articulation between
    consecutive notes** — legato, or detached when `artic` finds
    `sustain < delta` — written as document data, never as a `node_id`; each
    note's inflections stay its own expression. That makes a `pvoice` render
    into an `EventSequence` like any event pattern, and editable there.

    **Legato is realized by the instrumental abstraction, not written as
    curves.** The same two notes slurred are a slur in a symbolic score;
    overlapping note-ons in a MIDI channel in mono mode; on MPE a new note
    slightly overlapping the last, or a very fast glissando that is the
    previous note's bend; on the server a `set` of the one node the voice
    holds (sclang's `Pmono`, with a control's lag as the glide). Which one is
    the destination's, as X3.11 already makes the per-note curves the
    destination's (`crates/clausters-apps/PLAN.md`), and the rule is one,
    in the core.

    **The key is `legato`, and the document keeps both shapes** (the user,
    2026-10-10). A pattern yields events without ids — the document mints
    them when the events enter an `EventSequence` — so what a `pvoice` writes
    is the key every event already has: `legato >= 1` joins a note to the
    next in the voice, `legato < 1` makes the next re-attack (exactly
    `PmonoArtic`'s `sustain < delta`; in a `pbind`, where each event is its
    own node, the same value only overlaps). The render then writes the
    second shape over the ids it minted: a `tie` where the joined notes share
    a pitch (the notation key that exists) and a slur spanner over each joined
    run's ids, ending on the note that articulates. Each shape is derived from
    the other, so an edit to one recomputes the other and they cannot drift.

    ```python
    v = pvoice("lead", {
        "midinote": pseq([60, 62, 64, 64, 67]),
        "dur": 0.5,
        "legato": pseq([1.0, 1.0, 1.0, 0.6, 0.8]),
    }, artic=True)
    # stream: five notes, no ids; 60-62-64 joined, 64-64 a tie, 67 re-attacks
    v.render_events().data()
    # events 1..5 with their legato, event 3 with "tie": true,
    # and a slur over [1, 2, 3, 4]
    ```

    Where the spanner lives in the document (the apps plan names the
    sequence's list of spanners over event ids for the symbolic score) and its
    shape are the milestone's; the example's spelling is illustrative.
- ✅ **C74 — Documentation and the manual check** *(done 2026-10-10)*. Both
  books have a "Patterns" chapter, written as the arc went: the walk and the
  loop over a pattern, the two kinds, the operators against the list
  functions, the families, writing a pattern with `pattern`, and the
  articulation and inflection a voice is designed by. The manual check is one
  example in two languages, `basics/patterns`
  (`clients/python/examples/basics/patterns.py`,
  `clients/web/examples/basics/patterns.html`): a `pyramid` fed to a `pseq`
  with an operator over it, printed by a plain loop; a `pvoice` whose joined
  notes glide on one synth (a lagged `freq`) and render as ties and slurs; and
  a `pbind` counter-line of `pxrand`, `pstutter` and `pmeanrand` beside it
  under `ppar`, seeded. The audit reads the pair as one (the page's ending
  declared as every offline pair declares it); the Python render was run
  (4.4 s with a half-second tail, peak 0.22) and the page's patterns, ties and slurs were run under
  node and are the script's. **Waiting on the ear**: the user listens to both.

### The notebook client (`clausters-jupyter`) — moved to the `jupyter` branch

Shipped 2026-08-03/04 and taken off `main` on 2026-08-05. The package worked,
and the reason it left is not the feature but the shape: closing it opened six
hooks in this client and its TypeScript port whose only consumer was the
notebook, and one of them — an undeclared `interface.boot()` reached by
`getattr` inside `Server.boot`, whose implementer starts nothing and answers a
boot with a warning — is wrong enough to want redesigning rather than porting.
Reworking that under a running notebook is the slow way round.

The branch keeps the package, its front end (`clients/web/src/notebook/`), its
tests, its examples and its documentation, plus `ISOLATION.md`: the audit of
what the track put where, what it took back out of `main`, and what each hook
would have to become to return. What stayed here on its own merits is named
there too — the id share, the blob bulk path, per-instance hosts and pools, and
`boot` as an instance method.
