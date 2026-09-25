# clausters-apps — the applications over the document

The roadmap of `crates/clausters-apps`: the applications, each written once in
Rust, run by the standalone GUI host and bound by every client. A roadmap plus a
checkbox per milestone; the order in which they are taken is `ROADMAP.md`'s, and
a milestone's number is not an order.

## What the crate is

The rules are the crate's own (`src/lib.rs`): an application **draws nothing**.
It answers a window as a GuiDef and a correction as props, and reads the gestures
the host reports back — the same protocol every client speaks, which is what lets
one application run in a Python script, in a page and in `clausters-gui --session`
without knowing which. It never depends on the host and the host's renderer never
depends on it. The facts of a running system (widget ids, buffers, buses) are the
caller's and come in as arguments.

Under it: `clausters-document` holds the model, `clausters-editing` the
projections a structure owes its endpoints (props, payloads, the instance, the
applier). What is here is the application: the composition, the controls beside
the structure, and the answer to each gesture.

**The undo order is the crate's too.** `editing::Editing` holds one history and
seats every editor as a member (`Member::{Multitrack, Samples, External}`), so
two applications opened in one context walk one order and one opened alone is a
context of one. A structure the crate does not apply (a curve, a timeline, a
score) joins as an external member and gets its legs back.

## What is already here, and where it was recorded

| Application | In the crate | Recorded in |
| --- | --- | --- |
| multitrack editor | `multitrack::{window, props}`, `multitrack::editor::MultitrackEditor` | `crates/clausters-document/PLAN.md`, `O32`-`O33` |
| samples editor | `samples::{window, props, measures}`, `samples::editor::SamplesEditor` | `clients/gui/PLAN.md`, `AP7` |
| the undo order | `editing::Editing`, `turn` | `clients/gui/PLAN.md`, `AP7` step 4 |

**The shape every application here has had**, and the one each milestone below
follows unless it finds a reason not to: the window and its props in the crate;
the conversation (a gesture read, applied, answered with an `Outcome`) in the
crate; the doors (`clausters_apps_*` over the C ABI, the same functions over
wasm, declared in `docs/bindings.md`); both clients' editor a handle over the
crate that keeps only what a language owns (the socket, the awaiting, writing a
payload back onto its own objects); and the standalone host linking the crate
directly. The reference implementation is read call by call and deleted as each
part crosses, never delegated to.

A milestone here can open a **track** of its own when it turns out to be larger
than one milestone; the track then lives in this file, after the milestone that
opened it.

## The milestones

