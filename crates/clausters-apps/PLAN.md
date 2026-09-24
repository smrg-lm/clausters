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

- ⬜ **X7 - The audio editor's nodes on the server.** *(Asked for by the user
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
  buffer's end (`dcf7b90a`). The multitrack's reader has the same gap, filed on
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
  - **Effects in preview**: a slot between the readers and the output, where
    an effect is heard and not written. Applying one to the take is a separate
    operation on the server.

  **Acceptance:** past the take's end the output is exactly zero; play and
  pause at any frame make no click, and a loop's wrap is not faded; the meter
  shows the level while it plays and falls to zero on a pause; closing the
  window frees every node, so the node tree after a close is the one before the
  open; the same nodes in both clients and in the standalone host; the audio
  editor's example shows the meter.

  **Open:**
  - The GraphDef itself: its members, buses, slots and surface. It is written
    out and reviewed with the user before it is built (the user asked for that
    review, 2026-09-23).
  - The stop's ramp: its length, and where the position comes to rest after a
    stop (the sample the stop was asked at, or the end of the ramp).
  - **One transport per server**: `/transport_group` binds one group, so a
    multitrack and an audio editor on the same server both want it.
  - Whether the host's monitor goes away, or stays for a window with no
    application behind it.
  - Whether the meter is per channel, and where it sits in the window.

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

## Found by use: the running list of fixes

Every entry carries a checkbox, and a fixed one stays with the record of what was
wrong.