- ✅ **X1 - The audio editor.** *(Requirements stated by the user 2026-09-14;
  reopened 2026-09-23 for `X1.11`, closed the same day.)*
  The samples editor becomes **`AudioEditor`**: editing samples by hand (the
  pencil, one grabbed sample) is one of its operations and `SamplesEditor` stays
  the name of that part.

  **What it has to do that nothing does yet:**

  - **Cut, copy and paste over segments**, without copying samples. What exists:
    the segments are the Python client's (`clausters/segments.py`: `Segment`,
    `SegmentRun`, `BufferSegments`) and not Rust's; the server has
    `/buffer_stitch` and `/buffer_parts` (parts end to end, read-only, no
    overlap); no `/buffer_*` command changes a buffer's length; and the typed
    clipboard (`O7`) carries samples as `f32` blobs with nobody using it for this.
  - **Mix**, which segments cannot express because it computes new samples: an
    operation on samples, so the server's (the rule `S12` states — one place
    processes audio, and it is the server).
  - **A history in memory and on disk**, because editing large files is heavy and
    memory has to be used carefully. What exists: `clausters-document`'s
    `history.rs` has the `Spill` trait with one implementation, `MemorySpill`;
    `O5` and `O11` left the disk store "for the first caller that needs it"; `O8`
    decided the server's buffer is the working copy and the log keeps the span a
    write replaced, which was decided for short strokes. The design below
    replaces that for the audio editor.

  - **A history step re-reads what it changed, not the whole take.** Today the
    samples editor answers undo and redo with `reload`, which re-reads the
    whole buffer for a stroke of a thousand samples. `/gui_ack` already carries
    `source generation` pairs that the host keeps and nothing reads, and no
    client sends one (`clients/gui/PLAN.md`, Found by use, "A generation is
    carried, stored, and read by nothing"). What it waits on is what a source
    id is on that path -- a document's source, or a widget's `buffer=N` -- and
    this application is where that is answered.

  **The design: the history holds references to immutable takes, never samples**
  *(decided with the user 2026-09-22, after a survey of how audio and multitrack
  editors keep long sessions from exhausting memory; it also settles the
  document plan's Found-by-use entry "The takes a history can still reach are
  never given back", which is the same problem seen from the multitrack).*

  **What the field does, and the one principle under all of it.** Three shapes
  exist. *Immutable blocks shared between undo states*: a track is a sequence
  of blocks of about a megabyte, a block is never rewritten (a change makes new
  blocks), every undo state is a block list, so two states share everything
  that did not change and a duplicated selection costs no disk; a block goes
  when no state names it, and the history's space is reclaimed when the project
  closes. *A temporary file per undo level*: destructive editors copy the file
  to scratch and leave undo files on disk, so memory stays flat and disk grows,
  which is why they offer a manual "clear history" and warn when the scratch
  reserve runs out. *Non-destructive over files*: material stays in files, an
  edit changes regions, a render or a glue writes a new file; the history is
  light and some cap it in megabytes, dropping the oldest states; unused files
  are **never** deleted on their own, and one clean-up goes through a
  wastebasket flushed only in a later session. In all three **the history
  names immutable audio and never copies it**.

  **What clausters already has for that shape.** The server's joins
  (`/buffer_stitch`) are that block list: spans over takes, owning no
  samples, played by the engine directly, at mixed rates. `S19`'s regions make
  every buffer a mapped file under `--shm`, so "down to disk" already exists
  and the operating system's page cache is what manages memory. `S12` keeps
  every operation over samples the server's. And `History` already carries the
  deferred-free pattern (`forget`/`released`), for structures rather than
  sources; it caps entries (256) and not bytes.

  **The rules:**

  1. **A take is immutable once a history entry names it.** Cut, copy, paste,
     delete and insert produce a new **parts list** and move no samples; undo
     goes back to the previous list.
  2. **An operation that computes samples writes a new take the size of the
     span it touched** — mix, gain, a process, and **the pencil: a new take per
     gesture**, not a write into the buffer. The parts list splices it in.
     This is the block rule (a block is never updated) at the granularity of
     the edit instead of a fixed size.
  3. **A history entry holds source ids, not samples.** For audio the file-backed
     `Spill` is no longer needed — the takes *are* the disk store; `Spill` stays
     for payloads that are not audio.
  4. **A source is given back by reachability.** The roots are the document, the
     history's entries (undo and redo halves) and the clipboard; a take no root
     reaches — because the budget trimmed the pile, or a clear-history ran — is
     freed. `released()` generalized to sources, and it is what finally enforces
     `Lifetime::Temporary`'s "dies with the edit session".
  5. **Two budgets and an explicit clear**, as the field has them: the
     entry count that exists, plus a byte budget over what **only** the history
     holds, and a "clear history" verb.
  6. **Memory or disk is the backing's decision, not the history's.** With a
     mapped region the disk is the virtual memory. A save promotes only what the
     document reaches; the rest is temporary and goes when the session closes.
  7. **A parts list is flattened in Rust** (`clausters-core` or
     `clausters-editing`): a join over a join always resolves to one flat list of
     spans over takes, so repeated edits never approach the server's four-level
     nesting limit, and every client binds the one flattening.

  **What this reopens, stated rather than absorbed.** The document plan's
  decision of 2026-08-14 ("A destructive edit writes a temporary source, and
  becomes material by being rendered") and the one of 2026-08-17 ("Does the
  working copy still lead, now that a write costs the span?") put copy-on-write
  at the edit session and wrote strokes in place, on two arguments: a per-block
  copy rewrites a megabyte for a fifty-sample stroke, and a block sequence would
  have to be flattened to be heard. **Neither holds any more**: joins are
  played as they are, and a take the size of the span is not a block. For the
  audio editor the in-place write is replaced by rule 2; whether the samples
  editor over a buffer with no document behind it keeps the in-place path is
  read when the two converge into `AudioEditor`.

  **The steps**, each closed by its own commit, in this order because each
  one is what the next is written over:

  - ✅ **X1.1 - The parts list is Rust's.** A module of `clausters-document`
    over a flat `[Part]` (the join's own recipe, `session::Location::Segments`):
    its length, the parts a span of it is, and removing, inserting and
    replacing a span -- so cut, copy, paste and a new take spliced in are one
    arithmetic, written once. `picture.rs`'s private span reading becomes a
    caller of it. Every part here is at the join's rate: a mixed-rate list is
    the multitrack's case and is not widened into this one.
  - ✅ **X1.2 - A history names the sources it holds.** An entry declares the
    sources either of its halves reaches; the history answers which of a set
    no entry names any more, reports the ones its budget or a clear let go
    (`released()`'s rule, for sources), and trims by a byte budget over what
    only it holds. The roots it does not know -- the document, the clipboard
    -- are the caller's, and the question is asked with them.
  - ✅ **X1.3 - `AudioEditor`: the turns over a take made of parts.** Cut,
    copy, paste and delete over the selection as new parts lists; the pencil
    as a new take per gesture, spliced in; each turn answering the steps to
    carry out (allocate and write a take, restitch the drawn join, free what
    the history released) with the buffer numbers the caller hands in.
    *Landed as `clausters_apps::audio`, seated in `editing::Editing` as
    `Member::Audio`, with three answers taken on the way:* a take's source id
    is **its buffer number** (the question `X1.7` names, answered for this
    application); a **paste writes a new take** from the block the host's
    clipboard carries, since the clipboard is the host's and travels as
    samples -- a paste of what this editor itself cut could name its parts
    instead, and that is left open; and `/buffer_alloc` gained a
    `sampleRate`, so every new take is at the edited take's rate whatever
    the server runs at. A stroke over a take wider than one channel starts as
    the frames it was drawn over, copied out of the join by the server
    (`/buffer_gen copy`, which now reads only the span).
  - ✅ **X1.4 - Mix**, as the server's verb over two spans into a new take.
    `/buffer_mix` adds another buffer's frames into a span (the source may be a
    join); the editor's `mix` copies the frames under the block into a new take,
    writes the block into a scratch buffer it frees in the same steps, and mixes
    one into the other.
  - ✅ **X1.5 - The doors and both clients**: the C ABI, wasm, the Python
    `AudioEditor`, its web port, the standalone host; `docs/bindings.md`.
    The host's **cut puts nothing on the clipboard** (`ClipVerb::Cut` only
    reports the span): a cut is a copy and a removal, and the copy half is
    the host's to make, as its copy already is.
    *Landed with no new symbol*: the context's JSON door already carried any
    verb, so `openAudio` and `bytes` reach both clients through
    `clausters_apps_editing_call`, and a turn and a step answer `freed` under
    the member whose server the takes are on. `AudioEditor` in both clients
    walks the steps, tops up the buffers before every turn and frees what
    comes back; the host's cut now copies first, and **Ctrl+Shift+V** reports
    `"mix"`. The standalone host has no audio editor, and with the multitrack
    and the audio editor separate applications (`X1.9`) it has no box to
    open one from; running it there is its own question.
  - ✅ **X1.6 - Disk**: takes as regions under `--shm`, the browser's
    backing decided. *(Saving moved to `X1.9`, where it is what reaches the
    multitrack.)*
    **Decided with the user 2026-09-22: the browser uses its file system
    (the origin's private one, which `/buffer_write` and `/buffer_allocRead`
    already reach in a page) within the quota it is given.** That makes the
    backing **one rule on both platforms**, written once in the crate: a
    take only the history holds is **spilled** past a resident budget --
    written to a scratch path (`/buffer_write`, float, so the samples come
    back exact) and freed -- and a step whose list reads a spilled take
    reads it back (`/buffer_allocRead`) before the join is stitched. Its
    buffer number stays the take's identity while it is on disk. It helps
    natively too: a region under `--shm` lives in `/dev/shm`, which is
    memory. The byte budget stays the bound on everything the history
    holds, spilled or not, and a write the server refuses -- a quota full --
    leaves the take in memory (the member is told it was `kept`). The
    scratch directory is the caller's to name; a file is named by the buffer
    it held, so a directory holds at most one file per buffer number the
    session ever used.
    *Landed as the context's `resident` budget beside `bytes`: a turn and a
    step answer `stored` (the steps that write a take and free its buffer),
    a step that needs one back reads it before stitching, and a take freed
    from disk gives back its number without a `/buffer_free`. Both clients
    take `resident_bytes`/`residentBytes` and a `scratch` directory (a
    temporary one natively, one named after the join in a page); checked
    against a real server, where a take written out and read back returns
    exact.*
  - ➡️ **X1.7 - A step re-reads what it changed** (the `/gui_ack` generation,
    above). **Moved to `X2` with the user 2026-09-22**: it was written for an
    editor that writes a buffer in place, and the audio editor is not one --
    every edit stitches its join again and a cut moves everything after it, so
    re-reading the whole join is the right answer there.
  - ✅ **X1.8 - The books and the example** that is this milestone's manual
    test. `edit_audio.py` / `edit-audio.html`; both clients' composition
    chapters and `docs/architecture.md`. The script was checked by hand
    (2026-09-22): cut, undo and redo, copy, paste at the cursor.
  - ✅ **X1.9 - Saving the edited take.** **Reshaped with the user
    2026-09-22**, twice that day: the multitrack and the audio editor are
    **separate, incompatible applications** -- no box of the multitrack
    opens the audio editor, and nothing either does reaches the other. **A
    save writes over the file the take was read from**, and a save-as writes
    another file, which a later save writes over (`docs/decisions.md`). The
    editor's `save` writes the join through its parts (`/buffer_write`), as
    float unless told otherwise; Ctrl+S over its window is the same save.
    Checked against a real server: a cut take saved over its file reads back
    at the cut's length.

  - ✅ **X1.10 - The multitrack opens no editor over a box** *(decided with
    the user 2026-09-23)*. The multitrack edits non-destructively -- where
    things are, never the files or buffers a box reads -- so a double click
    on a box is a press like any other, and the `enter` outcome, the `box`
    verb and both clients' `enter` went. The applications are distinct and
    have distinct purposes; the rules of use that keep an application
    coherent are not what this crate implements. What it implements is what
    the audio editor can edit.
  - ✅ **X1.11 - The audio editor over a server buffer, with the pencil as
    one of its operations.** *(Stated by the user 2026-09-23.)* What the
    audio editor edits is **a file or a server buffer**, and `SamplesEditor`
    -- drawing samples by hand -- is a function of it rather than an
    application beside it. Over a buffer, **saving is rewriting the buffer's
    contents, or creating a new buffer**; reusing the buffer while it is
    being edited sounds a glitch, which is the user's to avoid and not the
    editor's to prevent. So the editor edits a private copy of what it opened,
    and the buffer it came from is written only by a save -- the rule the file
    already follows.
    *Landed 2026-09-23*: `open` makes the private copy (a buffer the caller
    hands over) and the history names only that; `save` takes a
    `Target::File` or a `Target::Buffer`, rewrites a buffer whole at the
    take's length through the join, and a save-as moves the target. Both
    clients' `save` take `path` or `buffer` (a `Buffer`, or `True`/`true`
    for a new one). Checked against a real server: a cut saved over its
    buffer leaves it at the cut's length, and an undo after it is whole.
    **And the samples editor is no longer an editor of its own** (with the
    user, 2026-09-23): `edit(buffer)` opens the audio editor in both
    clients, `SamplesEditor` and its member of the context went, and the
    `edit_samples` example pair with them -- `edit_audio` is the example. What
    stays of it is what the audio editor uses: the window a take is drawn in
    (`clausters_apps::samples`) and the reading of a stroke
    (`clausters_editing::samples`).

  **Open, and not decided here:** the browser has no mapping, so whether its
  history stays in memory under the byte budget or goes to OPFS; the budgets'
  defaults; where the segment model lands in Rust, given that
  `clausters/segments.py` has to come down to the core by the non-divergence
  rule; what a source id is on the `/gui_ack` path (above); how many takes a
  long day of pencil strokes leaves, and whether a gesture's take is coalesced
  with its neighbour's when the two are adjacent.

  **Related, each where it is written:** the audio editor as an analysis tool, in
  panes and layers on Sonic Visualiser's shape (`crates/clausters-document/PLAN.md`,
  `O24`); the layer stack (`clients/gui/PLAN.md`, `A5`-`A7`); spectral selection,
  the lasso and spectral drawing (`clients/gui/PLAN.md`, `D5`-`D7`).

- ⬜ **X2 - The buffer editor: drawing a table by hand.** *(Proposed by the user
  2026-09-14, on the samples editor as its model.)* A buffer on the server drawn
  and edited by hand — the manual counterpart of `/buffer_gen`, which computes the
  same tables from a formula. *(2026-09-23: the samples editor this was modelled
  on is gone -- a buffer opens in the audio editor, which edits a copy and
  writes it back on a save. What remains of its path is the in-place write,
  `/buffer_setRange` through `clausters_editing::samples::write_steps`, and
  whether a table is drawn in place or through the audio editor is this
  milestone's first question.)*

  **A history step re-reads what it changed, not the whole buffer** *(moved
  here from `X1.7`, 2026-09-22)*. An editor that writes a buffer in place --
  as this one may, and as the samples editor did -- answers undo and redo
  with `reload`, which re-reads the whole buffer for a stroke of a thousand
  samples. `/gui_ack` already carries `source generation` pairs that the host
  keeps and nothing reads, and no client sends one (`clients/gui/PLAN.md`,
  Found by use, "A generation is carried, stored, and read by nothing"). What
  it waits on is what a source id is on that path -- a document's source, or
  a widget's `buffer=N`; the audio editor answered it for itself (its takes'
  ids are their buffer numbers), which is the precedent to read first.

  **What a table is on this server, and what the editor has to respect**
  (`docs/schemas.md`, "Table generation and the wavetable format"):

  - `sine1`/`sine2`/`sine3` build **periodic** tables, `cheby` a transfer curve
    over `x∈[−1,1]` that holds its endpoint, `copy` overlays another buffer, and
    `env` discretizes a break-point curve through the same shape math `EnvGen`
    plays.
  - The `flags` pack `normalize` (1), `wavetable` (2) and `clear` (4).
  - A **wavetable-format** buffer does not hold the values: it holds interleaved
    offset/slope pairs, so an `N`-sample buffer is `N/2` points. Values drawn
    straight into one would be read wrong by `Osc`/`VOsc`/`Shaper`.

  **Open, and not decided here:** where the conversion to the wavetable format
  lives when the values are drawn (a core function the write calls, or a
  `/buffer_gen` command that takes the values); whether the flags are offered as
  `gen` offers them; whether the hand draws samples, or edits the parameters a
  command takes (a bar per harmonic for `sine1`, a point per segment for `env`,
  which is the `bpf` the curve editor already is); which picture draws a table.

- ⬜ **X3 - The notes editor.** *(Decided by the user 2026-09-14: the notes editor is
  complex, so it is worth making an application, since implementing it twice
  would be no good at all.)* The editor behind the `pianoroll` — a
  MIDI editor — written once here. Today it is `NotesDomain`, `NotesView` and
  `NotesEditor` in both clients (`clausters/gui/editing/events.py`,
  `src/gui/editing/events.ts`), over the host's `notes` element (the largest
  element after the multitrack and the signal family).

  **The question it opens is the one `O26` left:** the events view is shaped out of
  the **client's own objects** — a `Timeline` of `OscItem`/`MidiItem`, read through
  the client's `_pitch`, `_velocity` and `_label_of` — which is why it did not move
  with the other projections (`crates/clausters-document/PLAN.md`, `O26`). What the
  crate edits, then, is the first thing to settle, and it is not settled here.

  **Related:** the examples that edit notes through the raw event and have no
  history (`clients/python/PLAN.md`, "Half the editors a hand can use have no
  history") -- that entry closed on 2026-09-21 with `pianoroll` and
  `pianoroll_midi` as its only survivors, and **hands them here**, because
  porting them onto the editing seam waits on what this milestone edits; configurable key bindings (`clients/gui/PLAN.md`, `G36`) and the
  interaction-vocabulary entry beside it ("The whole interaction vocabulary is
  provisional…"); and an application inside
  another (`crates/clausters-document/PLAN.md`, Future directions), which is what
  would let a notes editor stand inside a multitrack or a script's window.

- ⬜ **X4 - The points editor: whether it is one.** *(Undecided; the user,
  2026-09-14, is in doubt: it may be good for it to have undo, and perhaps chrome
  could be added to it.)* What exists: the `bpf` element, `clausters_editing::points`
  (its view projection since `O25`), and `PointsDomain`/`PointsView`/`PointsEditor`
  in both clients. It already has a history — `edit(curve)` joins the context as an
  external member — so what an application would add is one implementation in
  Rust and chrome of its own. Whether that is worth a milestone is the decision.

- ⬜ **X5 - The score editor.** The third of the three applications over the
  document (`crates/clausters-document/PLAN.md`, `O24`). The notation model and
  what is still open about editing a page are the N track's
  (`clients/gui/PLAN.md`: `N7` what opening a foreign score preserves, `N8` which
  element admits which edit, `N9` the score in the arrangement). A `Score` already
  joins the editing context as an external member. What the application is, over
  that track, is not written yet.

- ⬜ **X6 - The composed views: which heavy widgets get an application.**
  *(Raised by the user 2026-09-14: it may also be worth moving some composed
  widgets, such as the scope and the waveform and spectrogram viewers.)*
  The drawing stays in the host — the crate never depends on it — so what could
  move is what each client composes around those widgets today:

  | Composition | Python | Web | Widgets |
  | --- | --- | --- | --- |
  | `scope()` | `scope.py` (251) | `scope.ts` (289) | `scope`, `phasescope`, `spectrum` over the server's taps |
  | `plot()`, and the patch window | `plot.py` (440) | `plot.ts` (550) | `plot` after an offline render; `patch` |

  and the pictures whose props are already one rule in
  `clausters_document::view::catalogue` (`waveform`, `bpf`, `pianoroll`). The
  heavy elements, for the record (`clients/gui/src/host/elements`, lines as of
  2026-09-14): `multitrack` 4963, the `signal` family (`waveform`, `spectrogram`,
  `plot`, `scope`, `spectrum`, `phasescope`) 3677 plus its GPU pipelines and frame
  slots, `notes` 1775, `curve` 803, `score` 788, `patch` 694, `keys` 547.

  **Open:** which of these are applications and which stay a client's composition;
  whether a monitor (scope, phase, spectrum, meters) is one application or several.
  **Related:** "The heavy families as features" (`clients/gui/PLAN.md`), which is
  about the bundle's size rather than where an application lives, over the same
  widgets.

- ✅ **X7 - The audio editor's nodes on the server.** *(Done 2026-09-24,
  heard and seen by the user in both examples; what is left is at the end of
  this plan.)* *(Asked for by the user
  2026-09-23: the multitrack has a node design on the server, and the audio
  editor needs its own, at least to watch its amplitude on a level meter;
  the shape below is the user's, the same day.)*

  **What exists.** The audio editor sounds its take through the GUI host's
  monitor (`clients/gui/src/host/play.rs`): one `clausters-gui-take` reader per
  channel, in a group bound to the transport. The host allocates the nodes and
  holds them until another take is played or the monitor is stopped. Nothing
  measures what the readers write, and no client knows the nodes exist. The
  multitrack has what this lacks: its node system is written once in
  `clausters_core::mixer` (GraphDefs for the multitrack, the track and the clip,
  with meter and send slots), and `clausters_editing::playback` carries it out.

  **What was found without it** *(2026-09-23)*. Space over a take with no
  selection played on past the take's end, and the reader held the take's last
  sample on the output for as long as the transport rolled. It was measured at
  +0.65 after an edit left a loud last sample: a DC offset on the whole system's
  audio that nothing in the window showed. The monitor's gate now closes at the
  buffer's end (`a082e583`). It went on sounding, measured on 2026-09-24:
  only the standalone session sent the monitor's def, so a host launched
  against a script's server played an older copy that the server had
  persisted to disk. Every attached link now sends it first. The multitrack's reader has the same gap, filed on
  its own (`crates/clausters-document/PLAN.md`, Found by use).

  **The design.** The audio editor gets **a GraphDef of its own**, new, not
  the multitrack's. The two applications stay separate: what the editor plays
  is shaped like one clip, without the clip's strip. Anything the multitrack's
  defs need fixed is a separate fix.

  ```text
  editor group                       not governed: never frozen
  |- transport group                 /transport_group: frozen and thawed by the transport
  |  `- playing                      readers, one per channel of the take
  |     |                            -> [fx] effects in preview, not written to the file
  |     `-> a bus the editor owns
  `- output                          reads that bus
     |- meter                        -> a control bus: the editor's level meter
     |- declick                      a ramp on play, pause and stop
     `- out                          -> the hardware
  ```

  - **The output is outside the transport group**, because it must not
    freeze. A paused meter falls to zero rather than holding what it last
    saw, and the declick has to run across the moment the readers stop. The
    same holds for the **master input**: the input's own master, whose
    meter measures what arrives whether the transport rolls or not, so it is
    never frozen either (the user, 2026-09-23).
  - **Play and pause reach the nodes as the engine's state, not as a signal
    passed down the tree.** `/transport_play` and `/transport_stop` become
    `Cmd::TransportRun`, which pauses or resumes the governed group at the
    exact sample (`Engine::apply`), and every UGen reads whether the transport
    rolls from `ProcessCtx::transport`. So the output group needs no message of
    its own, and wrapping both groups in a parent adds nothing to how the
    state arrives. The parent is still what owns them and frees them together.
  - **The declick needs a change in the transport.** A fade out cannot happen
    after the freeze, because on that sample the readers stop producing
    anything to fade. So a stop has to become a short stopping phase: the
    governed group keeps running and the position keeps advancing while a
    ramp that a UGen outside the group reads goes to zero, and only then is the
    group frozen. A play thaws the group and ramps up. The loop's wrap is not a
    stop and gets no ramp, so a loop is heard as the material joins. A locate
    while rolling is a discontinuity, and it is correct that it is one (the
    user, 2026-09-23).
  - **The reader's window is the take.** The editor stitches the join, so it
    knows its length after every edit and states it as the readers' `span`,
    and nothing past the end sounds. The per-sample check against the buffer
    that the monitor carries today is not needed.
  - **Effects in preview**: a chain between the readers and the output, where
    an effect is heard and not written. Applying one to the take is a separate
    operation on the server.

  **The GraphDef, for review** *(written 2026-09-23; nothing of it is built)*.
  Two graphs and three SynthDefs, named with their own prefix (`ae`) so
  nothing in them is the multitrack's. `N` is the take's channel count. The
  JSON is the wire's own shape (`docs/schemas.md`, "GraphDef"), written out for
  a stereo take.

  ```text
  root (0)
  └─ editor group                  /group_new, owned by the editor; never frozen
     ├─ transport group            /transport_group on the editor's own transport (T6)
     │  ├─ ae.play2                file in focus; graph: private bus dry (2); external bus out (2)
     │  │  ├─ [source] slot group  /graph_addSlot, one per channel of the take
     │  │  │  └─ ae.reader         chan 0 -> dry:0
     │  │  ├─ [source] slot group
     │  │  │  └─ ae.reader         chan 1 -> dry:1
     │  │  ├─ [fx] slot groups     effects in place on dry, in chain order; none by default
     │  │  └─ ae.pass2             dry -> out
     │  └─ ae.play1                another open file, mono; paused (/node_run 0)
     │     ├─ [source] slot group
     │     │  └─ ae.reader         chan 0 -> dry:0
     │     └─ ae.pass1             dry:0 -> out:0 and out:1 (a mono file on both sides)
     └─ ae.output2                 graph: external bus in (2), the editor's bus
        ├─ ae.meter2               in -> two control buses (the level meter)
        └─ ae.declick2             in × TransportFade -> hardware out 0, 1
  ```

  The bus between the two graphs is one **the editor allocates** (N audio
  channels from the client's bus allocator) and hands to both as their
  external bus. A graph's private buses belong to its own instance, and the
  two graphs live in different groups, so no private bus can join them.
  `ae.output2` is added at the tail of the editor group, after the transport
  group, so it reads the bus in the same block the readers wrote it.

  **`ae.reader`** -- one channel of the take, following the transport. It is
  the multitrack's reader without the parts a take played whole does not use:
  no `at` (the take starts at the transport's zero), no `start`, no `loop`
  (the transport loops) and no `rate` beyond the buffer's own.

  ```json
  {"name": "ae.reader",
   "controls": [
     {"name": "out",  "default": 0},
     {"name": "buf",  "default": 0},
     {"name": "chan", "default": 0},
     {"name": "span", "default": 0}],
   "ugens": [
     {"kind": "TransportPos", "inputs": [{"const": 0}]},
     {"kind": "BinaryOpUGen", "op": "ge", "inputs": [{"ugen": 0}, {"const": 0}]},
     {"kind": "BinaryOpUGen", "op": "lt", "inputs": [{"ugen": 0}, {"control": 3}]},
     {"kind": "Mul", "inputs": [{"ugen": 1}, {"ugen": 2}]},
     {"kind": "BufRateScale", "inputs": [{"control": 1}]},
     {"kind": "Mul", "inputs": [{"ugen": 0}, {"ugen": 4}]},
     {"kind": "BufRd", "inputs": [{"control": 1}, {"control": 2}, {"ugen": 5}, {"const": 0}]},
     {"kind": "Mul", "inputs": [{"ugen": 6}, {"ugen": 3}]},
     {"kind": "Add", "inputs": [{"control": 0}, {"control": 2}]},
     {"kind": "Out", "inputs": [{"ugen": 8}, {"ugen": 7}]}]}
  ```

  - `span` is the take's length on the transport, in engine samples (`frames`
    times the engine's rate over the take's): the editor stitches the join,
    so it knows the length after every edit and sets `span` in the same turn.
    Past it the gate is exactly zero, and nothing is compared against the
    buffer per sample.
  - The gate is a step at both ends, with no ramp: the take's first and last
    samples are heard as they are, so a click in the file is heard as a click.
  - `Out` writes `out + chan`, so each reader lands on its own channel of
    `dry` whatever the slot hands it as `out`.

  **`ae.pass2`** -- `dry` onto `out` at unity: `In` per channel into `Out`.
  It is what an empty `fx` slot leaves sounding, and the point after the
  effects where the take leaves the play graph.

  **`ae.play2`**:

  ```json
  {"name": "ae.play2",
   "buses": [
     {"name": "dry", "rate": "audio", "channels": 2},
     {"name": "out", "rate": "audio", "channels": 2, "external": true}],
   "members": [
     {"def": "ae.pass2", "controls": {"in0": "dry:0", "in1": "dry:1",
                                      "out0": "out:0", "out1": "out:1"}},
     {"def": "ae.reader", "slot": "source", "controls": {"out": "dry"}}],
   "surface": {
     "buf":  [{"member": 1, "control": "buf"}],
     "chan": [{"member": 1, "control": "chan"}],
     "span": [{"member": 1, "control": "span"}]}}
  ```

  **The effects are a linear chain in place on `dry`** *(decided by the user,
  2026-09-23)*. Each effect reads `dry` and replaces it (`In` ...
  `ReplaceOut`), and a chain is several of them in the order they were added:
  the auto-sort counts `ReplaceOut` as a read and a write, so inserts on one bus
  keep their insertion order (`docs/auto-order.md`). **A bypass pauses the
  effect** (`/node_run 0`). A paused node writes nothing, so `dry` carries on
  as the effect before it left it, and the next effect reads that -- bypassing
  one in the middle of a chain is sound. What a bypass does not do by itself:
  switch without a click (the wet signal is replaced by the dry one between
  two samples), keep a tail (a reverb's ring stops where it is), or forget the
  past (a resumed delay plays what it held when paused). Those are the chain's
  details, below.

  **A bypass is a pause, and removing an effect is another operation**
  *(weighed with the user, 2026-09-23)*. Freeing the effect's node sounds the
  same while it is out -- the bus passes untouched and nothing runs -- but
  bringing it back differs. A chain's order is its insertion order, so an
  effect freed from the middle comes back last, unless every effect after it
  is rebuilt or the server grows a positioned insert that an auto-sorted group
  does not take today. Freeing also loses what hangs off the node: its id,
  its ports' values, a `/graph_map` onto a curve or a control bus, and its
  state, all of which the editor would have to keep and send again. A pause
  keeps all of it, and costs one message each way. Freeing gives back memory
  and nothing else, which matters for a heavy effect left out a long time (a
  convolution reverb and its impulse response). That is **removing** the
  effect from the chain, a separate verb in the editor, and not a bypass.
  Neither avoids the click of switching between the processed and the direct
  signal: that needs the ramped mix below, before the pause, either way.

  **How a bypass switches without a click** *(the user, 2026-09-23)*. Every
  effect in the chain mixes its own processed and direct signal, and the mix is
  what ramps; the pause comes after the ramp has finished:

  ```text
  dry    = In(dry)
  wet    = the effect over dry
  amount = EnvGen(ASR, linear segments, gate = on)     // 0..1, reaches both exactly
  ReplaceOut(dry, LinXFade2(dry, wet, amount * 2 - 1))
  ```

  Bypassing sets `on = 0`, waits the release time and pauses the node
  (`/node_run 0`). Since the envelope has reached exactly 0 the output is
  exactly `dry`, and the pause leaves no step. Enabling resumes the node and
  sets `on = 1`. The alternatives, and why not them:

  - **`Lag`** (a lagged control: one pole, scsynth's coefficient, -60 dB in
    `time`) is asymptotic and never reaches 0 or 1, so the pause still cuts a
    residue of 0.1% of the difference between the processed and the direct
    signal.
  - **`VarLag`** is the same one-pole smoother with separate rise and fall
    times. It is not scsynth's `VarLag`, which runs a shaped segment, even
    though the name is the same.
  - **`Line`** and **`XLine`** are linear but run once, when the node is
    created, so they cannot switch back and forth.
  - **`XFade2`** is the equal-power crossfade. The processed and the direct
    signal are correlated, so at the middle of the fade an equal-power law is
    about 3 dB loud. **`LinXFade2`** keeps the amplitude constant, which is the
    law for two versions of one signal.

  **One `ae.play` per open file** *(the user, 2026-09-23)*. The editor works on
  several files at once -- tabs, or a list of open files -- and only one of
  them plays. So the transport group holds one `ae.play` instance per open file,
  all writing the editor's bus, and **every one but the file in focus is
  paused** (`/node_run 0` on the instance). The transport's freeze is the
  transport group's own flag, and a child's `/node_run` flag is its own
  (`NodeTree::set_paused` sets the one node it names), so a thaw does not wake a
  paused file. Switching files pauses one instance and runs the other; nothing
  is rebuilt. Copying a span from one file and pasting it into another, or into
  a new empty file, is the editing context's: each open file is a member with
  its own history (`crate::editing`), the clipboard is the host's, and a new
  empty file is a member over a new take. **Open:** whether the files share the
  editor's one position, with the editor locating to each file's own cursor on
  a switch, or each file has a transport of its own (`T6`).

  **`ae.meter2`** -- the multitrack's meter, the same algorithm under the
  editor's own name: `In` per channel, `Meter` with the field's ballistics
  (`clausters_core::mixer::METER_DECAY`, `METER_HOLD`), `OutCtl` onto a
  control bus the host draws. It is written once in `clausters-core` and only
  its name is the editor's. **`ae.declick2`** -- `In` per channel, times
  `TransportFade`, onto the hardware.

  **`ae.output2`**:

  ```json
  {"name": "ae.output2",
   "buses": [{"name": "in", "rate": "audio", "channels": 2, "external": true}],
   "members": [
     {"def": "ae.meter2",   "controls": {"in0": "in:0", "in1": "in:1"}},
     {"def": "ae.declick2", "controls": {"in0": "in:0", "in1": "in:1"}}],
   "surface": {
     "meter/out0":  [{"member": 0, "control": "out0"}],
     "meter/out1":  [{"member": 0, "control": "out1"}],
     "meter/decay": [{"member": 0, "control": "decay"}],
     "meter/hold":  [{"member": 0, "control": "hold"}],
     "out0": [{"member": 1, "control": "out0"}],
     "out1": [{"member": 1, "control": "out1"}]}}
  ```

  - **The meter reads the take before the declick**, so it shows the take and
    not the ramp. It still falls to zero on a pause, because the frozen readers
    write nothing and the bus is cleared every block.
  - **A mono take** is one reader, and its pass writes `dry:0` onto both
    channels of the editor's bus, so the take is heard on both sides at unity,
    as an audio editor plays a mono file. There is no pan law, since there is
    no strip. The width is the pass's to fix and not the output's, because the
    editor's bus is shared by every open file and they need not all be the
    same width.
  - **`TransportFade`** is a new UGen: the level of the transport's
    declick ramp, `1` while rolling, falling to `0` across the stopping phase
    and rising from `0` on a play. The engine publishes it in
    `ProcessCtx::transport` beside `rolling`, and with T6 it is the ramp of the
    node's own transport.

  **Acceptance:** past the take's end the output is exactly zero; play and
  pause at any frame make no click, and a loop's wrap is not faded; the meter
  shows the level while it plays and falls to zero on a pause; closing the
  window frees every node, so the node tree after a close is the one before the
  open; the same nodes in both clients and in the standalone host; the audio
  editor's example shows the meter.

  **Open:**
  - ✅ The GraphDef above, until the user has reviewed it. *Built as the
    progress entry below says, and accepted by ear and by eye (2026-09-24).*
  - The `fx` chain: moved to "Future directions", *"Effects in preview in
    the audio editor"*.
  - ✅ The stop's ramp: its length, and where the position comes to rest after a
    stop (the sample the stop was asked at, or the end of the ramp).
    *Decided by the user 2026-09-24, and shipped the same day*: the transport
    rolls in full through the ramp and **the position rests at its end**,
    where the readers stopped reading; the length is **per transport**,
    `/transport_fade <t> <samples>`, `0` (no ramp) by default, and the editor
    asks for its own. The end mark starts its ramp that long before the mark.
    `TransportFade` reads the level (`docs/decisions.md`, "A stop with a ramp
    rolls the ramp out, and rests where the readers stopped").
  - ✅ **The defs and the playback** *(2026-09-24)*:
    `clausters_core::audio_editor` and
    `clausters_editing::audio_playback::AudioEditorPlayback`, heard offline in
    `tests/audio_editor_graph.rs`. Three things moved from the GraphDef above
    while it was built. The names carry a dot before the width, as the
    mixer's do (`ae.play.2`). **The editor's bus reaches both graphs as port
    values** (`out0..` on `ae.play`, `in0..` on `ae.output`): a `/graph_new`
    at the top is handed no external bus, since it has no parent to hand it
    one. And **the output has one meter**, the level on control buses: the
    mark that waits is the `meter` widget's own ballistics, which are the
    core's. The transport is `AUDIO_EDITOR_TRANSPORT` (1) with a 5 ms ramp.
  - ✅ **Where the nodes live.** *Decided by the user 2026-09-24*: as the
    multitrack's do -- one design pattern in the repo. The defs are written
    once in `clausters-core`, the playback that makes them is the editing
    crate's, bound in C and wasm, held by both clients and the standalone
    host, and the application answers the keys with what the transport is
    asked to do.
  - ✅ **Which transport the editor takes.** Transport 1
    (`AUDIO_EDITOR_TRANSPORT`); the multitrack stays on 0. The editor group
    **follows** it (`/transport_follow`), so `ae.output` reads it without
    being frozen, and the window names it as its head clock.
  - ✅ **Whether the host's monitor goes away** *(2026-09-24)*: it stays, for
    a window with no application behind it, and it **is the same playback**
    -- the host holds an `AudioEditorPlayback` on a transport of its own
    (`play::MONITOR_TRANSPORT`, 2), so playing a take pane never moves a
    multitrack's position, and the old one-reader-per-channel def is gone. A
    window whose owner plays it says `plays` and the space bar is its
    owner's verb, the loop switch beside it; the audio editor's window says
    so, and the editor answers with the pass (`Outcome::play`).
  - ✅ **Whether the meter is per channel, and where it sits** *(taken as the
    default, 2026-09-24)*: one column per channel of the editor's bus, at
    the take's right, read in decibels off the control buses `ae.meter`
    writes.
  - The drawn play cursor of a take at another rate, and several files in
    one editor: moved to "Found by use" and "Future directions".
  - ✅ **The two cursors go together** *(the user, 2026-09-24)*: placing the
    position cursor -- a click, Home, End -- cues a stopped transport there,
    so the play cursor stands on it (`Outcome::cue`, the playback's `cue`,
    and the host's monitor alike); a rolling pass is left alone. Letting go
    of a selection puts the position cursor at its start. End is the take's
    last frame, and Home and End reach the take when the pointer is over the
    meter beside it.

- ✅ **X8 - A pass ends where the contents do, and a loop is a switch.**
  *(Done 2026-09-24; the standalone half of the multitrack's switch is
  deferred to "Future directions", below, by the user's decision.)*
  *(Asked for by the user 2026-09-24, out of the audio editor example; the
  key is the user's: "usá la tecla L, aún no vamos a hacer chrome para las
  apps y es mejor que el demo quede limpio".)* Needs `T7` (`PLAN.md`), the
  transport's end mark.

  **The audio editor.** Unless the editor is looping, a pass stops at the end
  and the play cursor goes back to the position cursor: the end of the take,
  or of the selection when there is one. The monitor sets the mark there with
  the position cursor as its `return`, and frees its readers when the
  transport reports the stop. **`L` switches the loop**: looping, a selection
  plays over and over and so does a take with none. It is the editor's state,
  not a prop of the view — no chrome, no button — and a line in the status bar
  says which way it went.

  **Shipped for the audio editor** *(2026-09-24)*, in the host, so both
  clients and the standalone host have it: the monitor's pass is a
  `play::Pass`, a loop or an end with its return, and `L` is a window key of
  both fronts. The host tells the engine's end from a stop it sent by counting
  the stops it sent (`play::Follow`), since every transport command's
  broadcast says "stopped" and only a transition nobody here caused is a pass
  that ended.

  **The multitrack, optionally.** The same end, at the end of its contents:
  the latest start plus duration of any clip on any track. It is a setting of
  the multitrack's playback, off by default, computed by the crate's playback
  (`clausters_editing`) so both clients and the standalone host set the same
  mark; an edit that moves the last clip moves the mark.

  **Shipped for the multitrack as a switch** *(2026-09-24)*:
  `MultitrackPlayback::set_stop_at_end` in `clausters_editing`, the end taken
  from `Multitrack::end` on every `sync` and the return from the position
  cursor `cue` and `stop` already carry, sent as the transport's end mark only
  when it moves -- bound in C and wasm and exposed as `Playback.stop_at_end` /
  `Playback.stopAtEnd` in both clients.

  **Decided for the standalone host** *(the user, 2026-09-24)*: the switch is
  reached with **a key**, and it is **saved in the session**. The
  infrastructure is here (the crate's switch), so the implementation waits for
  the applications' window chrome in standalone — both in "Future
  directions". Still open: whether a selection in the multitrack plays once as
  the audio editor's does.

  **Acceptance:** in the audio editor, a pass with no loop stops at the take's
  end and the play cursor stands on the position cursor; a selection stops at
  its end; with `L` on, both loop; the monitor holds no reader after the stop.
  In the multitrack with the setting on, playback stops at the last clip's end.
  Both clients and the standalone host, and the example names `L`.

## Definition of done (per milestone)

The project rule: code plus tests, a clear commit message, this file's checkbox,
the developer and user documentation where the change touches them, and a
commented example when the feature is user-facing — the example being how new
visible behaviour is checked by eye. Both clients and the standalone host stay
green, the doors are declared in `docs/bindings.md`, and the client code an
application replaces is deleted in the same pass. A `docs/decisions.md` entry
only for a choice with non-obvious context.

## Future directions (to fold into milestones as they firm up)

Every entry carries a checkbox.

- ⬜ **The applications' window chrome in standalone** *(recorded 2026-09-24,
  the user: "para poder correr las aplicaciones del host en standalone se va
  a necesitar que la ventana tenga chrome y no está implementado aún")*. A
  standalone host runs the applications with no client beside it, so whatever
  a script would set — a switch, a mode, a setting — has to be reachable from
  the window itself, and today the windows have no chrome for it. The audio
  editor's loop is a key and a status line precisely because there is none
  (X8). What the chrome is, and which of it is the application's rather than
  the widget's, is the design.

- ⬜ **The multitrack's stop-at-end in standalone: a key, saved in the
  session** *(decided by the user 2026-09-24; out of X8)*. The switch exists
  (`MultitrackPlayback::set_stop_at_end`, bound in both clients); the
  standalone host needs a key that flips it and the session to keep it, so a
  reopened session stops where it stopped before. Waits for the chrome entry
  above.

- ⬜ **Effects in preview in the audio editor** *(out of X7)*: a chain in
  place on `dry`, between the readers and the pass, whose effects are heard
  and not written; applying one to the take is a separate operation on the
  server. The shape is decided in X7 (a linear chain on `ReplaceOut`, a bypass
  that ramps `LinXFade2` and then pauses, removing as a separate verb); what
  is decided with the first effect is what a resumed effect does with the
  state it froze with (a delay line still holding the past), and how an
  effect is moved in the chain, since an auto-sorted group refuses a manual
  move.

- ⬜ **Several files in one audio editor** *(out of X7)*. The playback holds a
  file per open take and pauses all but the one in focus; the clients open
  one take per editor, so what tabs or a list of open files look like, and
  whether the files share one position or each locates to its own cursor on
  a switch, is open.

## Found by use: the running list of fixes

Every entry carries a checkbox, and a fixed one stays with the record of what was
wrong.

- ✅ **The play cursor of a take at another rate than the engine's runs
  ahead** *(found building X7, 2026-09-24)*. The audio editor's playback
  converts every frame to the engine's samples and the reader scales its
  phase, so the take is heard at its pitch and ends where it ends; the play
  cursor is drawn from the transport's position read as frames of the take,
  so over a 44.1 kHz take in a 48 kHz session it runs ahead by the ratio. The
  view has to scale the position by its rate over the engine's.
  **Fixed 2026-09-25**: `Host::head_clocks` — the one place a playhead's
  reading is resolved, for both fronts and whoever plays — reads a transport
  on a view that declares its samples' rate as that view's own frames, the
  position times its rate over the engine's. The device clock is left alone,
  since an anchor on it is in the engine's samples. The reading is rounded to a
  whole frame: a locate sends the frame as the nearest engine sample, and
  scaled back unrounded it stood a fraction of a frame beside the position
  cursor at a zoom that draws samples.

- ⬜ **A looping selection does not follow a new selection** *(the user,
  2026-09-25)*. Before X7 the audio editor's window played through the
  host's monitor, and a sweep moved the monitor's loop live
  (`transport_follows_selection`). Since X7 the window plays its own take
  (`plays`) on the editor's transport, and the sweep still sets the loop of
  the monitor's -- which is not sounding. The editor's `selection` arm keeps
  the span for the next play and tells the playback nothing. It has to
  answer a new selection, while a loop plays, with the playback's
  `set_loop`, bound for both clients; and the host has to leave the
  monitor's loop alone in a window that plays itself.
